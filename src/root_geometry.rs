use crate::model::OutputRotation;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_DISPLAY_SCALE_FACTOR: f64 = 2.0;
pub const MIN_DISPLAY_SCALE_FACTOR: f64 = 1.0;
pub const MAX_DISPLAY_SCALE_FACTOR: f64 = 4.0;
pub const MAX_ROOT_GEOMETRY_GENERATION: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayScaleSource {
    Default,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootSizeI32 {
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootSizeF64 {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayScaleStatus {
    pub feature: String,
    pub factor: f64,
    pub percent: f64,
    pub source: DisplayScaleSource,
    pub physical_size_px: RootSizeI32,
    pub logical_size: RootSizeF64,
    pub rotation: OutputRotation,
    pub root_geometry_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootGeometrySnapshot {
    pub physical_size_px: RootSizeI32,
    pub logical_size: RootSizeF64,
    pub factor: f64,
    pub source: DisplayScaleSource,
    pub rotation: OutputRotation,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalPixelRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeBufferProjection {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width_px: i32,
    pub height_px: i32,
    pub fractional_phase_x: f64,
    pub fractional_phase_y: f64,
    pub logical_clip: LogicalRect,
    pub scale_factor: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportProjection {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub coordinate_space: String,
    pub display_scale_factor: f64,
    pub rotation: OutputRotation,
    pub root_geometry_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootLayoutProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompositedContentProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeMaterializationProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportSetProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureSetProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputTransformProjection(pub RootGeometrySnapshot);
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayScaleProjection(pub RootGeometrySnapshot);

/// The only publishable root4 geometry value. Each member is staged in the
/// normative order and the complete value is swapped as one generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommittedRootGeometry {
    pub snapshot: RootGeometrySnapshot,
    pub root_layout: RootLayoutProjection,
    pub composited_content: CompositedContentProjection,
    pub native_materialization: NativeMaterializationProjection,
    pub viewports: ViewportSetProjection,
    pub captures: CaptureSetProjection,
    pub input: InputTransformProjection,
    pub status: DisplayScaleProjection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureGeometry {
    pub logical_rect: LogicalRect,
    pub physical_pixel_rect: PhysicalPixelRect,
    pub coordinate_space: String,
    pub physical_coordinate_space: String,
    pub display_scale_factor: f64,
    pub rotation: OutputRotation,
    pub root_geometry_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootGeometryError {
    InvalidDisplayScale,
    DisplayScaleApplyFailed,
    RootGeometryGenerationExhausted,
}

impl RootGeometryError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidDisplayScale => "invalid_display_scale",
            Self::DisplayScaleApplyFailed => "display_scale_apply_failed",
            Self::RootGeometryGenerationExhausted => "root_geometry_generation_exhausted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RootGeometryAuthority {
    startup_factor: f64,
    startup_source: DisplayScaleSource,
    committed: Option<CommittedRootGeometry>,
    #[cfg(test)]
    fail_staging_at: Option<usize>,
}

impl RootGeometryAuthority {
    pub fn from_config(config: Option<&Value>) -> Result<Self, RootGeometryError> {
        let nested = config
            .and_then(Value::as_object)
            .and_then(|root| root.get("root4"))
            .and_then(Value::as_object)
            .and_then(|root4| root4.get("display_scale_factor"));
        let (startup_factor, startup_source) = match nested {
            Some(value) => (validate_factor_value(value)?, DisplayScaleSource::Config),
            None => (DEFAULT_DISPLAY_SCALE_FACTOR, DisplayScaleSource::Default),
        };
        Ok(Self {
            startup_factor,
            startup_source,
            committed: None,
            #[cfg(test)]
            fail_staging_at: None,
        })
    }

    pub fn startup_factor(&self) -> f64 {
        self.startup_factor
    }

    pub fn committed(&self) -> Option<RootGeometrySnapshot> {
        self.committed.map(|committed| committed.snapshot)
    }

    pub fn committed_projections(&self) -> Option<CommittedRootGeometry> {
        self.committed
    }

    pub fn initialize(
        &mut self,
        physical_width: i32,
        physical_height: i32,
        rotation: OutputRotation,
    ) -> Result<RootGeometrySnapshot, RootGeometryError> {
        let staged = self.prepare_initialize(physical_width, physical_height, rotation)?;
        let snapshot = staged.snapshot;
        self.committed = Some(staged);
        Ok(snapshot)
    }

    pub fn prepare_initialize(
        &self,
        physical_width: i32,
        physical_height: i32,
        rotation: OutputRotation,
    ) -> Result<CommittedRootGeometry, RootGeometryError> {
        if self.committed.is_some() {
            return Err(RootGeometryError::DisplayScaleApplyFailed);
        }
        let snapshot = RootGeometrySnapshot::new(
            physical_width,
            physical_height,
            self.startup_factor,
            self.startup_source,
            rotation,
            1,
        )?;
        self.stage_all(snapshot)
    }

    pub fn set_scale(&mut self, factor: f64) -> Result<RootGeometrySnapshot, RootGeometryError> {
        self.set_scale_from_source(factor, DisplayScaleSource::Config)
    }

    pub fn set_scale_from_source(
        &mut self,
        factor: f64,
        source: DisplayScaleSource,
    ) -> Result<RootGeometrySnapshot, RootGeometryError> {
        validate_factor(factor)?;
        self.mutate(|base, generation| {
            RootGeometrySnapshot::new(
                base.physical_size_px.width,
                base.physical_size_px.height,
                factor,
                source,
                base.rotation,
                generation,
            )
        })
    }

    pub fn prepare_scale(
        &self,
        factor: f64,
        source: DisplayScaleSource,
    ) -> Result<CommittedRootGeometry, RootGeometryError> {
        validate_factor(factor)?;
        let base = self
            .committed
            .map(|committed| committed.snapshot)
            .ok_or(RootGeometryError::DisplayScaleApplyFailed)?;
        if base.generation == MAX_ROOT_GEOMETRY_GENERATION {
            return Err(RootGeometryError::RootGeometryGenerationExhausted);
        }
        let snapshot = RootGeometrySnapshot::new(
            base.physical_size_px.width,
            base.physical_size_px.height,
            factor,
            source,
            base.rotation,
            base.generation + 1,
        )?;
        self.stage_all(snapshot)
    }

    pub fn prepare_rotation(
        &self,
        rotation: OutputRotation,
    ) -> Result<CommittedRootGeometry, RootGeometryError> {
        let base = self
            .committed
            .map(|committed| committed.snapshot)
            .ok_or(RootGeometryError::DisplayScaleApplyFailed)?;
        if base.generation == MAX_ROOT_GEOMETRY_GENERATION {
            return Err(RootGeometryError::RootGeometryGenerationExhausted);
        }
        self.stage_all(RootGeometrySnapshot::new(
            base.physical_size_px.width,
            base.physical_size_px.height,
            base.factor,
            base.source,
            rotation,
            base.generation + 1,
        )?)
    }

    pub fn prepare_mode(
        &self,
        physical_width: i32,
        physical_height: i32,
    ) -> Result<CommittedRootGeometry, RootGeometryError> {
        let base = self
            .committed
            .map(|committed| committed.snapshot)
            .ok_or(RootGeometryError::DisplayScaleApplyFailed)?;
        if base.generation == MAX_ROOT_GEOMETRY_GENERATION {
            return Err(RootGeometryError::RootGeometryGenerationExhausted);
        }
        self.stage_all(RootGeometrySnapshot::new(
            physical_width,
            physical_height,
            base.factor,
            base.source,
            base.rotation,
            base.generation + 1,
        )?)
    }

    pub fn commit_prepared(&mut self, prepared: CommittedRootGeometry) {
        self.committed = Some(prepared);
    }

    pub fn set_mode(
        &mut self,
        physical_width: i32,
        physical_height: i32,
    ) -> Result<RootGeometrySnapshot, RootGeometryError> {
        if self.committed.is_none() {
            return self.initialize(physical_width, physical_height, OutputRotation::Deg0);
        }
        self.mutate(|base, generation| {
            RootGeometrySnapshot::new(
                physical_width,
                physical_height,
                base.factor,
                base.source,
                base.rotation,
                generation,
            )
        })
    }

    pub fn set_rotation(
        &mut self,
        rotation: OutputRotation,
    ) -> Result<RootGeometrySnapshot, RootGeometryError> {
        self.mutate(|base, generation| {
            RootGeometrySnapshot::new(
                base.physical_size_px.width,
                base.physical_size_px.height,
                base.factor,
                base.source,
                rotation,
                generation,
            )
        })
    }

    fn mutate(
        &mut self,
        build: impl FnOnce(RootGeometrySnapshot, u64) -> Result<RootGeometrySnapshot, RootGeometryError>,
    ) -> Result<RootGeometrySnapshot, RootGeometryError> {
        let base = self
            .committed
            .map(|committed| committed.snapshot)
            .ok_or(RootGeometryError::DisplayScaleApplyFailed)?;
        if base.generation == MAX_ROOT_GEOMETRY_GENERATION {
            return Err(RootGeometryError::RootGeometryGenerationExhausted);
        }
        let staged = build(base, base.generation + 1)?;
        let projections = self.stage_all(staged)?;
        self.committed = Some(projections);
        Ok(staged)
    }

    fn stage_all(
        &self,
        snapshot: RootGeometrySnapshot,
    ) -> Result<CommittedRootGeometry, RootGeometryError> {
        validate_snapshot(snapshot)?;
        macro_rules! stage {
            ($position:literal, $value:expr) => {{
                #[cfg(test)]
                if self.fail_staging_at == Some($position) {
                    return Err(RootGeometryError::DisplayScaleApplyFailed);
                }
                $value
            }};
        }
        let root_layout = stage!(1, RootLayoutProjection(snapshot));
        let composited_content = stage!(2, CompositedContentProjection(snapshot));
        let native_materialization = stage!(3, NativeMaterializationProjection(snapshot));
        let viewports = stage!(4, ViewportSetProjection(snapshot));
        let captures = stage!(5, CaptureSetProjection(snapshot));
        let input = stage!(6, InputTransformProjection(snapshot));
        let status = stage!(7, DisplayScaleProjection(snapshot));
        Ok(CommittedRootGeometry {
            snapshot,
            root_layout,
            composited_content,
            native_materialization,
            viewports,
            captures,
            input,
            status,
        })
    }

    #[cfg(test)]
    fn fail_staging_at(&mut self, position: Option<usize>) {
        self.fail_staging_at = position;
    }

    #[cfg(test)]
    fn set_generation_for_test(&mut self, generation: u64) {
        self.committed
            .as_mut()
            .expect("initialized geometry")
            .snapshot
            .generation = generation;
    }
}

impl RootGeometrySnapshot {
    fn new(
        physical_width: i32,
        physical_height: i32,
        factor: f64,
        source: DisplayScaleSource,
        rotation: OutputRotation,
        generation: u64,
    ) -> Result<Self, RootGeometryError> {
        if physical_width <= 0 || physical_height <= 0 {
            return Err(RootGeometryError::DisplayScaleApplyFailed);
        }
        let physical_size_px = RootSizeI32 {
            width: physical_width,
            height: physical_height,
        };
        let (oriented_width, oriented_height) = match rotation {
            OutputRotation::Deg0 | OutputRotation::Deg180 => (
                physical_size_px.width as f64,
                physical_size_px.height as f64,
            ),
            OutputRotation::Deg90 | OutputRotation::Deg270 => (
                physical_size_px.height as f64,
                physical_size_px.width as f64,
            ),
        };
        Ok(Self {
            physical_size_px,
            logical_size: RootSizeF64 {
                width: oriented_width / factor,
                height: oriented_height / factor,
            },
            factor,
            source,
            rotation,
            generation,
        })
    }

    pub fn status(self) -> DisplayScaleStatus {
        DisplayScaleStatus {
            feature: "root4.display_scale.v1".to_string(),
            factor: self.factor,
            percent: self.factor * 100.0,
            source: self.source,
            physical_size_px: self.physical_size_px,
            logical_size: self.logical_size,
            rotation: self.rotation,
            root_geometry_generation: self.generation,
        }
    }

    pub fn logical_size_i32(self) -> RootSizeI32 {
        RootSizeI32 {
            width: self.logical_size.width.floor().max(1.0) as i32,
            height: self.logical_size.height.floor().max(1.0) as i32,
        }
    }

    pub fn oriented_physical_size_i32(self) -> RootSizeI32 {
        match self.rotation {
            OutputRotation::Deg0 | OutputRotation::Deg180 => self.physical_size_px,
            OutputRotation::Deg90 | OutputRotation::Deg270 => RootSizeI32 {
                width: self.physical_size_px.height,
                height: self.physical_size_px.width,
            },
        }
    }

    pub fn physical_to_logical(self, px: f64, py: f64) -> (f64, f64) {
        let width = self.physical_size_px.width as f64;
        let height = self.physical_size_px.height as f64;
        let (x, y) = match self.rotation {
            OutputRotation::Deg0 => (px / self.factor, py / self.factor),
            OutputRotation::Deg90 => (py / self.factor, (width - px) / self.factor),
            OutputRotation::Deg180 => ((width - px) / self.factor, (height - py) / self.factor),
            OutputRotation::Deg270 => ((height - py) / self.factor, px / self.factor),
        };
        (
            clamp_half_open(x, self.logical_size.width),
            clamp_half_open(y, self.logical_size.height),
        )
    }

    pub fn physical_crop(self, rect: LogicalRect) -> PhysicalPixelRect {
        let corners = [
            self.logical_to_physical(rect.x, rect.y),
            self.logical_to_physical(rect.x + rect.width, rect.y),
            self.logical_to_physical(rect.x, rect.y + rect.height),
            self.logical_to_physical(rect.x + rect.width, rect.y + rect.height),
        ];
        let min_x = corners
            .iter()
            .map(|point| point.0)
            .fold(f64::INFINITY, f64::min);
        let min_y = corners
            .iter()
            .map(|point| point.1)
            .fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|point| point.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = corners
            .iter()
            .map(|point| point.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let x0 = min_x.floor().clamp(0.0, self.physical_size_px.width as f64) as i32;
        let y0 = min_y
            .floor()
            .clamp(0.0, self.physical_size_px.height as f64) as i32;
        let x1 = max_x.ceil().clamp(0.0, self.physical_size_px.width as f64) as i32;
        let y1 = max_y.ceil().clamp(0.0, self.physical_size_px.height as f64) as i32;
        PhysicalPixelRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    pub fn native_buffer(self, rect: LogicalRect) -> NativeBufferProjection {
        let scaled_x = rect.x * self.factor;
        let scaled_y = rect.y * self.factor;
        let origin_x = scaled_x.floor() as i32;
        let origin_y = scaled_y.floor() as i32;
        NativeBufferProjection {
            origin_x,
            origin_y,
            width_px: ((rect.x + rect.width) * self.factor).ceil() as i32 - origin_x,
            height_px: ((rect.y + rect.height) * self.factor).ceil() as i32 - origin_y,
            fractional_phase_x: scaled_x - origin_x as f64,
            fractional_phase_y: scaled_y - origin_y as f64,
            logical_clip: rect,
            scale_factor: self.factor,
        }
    }

    pub fn viewport(self, rect: LogicalRect) -> ViewportProjection {
        ViewportProjection {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            coordinate_space: "root4_oriented_logical".to_string(),
            display_scale_factor: self.factor,
            rotation: self.rotation,
            root_geometry_generation: self.generation,
        }
    }

    pub fn capture_geometry(self, logical_rect: LogicalRect) -> CaptureGeometry {
        CaptureGeometry {
            logical_rect,
            physical_pixel_rect: self.physical_crop(logical_rect),
            coordinate_space: "root4_oriented_logical".to_string(),
            physical_coordinate_space: "root4_raw_scanout_px".to_string(),
            display_scale_factor: self.factor,
            rotation: self.rotation,
            root_geometry_generation: self.generation,
        }
    }

    pub fn logical_to_physical(self, ox: f64, oy: f64) -> (f64, f64) {
        let width = self.physical_size_px.width as f64;
        let height = self.physical_size_px.height as f64;
        match self.rotation {
            OutputRotation::Deg0 => (ox * self.factor, oy * self.factor),
            OutputRotation::Deg90 => (width - oy * self.factor, ox * self.factor),
            OutputRotation::Deg180 => (width - ox * self.factor, height - oy * self.factor),
            OutputRotation::Deg270 => (oy * self.factor, height - ox * self.factor),
        }
    }
}

pub fn validate_factor_value(value: &Value) -> Result<f64, RootGeometryError> {
    value
        .as_f64()
        .ok_or(RootGeometryError::InvalidDisplayScale)
        .and_then(validate_factor)
}

pub fn validate_factor(factor: f64) -> Result<f64, RootGeometryError> {
    if (MIN_DISPLAY_SCALE_FACTOR..=MAX_DISPLAY_SCALE_FACTOR).contains(&factor) {
        Ok(factor)
    } else {
        Err(RootGeometryError::InvalidDisplayScale)
    }
}

fn clamp_half_open(value: f64, length: f64) -> f64 {
    value.max(0.0).min(next_down(length))
}

fn next_down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
}

fn validate_snapshot(snapshot: RootGeometrySnapshot) -> Result<(), RootGeometryError> {
    let valid = snapshot.factor >= MIN_DISPLAY_SCALE_FACTOR
        && snapshot.factor <= MAX_DISPLAY_SCALE_FACTOR
        && snapshot.physical_size_px.width > 0
        && snapshot.physical_size_px.height > 0
        && snapshot.logical_size.width > 0.0
        && snapshot.logical_size.height > 0.0;
    if valid {
        Ok(())
    } else {
        Err(RootGeometryError::DisplayScaleApplyFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(rotation: OutputRotation) -> RootGeometrySnapshot {
        RootGeometrySnapshot::new(3840, 2160, 1.3, DisplayScaleSource::Config, rotation, 9).unwrap()
    }

    #[test]
    fn config_resolution_is_nested_only_and_defaults_to_two() {
        let aliases = json!({
            "root4.display_scale_factor": 4.0,
            "root4": { "scale": 4.0, "display_scale": 4.0 }
        });
        let authority = RootGeometryAuthority::from_config(Some(&aliases)).unwrap();
        assert_eq!(authority.startup_factor(), 2.0);
        let nested = json!({
            "root4.display_scale_factor": 4.0,
            "root4": { "display_scale_factor": 1.5, "scale": 4.0 }
        });
        let authority = RootGeometryAuthority::from_config(Some(&nested)).unwrap();
        assert_eq!(authority.startup_factor(), 1.5);
        assert_eq!(authority.startup_source, DisplayScaleSource::Config);
    }

    #[test]
    fn invalid_config_types_and_bounds_are_rejected_without_fallback() {
        for value in [
            json!(0.999),
            json!(4.001),
            json!("2"),
            json!(null),
            json!(true),
            json!([]),
            json!({}),
        ] {
            let config = json!({ "root4": { "display_scale_factor": value } });
            assert_eq!(
                RootGeometryAuthority::from_config(Some(&config)).unwrap_err(),
                RootGeometryError::InvalidDisplayScale
            );
        }
        for factor in [1.0, 4.0] {
            let config = json!({ "root4": { "display_scale_factor": factor } });
            assert!(RootGeometryAuthority::from_config(Some(&config)).is_ok());
        }
    }

    #[test]
    fn status_uses_one_committed_snapshot() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        let status = authority
            .initialize(3840, 2160, OutputRotation::Deg0)
            .unwrap()
            .status();
        assert_eq!(status.feature, "root4.display_scale.v1");
        assert_eq!(status.factor, 2.0);
        assert_eq!(status.percent, 200.0);
        assert_eq!(status.source, DisplayScaleSource::Default);
        assert_eq!(
            status.logical_size,
            RootSizeF64 {
                width: 1920.0,
                height: 1080.0
            }
        );
        assert_eq!(status.root_geometry_generation, 1);
    }

    #[test]
    fn same_factor_config_edit_commits_config_source_and_increments_once() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        authority
            .initialize(3840, 2160, OutputRotation::Deg0)
            .unwrap();
        let snapshot = authority.set_scale(2.0).unwrap();
        assert_eq!(snapshot.source, DisplayScaleSource::Config);
        assert_eq!(snapshot.generation, 2);
    }

    #[test]
    fn restart_reestablishes_generation_one_from_selected_config() {
        for factor in [1.0, 2.0, 4.0] {
            let config = json!({ "root4": { "display_scale_factor": factor } });
            for _ in 0..2 {
                let mut authority = RootGeometryAuthority::from_config(Some(&config)).unwrap();
                let snapshot = authority
                    .initialize(3840, 2160, OutputRotation::Deg0)
                    .unwrap();
                assert_eq!(snapshot.factor, factor);
                assert_eq!(snapshot.generation, 1);
                assert_eq!(snapshot.source, DisplayScaleSource::Config);
            }
        }
    }

    #[test]
    fn every_staging_failure_rolls_back_the_entire_snapshot() {
        for position in 1..=7 {
            let mut authority = RootGeometryAuthority::from_config(None).unwrap();
            let before = authority
                .initialize(3840, 2160, OutputRotation::Deg0)
                .unwrap();
            let before_projections = authority.committed_projections().unwrap();
            authority.fail_staging_at(Some(position));
            assert_eq!(
                authority.set_scale(1.5),
                Err(RootGeometryError::DisplayScaleApplyFailed)
            );
            assert_eq!(authority.committed(), Some(before));
            assert_eq!(authority.committed_projections(), Some(before_projections));
        }
    }

    #[test]
    fn committed_bundle_tags_all_seven_consumers_with_one_generation() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        authority
            .initialize(3840, 2160, OutputRotation::Deg0)
            .unwrap();
        let committed = authority.committed_projections().unwrap();
        assert_eq!(committed.root_layout.0.generation, 1);
        assert_eq!(committed.composited_content.0.generation, 1);
        assert_eq!(committed.native_materialization.0.generation, 1);
        assert_eq!(committed.viewports.0.generation, 1);
        assert_eq!(committed.captures.0.generation, 1);
        assert_eq!(committed.input.0.generation, 1);
        assert_eq!(committed.status.0.generation, 1);
    }

    #[test]
    fn invalid_physical_dimensions_fail_without_clamping_or_publication() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        assert_eq!(
            authority.initialize(0, 2160, OutputRotation::Deg0),
            Err(RootGeometryError::DisplayScaleApplyFailed)
        );
        assert!(authority.committed_projections().is_none());
    }

    #[test]
    fn generation_exhaustion_is_checked_before_staging() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        let before = authority
            .initialize(3840, 2160, OutputRotation::Deg0)
            .unwrap();
        authority.set_generation_for_test(MAX_ROOT_GEOMETRY_GENERATION);
        let exhausted = authority.committed().unwrap();
        assert_eq!(
            authority.set_scale(1.5),
            Err(RootGeometryError::RootGeometryGenerationExhausted)
        );
        assert_eq!(authority.committed(), Some(exhausted));
        assert_ne!(before.generation, exhausted.generation);
    }

    #[test]
    fn serialized_factor_mode_rotation_mutations_derive_from_last_commit() {
        let mut authority = RootGeometryAuthority::from_config(None).unwrap();
        authority
            .initialize(3840, 2160, OutputRotation::Deg0)
            .unwrap();
        let scaled = authority.set_scale(1.5).unwrap();
        assert_eq!((scaled.factor, scaled.generation), (1.5, 2));

        authority.fail_staging_at(Some(4));
        assert_eq!(
            authority.set_mode(2560, 1440),
            Err(RootGeometryError::DisplayScaleApplyFailed)
        );
        assert_eq!(authority.committed(), Some(scaled));

        authority.fail_staging_at(None);
        let rotated = authority.set_rotation(OutputRotation::Deg90).unwrap();
        assert_eq!(rotated.generation, 3);
        assert_eq!(rotated.factor, 1.5);
        assert_eq!(rotated.physical_size_px, scaled.physical_size_px);
        assert_eq!(rotated.rotation, OutputRotation::Deg90);
    }

    #[test]
    fn fractional_geometry_matches_all_rotation_goldens() {
        let rect = LogicalRect {
            x: 10.25,
            y: 20.5,
            width: 100.25,
            height: 50.75,
        };
        let expected = [
            (
                OutputRotation::Deg0,
                PhysicalPixelRect {
                    x: 13,
                    y: 26,
                    width: 131,
                    height: 67,
                },
            ),
            (
                OutputRotation::Deg90,
                PhysicalPixelRect {
                    x: 3747,
                    y: 13,
                    width: 67,
                    height: 131,
                },
            ),
            (
                OutputRotation::Deg180,
                PhysicalPixelRect {
                    x: 3696,
                    y: 2067,
                    width: 131,
                    height: 67,
                },
            ),
            (
                OutputRotation::Deg270,
                PhysicalPixelRect {
                    x: 26,
                    y: 2016,
                    width: 67,
                    height: 131,
                },
            ),
        ];
        for (rotation, crop) in expected {
            assert_eq!(snapshot(rotation).physical_crop(rect), crop);
        }
        let buffer = snapshot(OutputRotation::Deg0).native_buffer(rect);
        assert_eq!((buffer.origin_x, buffer.origin_y), (13, 26));
        assert_eq!((buffer.width_px, buffer.height_px), (131, 67));
        assert!((buffer.fractional_phase_x - 0.325).abs() < 1e-12);
        assert!((buffer.fractional_phase_y - 0.65).abs() < 1e-12);
    }

    #[test]
    fn input_transform_applies_scale_and_rotation_once() {
        let expected = [
            (OutputRotation::Deg0, (10.384615384615, 20.384615384615)),
            (OutputRotation::Deg90, (20.384615384615, 2943.461538461538)),
            (
                OutputRotation::Deg180,
                (2943.461538461538, 1641.153846153846),
            ),
            (OutputRotation::Deg270, (1641.153846153846, 10.384615384615)),
        ];
        for (rotation, golden) in expected {
            let actual = snapshot(rotation).physical_to_logical(13.5, 26.5);
            assert!((actual.0 - golden.0).abs() < 1e-9);
            assert!((actual.1 - golden.1).abs() < 1e-9);
        }
    }

    #[test]
    fn adjacent_fractional_crops_have_no_gap_and_at_most_one_pixel_overlap() {
        let geometry = snapshot(OutputRotation::Deg0);
        let left = geometry.physical_crop(LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 10.25,
            height: 5.0,
        });
        let right = geometry.physical_crop(LogicalRect {
            x: 10.25,
            y: 0.0,
            width: 10.25,
            height: 5.0,
        });
        let left_edge = left.x + left.width;
        let overlap = left_edge - right.x;
        assert!((0..=1).contains(&overlap));
    }
}
