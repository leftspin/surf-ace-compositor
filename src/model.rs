use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(pub String);

impl PaneId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTargetClass {
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneRenderMode {
    SurfAceRendered,
    ExternalNative {
        target: NativeTargetClass,
        process: ProcessSpec,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ExternalNativeLifecycleState {
    Absent,
    Launching { pid: u32 },
    Attached { pid: u32 },
    Failed { reason: String },
    Exited { pid: u32, exit_code: Option<i32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventFamily {
    KeyboardInput,
    PointerInput,
    TextSelection,
    PaneAnnotations,
    HtmlDomEvents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalNativeEventContract {
    pub supported: Vec<EventFamily>,
    pub adapted: Vec<EventFamily>,
    pub suppressed: Vec<EventFamily>,
}

impl ExternalNativeEventContract {
    pub fn terminal_v1() -> Self {
        Self {
            supported: vec![EventFamily::KeyboardInput, EventFamily::PointerInput],
            adapted: vec![EventFamily::TextSelection],
            suppressed: vec![EventFamily::PaneAnnotations, EventFamily::HtmlDomEvents],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderPaneSnapshot {
    pub id: PaneId,
    pub geometry: PaneGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneStatus {
    pub id: PaneId,
    pub geometry: PaneGeometry,
    pub render_mode: PaneRenderMode,
    pub external_native_state: ExternalNativeLifecycleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_native_event_contract: Option<ExternalNativeEventContract>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrototypePolicyStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_overlay_pane: Option<PaneId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    #[default]
    None,
    Winit,
    HostDrm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    #[default]
    Inactive,
    Starting,
    PreflightReady,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFocusTarget {
    MainApp,
    OverlayNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRuntimeStartTrigger {
    Bootstrap,
    ControlRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHostPresentOwnership {
    #[default]
    None,
    Dumb,
    DirectGbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHostQueuedPresentSource {
    #[default]
    None,
    Dumb,
    DirectGbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDmabufFormatStatus {
    pub code: u32,
    pub modifier: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub backend: RuntimeBackend,
    pub phase: RuntimePhase,
    pub main_app_match_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayland_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<i32>,
    pub redraw_count: u64,
    pub input_event_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_seat_name: Option<String>,
    pub host_detected_drm_device_count: u32,
    pub host_opened_drm_device_count: u32,
    pub host_output_ownership: bool,
    pub host_start_attempt_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_start_trigger: Option<HostRuntimeStartTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_primary_drm_path: Option<String>,
    pub host_present_ownership: RuntimeHostPresentOwnership,
    pub host_atomic_commit_enabled: bool,
    pub host_overlay_plane_capable: bool,
    pub host_last_queued_present_source: RuntimeHostQueuedPresentSource,
    pub host_last_queued_atomic_commit: bool,
    pub host_last_queued_overlay_plane: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_queued_primary_dmabuf_format: Option<RuntimeDmabufFormatStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_queued_overlay_dmabuf_format: Option<RuntimeDmabufFormatStatus>,
    pub dmabuf_protocol_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dmabuf_protocol_formats: Vec<RuntimeDmabufFormatStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_app_surface_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_surface_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_bound_pane_id: Option<PaneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_focus_target: Option<RuntimeFocusTarget>,
    pub denied_toplevel_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::None,
            phase: RuntimePhase::Inactive,
            main_app_match_hint: "surf-ace".to_string(),
            wayland_socket: None,
            window_width: None,
            window_height: None,
            redraw_count: 0,
            input_event_count: 0,
            host_seat_name: None,
            host_detected_drm_device_count: 0,
            host_opened_drm_device_count: 0,
            host_output_ownership: false,
            host_start_attempt_count: 0,
            host_last_start_trigger: None,
            host_primary_drm_path: None,
            host_present_ownership: RuntimeHostPresentOwnership::None,
            host_atomic_commit_enabled: false,
            host_overlay_plane_capable: false,
            host_last_queued_present_source: RuntimeHostQueuedPresentSource::None,
            host_last_queued_atomic_commit: false,
            host_last_queued_overlay_plane: false,
            host_last_queued_primary_dmabuf_format: None,
            host_last_queued_overlay_dmabuf_format: None,
            dmabuf_protocol_enabled: false,
            dmabuf_protocol_formats: Vec::new(),
            main_app_surface_id: None,
            overlay_surface_id: None,
            overlay_bound_pane_id: None,
            active_focus_target: None,
            denied_toplevel_count: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub host_mode_active: bool,
    pub output_rotation: OutputRotation,
    pub panes: Vec<PaneStatus>,
    pub prototype_policy: PrototypePolicyStatus,
    pub runtime: RuntimeStatus,
}
