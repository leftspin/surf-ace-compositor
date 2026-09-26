use crate::model::{OutputRotation, PaneId};
use crate::output_rotation_model::{CapturePixelRotation, OutputRotationModel};
use crate::root_geometry::{CaptureGeometry, LogicalRect, RootGeometrySnapshot};
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ScreenCaptureStore {
    inner: Arc<Mutex<ScreenCaptureFrames>>,
}

#[derive(Clone, Default)]
struct ScreenCaptureFrames {
    committed: Option<ScreenCaptureFrame>,
    staging_generation: Option<u64>,
    staged: Option<ScreenCaptureFrame>,
}

#[derive(Clone)]
struct ScreenCaptureFrame {
    src_width: usize,
    src_height: usize,
    output_rotation: OutputRotation,
    packed_xrgb8888: Vec<u8>,
    root_geometry: Option<RootGeometrySnapshot>,
    pane_captures: Vec<(PaneId, CaptureGeometry)>,
}

impl ScreenCaptureFrame {
    fn view_xrgb8888(&self) -> (usize, usize, Vec<u8>) {
        rotate_scanout_to_view(
            &self.packed_xrgb8888,
            self.src_width,
            self.src_height,
            self.output_rotation,
        )
    }
}

impl ScreenCaptureStore {
    #[cfg(test)]
    pub(crate) fn root4_generations_for_test(&self) -> (Option<u64>, Option<u64>) {
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        (
            guard
                .committed
                .as_ref()
                .and_then(|frame| frame.root_geometry)
                .map(|geometry| geometry.generation),
            guard.staging_generation,
        )
    }

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
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.committed = Some(ScreenCaptureFrame {
            src_width,
            src_height,
            output_rotation,
            packed_xrgb8888: packed,
            root_geometry: None,
            pane_captures: Vec::new(),
        });
    }

    pub(crate) fn update_root4_scanout_xrgb8888(
        &self,
        src_pixels: &[u8],
        src_stride: usize,
        src_width: usize,
        src_height: usize,
        src_flipped: bool,
        root_geometry: RootGeometrySnapshot,
        pane_captures: &[(PaneId, CaptureGeometry)],
    ) {
        if src_width == 0 || src_height == 0 {
            return;
        }
        let packed = pack_xrgb8888(src_pixels, src_stride, src_width, src_height, src_flipped);
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let frame = ScreenCaptureFrame {
            src_width,
            src_height,
            output_rotation: root_geometry.rotation,
            packed_xrgb8888: packed,
            root_geometry: Some(root_geometry),
            pane_captures: pane_captures.to_vec(),
        };
        if guard.staging_generation == Some(root_geometry.generation) {
            guard.staged = Some(frame);
        } else {
            guard.committed = Some(frame);
        }
    }

    pub(crate) fn begin_root4_stage(&self, generation: u64) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.staging_generation = Some(generation);
        guard.staged = None;
    }

    pub(crate) fn commit_root4_stage(&self, generation: u64) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.staging_generation == Some(generation) {
            if let Some(frame) = guard.staged.take() {
                guard.committed = Some(frame);
            }
            guard.staging_generation = None;
        }
    }

    pub(crate) fn discard_root4_stage(&self, generation: u64) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.staging_generation == Some(generation) {
            guard.staging_generation = None;
            guard.staged = None;
        }
    }

    pub(crate) fn root4_stage_has_frame(&self, generation: u64) -> bool {
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.staging_generation == Some(generation)
            && guard
                .staged
                .as_ref()
                .and_then(|frame| frame.root_geometry)
                .is_some_and(|geometry| geometry.generation == generation)
    }

    pub fn write_png(&self, path: &Path) -> io::Result<()> {
        let frame = {
            let guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.committed.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "screen capture unavailable")
            })?
        };
        let (width, height, xrgb8888) = frame.view_xrgb8888();

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder.write_header()?;

        let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
        for pixel in xrgb8888.chunks_exact(4) {
            rgb.push(pixel[2]);
            rgb.push(pixel[1]);
            rgb.push(pixel[0]);
        }
        png_writer.write_image_data(&rgb)?;
        Ok(())
    }

    pub fn write_root4_png(
        &self,
        path: &Path,
        logical_rect: LogicalRect,
    ) -> io::Result<CaptureGeometry> {
        let frame = {
            let guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.committed.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "screen capture unavailable")
            })?
        };
        let root_geometry = frame.root_geometry.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "root4 capture geometry unavailable",
            )
        })?;
        let geometry = root_geometry.capture_geometry(logical_rect);
        let crop = geometry.physical_pixel_rect;
        let xrgb8888 = crop_xrgb8888(
            &frame.packed_xrgb8888,
            frame.src_width,
            frame.src_height,
            crop.x as usize,
            crop.y as usize,
            crop.width as usize,
            crop.height as usize,
        )?;
        write_xrgb8888_png(path, crop.width as usize, crop.height as usize, &xrgb8888)?;
        Ok(geometry)
    }

    pub(crate) fn write_root4_full_png(&self, path: &Path) -> io::Result<CaptureGeometry> {
        let frame = {
            let guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.committed.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "screen capture unavailable")
            })?
        };
        let root = frame.root_geometry.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "root4 capture geometry unavailable",
            )
        })?;
        let geometry = root.capture_geometry(LogicalRect {
            x: 0.0,
            y: 0.0,
            width: root.logical_size.width,
            height: root.logical_size.height,
        });
        let crop = geometry.physical_pixel_rect;
        let xrgb8888 = crop_xrgb8888(
            &frame.packed_xrgb8888,
            frame.src_width,
            frame.src_height,
            crop.x as usize,
            crop.y as usize,
            crop.width as usize,
            crop.height as usize,
        )?;
        write_xrgb8888_png(path, crop.width as usize, crop.height as usize, &xrgb8888)?;
        Ok(geometry)
    }

    pub(crate) fn write_root4_pane_png(
        &self,
        path: &Path,
        pane_id: &PaneId,
    ) -> io::Result<CaptureGeometry> {
        let frame = {
            let guard = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.committed.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "screen capture unavailable")
            })?
        };
        let geometry = frame
            .pane_captures
            .iter()
            .find(|(candidate, _)| candidate == pane_id)
            .map(|(_, geometry)| geometry.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "pane capture unavailable"))?;
        let crop = geometry.physical_pixel_rect;
        let xrgb8888 = crop_xrgb8888(
            &frame.packed_xrgb8888,
            frame.src_width,
            frame.src_height,
            crop.x as usize,
            crop.y as usize,
            crop.width as usize,
            crop.height as usize,
        )?;
        write_xrgb8888_png(path, crop.width as usize, crop.height as usize, &xrgb8888)?;
        Ok(geometry)
    }
}

fn crop_xrgb8888(
    pixels: &[u8],
    src_width: usize,
    src_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> io::Result<Vec<u8>> {
    if x.saturating_add(width) > src_width || y.saturating_add(height) > src_height {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture crop exceeds scanout",
        ));
    }
    let mut cropped = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for row in y..y + height {
        let start = (row.saturating_mul(src_width).saturating_add(x)).saturating_mul(4);
        let end = start.saturating_add(width.saturating_mul(4));
        cropped.extend_from_slice(&pixels[start..end]);
    }
    Ok(cropped)
}

fn write_xrgb8888_png(path: &Path, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let writer = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(writer, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder.write_header()?;
    let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
    for pixel in pixels.chunks_exact(4) {
        rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    png_writer.write_image_data(&rgb)?;
    Ok(())
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
    match OutputRotationModel::new(output_rotation).capture_pixel_rotation() {
        CapturePixelRotation::Identity => (src_width, src_height, src_pixels.to_vec()),
        CapturePixelRotation::Rotate90Clockwise => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            CapturePixelRotation::Rotate90Clockwise,
        ),
        CapturePixelRotation::Rotate180 => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            CapturePixelRotation::Rotate180,
        ),
        CapturePixelRotation::Rotate90Counterclockwise => rotate_xrgb8888(
            src_pixels,
            src_width,
            src_height,
            CapturePixelRotation::Rotate90Counterclockwise,
        ),
    }
}

fn rotate_xrgb8888(
    src_pixels: &[u8],
    src_width: usize,
    src_height: usize,
    transform: CapturePixelRotation,
) -> (usize, usize, Vec<u8>) {
    let (dst_width, dst_height) = match transform {
        CapturePixelRotation::Identity => (src_width, src_height),
        CapturePixelRotation::Rotate180 => (src_width, src_height),
        CapturePixelRotation::Rotate90Clockwise
        | CapturePixelRotation::Rotate90Counterclockwise => (src_height, src_width),
    };
    let mut dst = vec![0u8; dst_width.saturating_mul(dst_height).saturating_mul(4)];

    for y in 0..src_height {
        for x in 0..src_width {
            let (dst_x, dst_y) = match transform {
                CapturePixelRotation::Identity => (x, y),
                CapturePixelRotation::Rotate90Clockwise => {
                    (src_height.saturating_sub(1).saturating_sub(y), x)
                }
                CapturePixelRotation::Rotate180 => (
                    src_width.saturating_sub(1).saturating_sub(x),
                    src_height.saturating_sub(1).saturating_sub(y),
                ),
                CapturePixelRotation::Rotate90Counterclockwise => {
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
    use crate::root_geometry::RootGeometryAuthority;
    use serde_json::json;

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
    fn root4_capture_keeps_raw_scanout_raster_and_pinned_generation() {
        let mut authority = RootGeometryAuthority::from_config(Some(
            &json!({"root4": {"display_scale_factor": 1.0}}),
        ))
        .unwrap();
        let snapshot = authority.initialize(4, 3, OutputRotation::Deg90).unwrap();
        let store = ScreenCaptureStore::default();
        store.update_root4_scanout_xrgb8888(&[0x20; 4 * 3 * 4], 4 * 4, 4, 3, false, snapshot, &[]);
        let path = std::env::temp_dir().join(format!(
            "surf-ace-root4-raw-capture-{}.png",
            std::process::id()
        ));
        let geometry = store
            .write_root4_png(
                &path,
                LogicalRect {
                    x: 0.0,
                    y: 0.0,
                    width: 3.0,
                    height: 4.0,
                },
            )
            .unwrap();
        assert_eq!(
            (
                geometry.physical_pixel_rect.width,
                geometry.physical_pixel_rect.height
            ),
            (4, 3)
        );
        assert_eq!(geometry.root_geometry_generation, 1);
        let png = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn root4_transaction_requires_matching_staged_capture_frame() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        let snapshot = authority.initialize(4, 4, OutputRotation::Deg0).unwrap();
        let store = ScreenCaptureStore::default();
        store.begin_root4_stage(snapshot.generation);
        assert!(!store.root4_stage_has_frame(snapshot.generation));
        store.update_root4_scanout_xrgb8888(&[0x20; 4 * 4 * 4], 16, 4, 4, false, snapshot, &[]);
        assert!(store.root4_stage_has_frame(snapshot.generation));
        store.discard_root4_stage(snapshot.generation);
        assert!(!store.root4_stage_has_frame(snapshot.generation));
    }

    #[test]
    fn deg180_capture_rotates_scanout_180_degrees() {
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        let (width, height, rotated) = rotate_scanout_to_view(&src, 2, 2, OutputRotation::Deg180);
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            rotated,
            vec![
                0x04, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, //
                0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn deg270_capture_rotates_landscape_scanout_into_portrait_view() {
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        let (width, height, rotated) = rotate_scanout_to_view(&src, 2, 2, OutputRotation::Deg270);
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            rotated,
            vec![
                0x02, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, //
                0x01, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
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
    fn deg90_capture_ignores_renderer_row_flip_before_rotation() {
        let store = ScreenCaptureStore::default();
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        let src_flipped = OutputRotationModel::new(OutputRotation::Deg90).capture_src_flipped(true);
        store.update_from_scanout_xrgb8888(&src, 8, 2, 2, src_flipped, OutputRotation::Deg90);

        let frame = store
            .inner
            .lock()
            .expect("capture frame should lock")
            .clone()
            .committed
            .expect("capture frame should exist");
        let (width, height, xrgb8888) = frame.view_xrgb8888();
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            xrgb8888,
            vec![
                0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, //
                0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
            ]
        );
    }
}
