use crate::model::OutputRotation;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ScreenCaptureStore {
    inner: Arc<Mutex<Option<ScreenCaptureFrame>>>,
}

#[derive(Clone)]
struct ScreenCaptureFrame {
    width: usize,
    height: usize,
    xrgb8888: Vec<u8>,
}

impl ScreenCaptureStore {
    pub fn update_from_scanout_xrgb8888(
        &self,
        src_pixels: &[u8],
        src_stride: usize,
        src_width: usize,
        src_height: usize,
        src_flipped: bool,
        output_rotation: OutputRotation,
    ) {
        if src_width == 0 || src_height == 0 {
            return;
        }
        let packed = pack_xrgb8888(src_pixels, src_stride, src_width, src_height, src_flipped);
        let (width, height, xrgb8888) =
            rotate_scanout_to_view(&packed, src_width, src_height, output_rotation);
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = Some(ScreenCaptureFrame {
            width,
            height,
            xrgb8888,
        });
    }

    pub fn write_png(&self, path: &Path) -> io::Result<()> {
        let frame = {
            let guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "screen capture unavailable")
            })?
        };

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, frame.width as u32, frame.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder.write_header()?;

        let mut rgb =
            Vec::with_capacity(frame.width.saturating_mul(frame.height).saturating_mul(3));
        for pixel in frame.xrgb8888.chunks_exact(4) {
            rgb.push(pixel[2]);
            rgb.push(pixel[1]);
            rgb.push(pixel[0]);
        }
        png_writer.write_image_data(&rgb)?;
        Ok(())
    }
}

fn pack_xrgb8888(
    src_pixels: &[u8],
    src_stride: usize,
    src_width: usize,
    src_height: usize,
    src_flipped: bool,
) -> Vec<u8> {
    let row_len = src_width.saturating_mul(4);
    let mut packed = vec![0u8; src_height.saturating_mul(row_len)];
    if row_len == 0 || src_stride == 0 {
        return packed;
    }

    for y in 0..src_height {
        let src_y = if src_flipped {
            src_height.saturating_sub(1).saturating_sub(y)
        } else {
            y
        };
        let src_start = src_y.saturating_mul(src_stride);
        let src_end = src_start.saturating_add(row_len).min(src_pixels.len());
        let dst_start = y.saturating_mul(row_len);
        let dst_end = dst_start.saturating_add(src_end.saturating_sub(src_start));
        if src_end <= src_start || dst_end > packed.len() {
            continue;
        }
        packed[dst_start..dst_end].copy_from_slice(&src_pixels[src_start..src_end]);
    }

    packed
}

fn rotate_scanout_to_view(
    src_pixels: &[u8],
    src_width: usize,
    src_height: usize,
    output_rotation: OutputRotation,
) -> (usize, usize, Vec<u8>) {
    match output_rotation {
        OutputRotation::Deg0 => (src_width, src_height, src_pixels.to_vec()),
        OutputRotation::Deg180 => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            RotationTransform::Rotate180,
        ),
        OutputRotation::Deg90 => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            RotationTransform::Rotate90Clockwise,
        ),
        OutputRotation::Deg270 => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            RotationTransform::Rotate90Counterclockwise,
        ),
    }
}

enum RotationTransform {
    Rotate90Clockwise,
    Rotate180,
    Rotate90Counterclockwise,
}

fn rotate_xrgb8888(
    src_pixels: &[u8],
    src_width: usize,
    src_height: usize,
    transform: RotationTransform,
) -> (usize, usize, Vec<u8>) {
    let (dst_width, dst_height) = match transform {
        RotationTransform::Rotate180 => (src_width, src_height),
        RotationTransform::Rotate90Clockwise | RotationTransform::Rotate90Counterclockwise => {
            (src_height, src_width)
        }
    };
    let mut dst = vec![0u8; dst_width.saturating_mul(dst_height).saturating_mul(4)];

    for y in 0..src_height {
        for x in 0..src_width {
            let (dst_x, dst_y) = match transform {
                RotationTransform::Rotate90Clockwise => {
                    (src_height.saturating_sub(1).saturating_sub(y), x)
                }
                RotationTransform::Rotate180 => (
                    src_width.saturating_sub(1).saturating_sub(x),
                    src_height.saturating_sub(1).saturating_sub(y),
                ),
                RotationTransform::Rotate90Counterclockwise => {
                    (y, src_width.saturating_sub(1).saturating_sub(x))
                }
            };
            let src_index = (y.saturating_mul(src_width).saturating_add(x)).saturating_mul(4);
            let dst_index =
                (dst_y.saturating_mul(dst_width).saturating_add(dst_x)).saturating_mul(4);
            if src_index.saturating_add(4) > src_pixels.len()
                || dst_index.saturating_add(4) > dst.len()
            {
                continue;
            }
            dst[dst_index..dst_index + 4].copy_from_slice(&src_pixels[src_index..src_index + 4]);
        }
    }

    (dst_width, dst_height, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deg90_capture_rotates_landscape_scanout_into_portrait_view() {
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        let (width, height, rotated) = rotate_scanout_to_view(&src, 2, 2, OutputRotation::Deg90);
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            rotated,
            vec![
                0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, //
                0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn store_writes_png_from_latest_frame() {
        let store = ScreenCaptureStore::default();
        let src = vec![0x10, 0x20, 0x30, 0x00];
        store.update_from_scanout_xrgb8888(&src, 4, 1, 1, false, OutputRotation::Deg0);
        let path = std::env::temp_dir().join(format!(
            "surf-ace-screen-capture-test-{}.png",
            std::process::id()
        ));
        store.write_png(&path).expect("png should be written");
        let metadata = std::fs::metadata(&path).expect("png metadata should exist");
        assert!(metadata.len() > 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn store_writes_png_to_relative_path() {
        let store = ScreenCaptureStore::default();
        let src = vec![0x10, 0x20, 0x30, 0x00];
        store.update_from_scanout_xrgb8888(&src, 4, 1, 1, false, OutputRotation::Deg0);

        let temp_root = std::env::temp_dir().join(format!(
            "surf-ace-screen-capture-relative-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_root).expect("temp root should exist");
        let prior_dir = std::env::current_dir().expect("current dir should exist");
        std::env::set_current_dir(&temp_root).expect("current dir should switch");

        let path = Path::new("relative-capture.png");
        store.write_png(path).expect("png should be written");
        let metadata = std::fs::metadata(path).expect("relative png should exist");
        assert!(metadata.len() > 0);

        std::env::set_current_dir(prior_dir).expect("current dir should restore");
        let _ = std::fs::remove_file(temp_root.join(path));
        let _ = std::fs::remove_dir(temp_root);
    }

    #[test]
    fn deg90_capture_honors_flipped_mapping_before_rotation() {
        let store = ScreenCaptureStore::default();
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        store.update_from_scanout_xrgb8888(&src, 8, 2, 2, true, OutputRotation::Deg90);

        let frame = store
            .inner
            .lock()
            .expect("capture frame should lock")
            .clone()
            .expect("capture frame should exist");
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(
            frame.xrgb8888,
            vec![
                0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, //
                0x02, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
            ]
        );
    }
}
