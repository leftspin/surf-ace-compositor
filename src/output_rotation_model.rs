use crate::model::OutputRotation;
use smithay::utils::Transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePixelRotation {
    Identity,
    Rotate90Clockwise,
    Rotate180,
    Rotate90Counterclockwise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRotationModel {
    rotation: OutputRotation,
}

impl OutputRotationModel {
    pub const fn new(rotation: OutputRotation) -> Self {
        Self { rotation }
    }

    pub const fn rotation(self) -> OutputRotation {
        self.rotation
    }

    pub const fn swaps_axes(self) -> bool {
        matches!(
            self.rotation,
            OutputRotation::Deg90 | OutputRotation::Deg270
        )
    }

    pub const fn scene_texture_transform(self) -> Transform {
        match self.rotation {
            OutputRotation::Deg0 => Transform::Normal,
            // Texture composition uses Smithay texture-space transforms rather than
            // output-space transforms, so quarter-turns keep the flipped variants.
            OutputRotation::Deg90 => Transform::Flipped90,
            OutputRotation::Deg180 => Transform::_180,
            OutputRotation::Deg270 => Transform::Flipped270,
        }
    }

    pub const fn output_transform(self) -> Transform {
        match self.rotation {
            OutputRotation::Deg0 => Transform::Normal,
            OutputRotation::Deg90 => Transform::_270,
            OutputRotation::Deg180 => Transform::_180,
            OutputRotation::Deg270 => Transform::_90,
        }
    }

    pub const fn present_preserves_readback_row_order(self) -> bool {
        false
    }

    pub const fn present_unflip_rows(self) -> bool {
        !self.present_preserves_readback_row_order()
    }

    pub const fn capture_src_flipped(self, mapping_flipped: bool) -> bool {
        match self.rotation {
            OutputRotation::Deg90 | OutputRotation::Deg270 => mapping_flipped,
            OutputRotation::Deg0 | OutputRotation::Deg180 => false,
        }
    }

    pub const fn capture_pixel_rotation(self) -> CapturePixelRotation {
        match self.rotation {
            OutputRotation::Deg0 => CapturePixelRotation::Identity,
            OutputRotation::Deg90 => CapturePixelRotation::Rotate90Clockwise,
            OutputRotation::Deg180 => CapturePixelRotation::Rotate180,
            OutputRotation::Deg270 => CapturePixelRotation::Rotate90Counterclockwise,
        }
    }

    pub const fn logical_size_i32(self, width: i32, height: i32) -> (i32, i32) {
        if self.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CapturePixelRotation, OutputRotationModel};
    use crate::model::OutputRotation;
    use smithay::utils::Transform;

    #[test]
    fn model_derives_rotation_table() {
        let deg0 = OutputRotationModel::new(OutputRotation::Deg0);
        assert_eq!(deg0.rotation(), OutputRotation::Deg0);
        assert!(!deg0.swaps_axes());
        assert_eq!(deg0.logical_size_i32(3840, 2160), (3840, 2160));
        assert_eq!(deg0.scene_texture_transform(), Transform::Normal);
        assert_eq!(deg0.output_transform(), Transform::Normal);
        assert!(deg0.present_unflip_rows());
        assert_eq!(
            deg0.capture_pixel_rotation(),
            CapturePixelRotation::Identity
        );
        assert!(!deg0.capture_src_flipped(true));

        let deg90 = OutputRotationModel::new(OutputRotation::Deg90);
        assert!(deg90.swaps_axes());
        assert_eq!(deg90.logical_size_i32(3840, 2160), (2160, 3840));
        assert_eq!(deg90.scene_texture_transform(), Transform::Flipped90);
        assert_eq!(deg90.output_transform(), Transform::_270);
        assert!(deg90.present_unflip_rows());
        assert_eq!(
            deg90.capture_pixel_rotation(),
            CapturePixelRotation::Rotate90Clockwise
        );
        assert!(deg90.capture_src_flipped(true));

        let deg180 = OutputRotationModel::new(OutputRotation::Deg180);
        assert!(!deg180.swaps_axes());
        assert_eq!(deg180.logical_size_i32(3840, 2160), (3840, 2160));
        assert_eq!(deg180.scene_texture_transform(), Transform::_180);
        assert_eq!(deg180.output_transform(), Transform::_180);
        assert!(deg180.present_unflip_rows());
        assert_eq!(
            deg180.capture_pixel_rotation(),
            CapturePixelRotation::Rotate180
        );
        assert!(!deg180.capture_src_flipped(true));

        let deg270 = OutputRotationModel::new(OutputRotation::Deg270);
        assert!(deg270.swaps_axes());
        assert_eq!(deg270.logical_size_i32(3840, 2160), (2160, 3840));
        assert_eq!(deg270.scene_texture_transform(), Transform::Flipped270);
        assert_eq!(deg270.output_transform(), Transform::_90);
        assert!(deg270.present_unflip_rows());
        assert_eq!(
            deg270.capture_pixel_rotation(),
            CapturePixelRotation::Rotate90Counterclockwise
        );
        assert!(deg270.capture_src_flipped(true));
    }

    #[test]
    fn capture_src_flip_uses_verified_rotation_policy() {
        assert!(!OutputRotationModel::new(OutputRotation::Deg0).capture_src_flipped(true));
        assert!(OutputRotationModel::new(OutputRotation::Deg90).capture_src_flipped(true));
        assert!(!OutputRotationModel::new(OutputRotation::Deg180).capture_src_flipped(true));
        assert!(OutputRotationModel::new(OutputRotation::Deg270).capture_src_flipped(true));

        for rotation in [
            OutputRotation::Deg0,
            OutputRotation::Deg90,
            OutputRotation::Deg180,
            OutputRotation::Deg270,
        ] {
            assert!(!OutputRotationModel::new(rotation).capture_src_flipped(false));
        }
    }

    #[test]
    fn quarter_turn_scene_texture_transform_stays_distinct_from_output_transform() {
        let deg90 = OutputRotationModel::new(OutputRotation::Deg90);
        assert_eq!(deg90.scene_texture_transform(), Transform::Flipped90);
        assert_eq!(deg90.output_transform(), Transform::_270);
        assert_ne!(deg90.scene_texture_transform(), deg90.output_transform());

        let deg270 = OutputRotationModel::new(OutputRotation::Deg270);
        assert_eq!(deg270.scene_texture_transform(), Transform::Flipped270);
        assert_eq!(deg270.output_transform(), Transform::_90);
        assert_ne!(deg270.scene_texture_transform(), deg270.output_transform());
    }
}
