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
pub enum MainAppSurfaceBinding {
    AppId { app_id: String },
    Title { title: String },
    AppIdAndTitle { app_id: String, title: String },
}

impl MainAppSurfaceBinding {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::AppId { app_id } if app_id.trim().is_empty() => {
                Err("main app binding app_id must not be empty")
            }
            Self::Title { title } if title.trim().is_empty() => {
                Err("main app binding title must not be empty")
            }
            Self::AppIdAndTitle { app_id, .. } if app_id.trim().is_empty() => {
                Err("main app binding app_id must not be empty")
            }
            Self::AppIdAndTitle { title, .. } if title.trim().is_empty() => {
                Err("main app binding title must not be empty")
            }
            Self::AppId { .. } | Self::Title { .. } | Self::AppIdAndTitle { .. } => Ok(()),
        }
    }

    pub fn match_identity(
        &self,
        app_id: Option<&str>,
        title: Option<&str>,
    ) -> MainAppSurfaceBindingMatch {
        match self {
            Self::AppId { app_id: expected } => match app_id {
                Some(actual) if actual == expected => MainAppSurfaceBindingMatch::Match,
                Some(_) => MainAppSurfaceBindingMatch::Mismatch,
                None => MainAppSurfaceBindingMatch::Pending,
            },
            Self::Title { title: expected } => match title {
                Some(actual) if actual == expected => MainAppSurfaceBindingMatch::Match,
                Some(_) => MainAppSurfaceBindingMatch::Mismatch,
                None => MainAppSurfaceBindingMatch::Pending,
            },
            Self::AppIdAndTitle {
                app_id: expected_app_id,
                title: expected_title,
            } => {
                if app_id.is_some_and(|actual| actual != expected_app_id) {
                    return MainAppSurfaceBindingMatch::Mismatch;
                }
                if title.is_some_and(|actual| actual != expected_title) {
                    return MainAppSurfaceBindingMatch::Mismatch;
                }
                match (app_id, title) {
                    (Some(actual_app_id), Some(actual_title))
                        if actual_app_id == expected_app_id && actual_title == expected_title =>
                    {
                        MainAppSurfaceBindingMatch::Match
                    }
                    _ => MainAppSurfaceBindingMatch::Pending,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainAppSurfaceBindingMatch {
    Match,
    Pending,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceBindingEvidenceOutcome {
    MatchesIntent,
    MismatchesIntent,
    PendingIdentity,
    NotRequired,
}

impl From<MainAppSurfaceBindingMatch> for SurfaceBindingEvidenceOutcome {
    fn from(value: MainAppSurfaceBindingMatch) -> Self {
        match value {
            MainAppSurfaceBindingMatch::Match => Self::MatchesIntent,
            MainAppSurfaceBindingMatch::Pending => Self::PendingIdentity,
            MainAppSurfaceBindingMatch::Mismatch => Self::MismatchesIntent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchTokenEvidence {
    Matched,
    Mismatched,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceBindingEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        rename = "launchToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub launch_token: Option<LaunchTokenEvidence>,
    pub outcome: SurfaceBindingEvidenceOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainAppLaunchIntent {
    pub process: ProcessSpec,
    pub binding: MainAppSurfaceBinding,
}

impl MainAppLaunchIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.process.command.trim().is_empty() {
            return Err("main app process command must not be empty");
        }
        self.binding.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MainAppLaunchState {
    #[default]
    NotRequested,
    WaitingForRuntime,
    Launching {
        pid: u32,
    },
    Attached {
        pid: u32,
    },
    Failed {
        reason: String,
    },
    Exited {
        pid: u32,
        exit_code: Option<i32>,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OverlayRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositorOverlayKind {
    PaneBadge,
    HistoryBack,
    HistoryForward,
    PaneHandle,
    AnnotationControl,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayCaptureCapability {
    PointerHover,
    PointerButton,
    PointerAxis,
    Touch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayRegionUpdateReason {
    Initial,
    Layout,
    Resize,
    Visibility,
    Drag,
    Animation,
    NativeAttach,
    NativeDetach,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayCoordinateSpace {
    SurfaceLogical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayRegionRequest {
    pub region_id: String,
    pub pane_id: PaneId,
    pub pane_instance_id: String,
    pub kind: CompositorOverlayKind,
    pub rect: OverlayRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_index: Option<i32>,
    pub captures: Vec<OverlayCaptureCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayRegionStatus {
    pub region_id: String,
    pub pane_id: PaneId,
    pub pane_instance_id: String,
    pub kind: CompositorOverlayKind,
    pub rect: OverlayRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_index: Option<i32>,
    pub captures: Vec<OverlayCaptureCapability>,
    pub clamped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayRegionsStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_revision: Option<u64>,
    pub region_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<OverlayRegionUpdateReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<OverlayRegionStatus>,
}

impl Default for OverlayRegionsStatus {
    fn default() -> Self {
        Self {
            surface_id: None,
            window_id: None,
            active_revision: None,
            region_count: 0,
            topology_epoch: None,
            update_reason: None,
            last_updated_at: None,
            regions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePaneHostRequest {
    pub id: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default)]
    pub revision: u64,
    pub geometry: PaneGeometry,
    pub target: NativeTargetClass,
    pub process: ProcessSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePaneHostStatus {
    pub pane_id: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<u32>,
    pub lifecycle: ExternalNativeLifecycleState,
    pub process: ProcessSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_evidence: Option<SurfaceBindingEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneStatus {
    pub id: PaneId,
    pub geometry: PaneGeometry,
    pub render_mode: PaneRenderMode,
    pub external_native_state: ExternalNativeLifecycleState,
    #[serde(
        rename = "nativeHost",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub native_host: Option<NativePaneHostStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_native_binding_evidence: Option<SurfaceBindingEvidence>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFocusTarget {
    MainApp,
    OverlayNative,
    NativePane { pane_id: PaneId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostRuntimeStartTrigger {
    Bootstrap,
    ControlRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSelectionMode {
    #[default]
    Automatic,
    Forced,
    FallbackAfterFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHostSelectionState {
    #[default]
    Automatic,
    Forced,
    ForcedFailed,
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
    pub runtime_selection_mode: RuntimeSelectionMode,
    pub runtime_operator_action_needed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_operator_action_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_last_selection_attempt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_last_selection_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_app_launch_intent: Option<MainAppLaunchIntent>,
    pub main_app_launch_state: MainAppLaunchState,
    #[serde(skip)]
    pub main_app_launch_token: Option<String>,
    pub shell_overlay_toggle_shortcut: String,
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
    pub host_start_request_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_start_trigger: Option<HostRuntimeStartTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_primary_drm_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_forced_drm_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_forced_output_name: Option<String>,
    pub host_device_selection_state: RuntimeHostSelectionState,
    pub host_output_selection_state: RuntimeHostSelectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_active_connector_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_active_connector_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_selection_attempt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_last_selection_result: Option<String>,
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
    pub main_app_binding_evidence: Option<SurfaceBindingEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_surface_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_bound_pane_id: Option<PaneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_focus_target: Option<RuntimeFocusTarget>,
    #[serde(default)]
    pub overlay_region_debug_borders: bool,
    pub denied_toplevel_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::None,
            phase: RuntimePhase::Inactive,
            runtime_selection_mode: RuntimeSelectionMode::Automatic,
            runtime_operator_action_needed: false,
            runtime_operator_action_reason: None,
            runtime_last_selection_attempt: None,
            runtime_last_selection_result: None,
            main_app_launch_intent: None,
            main_app_launch_state: MainAppLaunchState::NotRequested,
            main_app_launch_token: None,
            shell_overlay_toggle_shortcut: "Super+`".to_string(),
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
            host_start_request_pending: false,
            host_last_start_trigger: None,
            host_primary_drm_path: None,
            host_forced_drm_path: None,
            host_forced_output_name: None,
            host_device_selection_state: RuntimeHostSelectionState::Automatic,
            host_output_selection_state: RuntimeHostSelectionState::Automatic,
            host_active_connector_name: None,
            host_active_connector_id: None,
            host_last_selection_attempt: None,
            host_last_selection_result: None,
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
            main_app_binding_evidence: None,
            overlay_surface_id: None,
            overlay_bound_pane_id: None,
            active_focus_target: None,
            overlay_region_debug_borders: false,
            denied_toplevel_count: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub host_mode_active: bool,
    pub output_rotation: OutputRotation,
    pub panes: Vec<PaneStatus>,
    #[serde(default)]
    pub overlay_regions: OverlayRegionsStatus,
    pub prototype_policy: PrototypePolicyStatus,
    pub runtime: RuntimeStatus,
}

#[cfg(test)]
mod tests {
    use super::{
        MainAppLaunchIntent, MainAppLaunchState, MainAppSurfaceBinding, MainAppSurfaceBindingMatch,
        ProcessSpec,
    };
    use std::collections::BTreeMap;

    #[test]
    fn main_app_surface_binding_matches_exact_app_id() {
        let binding = MainAppSurfaceBinding::AppId {
            app_id: "surf-ace".to_string(),
        };

        assert_eq!(
            binding.match_identity(Some("surf-ace"), Some("ignored")),
            MainAppSurfaceBindingMatch::Match
        );
        assert_eq!(
            binding.match_identity(Some("surf-ace-dev"), Some("ignored")),
            MainAppSurfaceBindingMatch::Mismatch
        );
        assert_eq!(
            binding.match_identity(None, Some("ignored")),
            MainAppSurfaceBindingMatch::Pending
        );
    }

    #[test]
    fn main_app_surface_binding_requires_both_app_id_and_title_when_declared() {
        let binding = MainAppSurfaceBinding::AppIdAndTitle {
            app_id: "surf-ace".to_string(),
            title: "Surf Ace".to_string(),
        };

        assert_eq!(
            binding.match_identity(Some("surf-ace"), None),
            MainAppSurfaceBindingMatch::Pending
        );
        assert_eq!(
            binding.match_identity(Some("surf-ace"), Some("Other")),
            MainAppSurfaceBindingMatch::Mismatch
        );
        assert_eq!(
            binding.match_identity(Some("surf-ace"), Some("Surf Ace")),
            MainAppSurfaceBindingMatch::Match
        );
    }

    #[test]
    fn main_app_launch_intent_validation_rejects_empty_binding_or_command() {
        let invalid_command = MainAppLaunchIntent {
            process: ProcessSpec {
                command: "   ".to_string(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
            },
            binding: MainAppSurfaceBinding::AppId {
                app_id: "surf-ace".to_string(),
            },
        };
        assert_eq!(
            invalid_command.validate(),
            Err("main app process command must not be empty")
        );

        let invalid_binding = MainAppLaunchIntent {
            process: ProcessSpec {
                command: "foot".to_string(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
            },
            binding: MainAppSurfaceBinding::Title {
                title: "".to_string(),
            },
        };
        assert_eq!(
            invalid_binding.validate(),
            Err("main app binding title must not be empty")
        );
    }

    #[test]
    fn main_app_launch_state_defaults_to_not_requested() {
        assert_eq!(
            MainAppLaunchState::default(),
            MainAppLaunchState::NotRequested
        );
    }
}
