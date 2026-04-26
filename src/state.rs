use crate::model::{
    ExternalNativeEventContract, ExternalNativeLifecycleState, HostRuntimeStartTrigger,
    MainAppLaunchIntent, MainAppLaunchState, MainAppSurfaceBinding, NativePaneHostRequest,
    NativePaneHostStatus, NativeTargetClass, OutputRotation, OverlayCoordinateSpace, OverlayRect,
    OverlayRegionRequest, OverlayRegionStatus, OverlayRegionUpdateReason, OverlayRegionsStatus,
    PaneId, PaneRenderMode, PaneStatus, ProcessSpec, ProviderPaneSnapshot, RuntimeBackend,
    RuntimeDmabufFormatStatus, RuntimeFocusTarget, RuntimeHostPresentOwnership,
    RuntimeHostQueuedPresentSource, RuntimeHostSelectionState, RuntimePhase, RuntimeSelectionMode,
    RuntimeStatus, StatusSnapshot, SurfaceBindingEvidence,
};
use crate::policy::{PrototypeOverlayPolicy, PrototypePolicyError};
use crate::process_manager::ProcessController;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const SHELL_OVERLAY_PANE_ID: &str = "shell-overlay";
const OVERLAY_REGIONS_MAX_COUNT: usize = 1024;
pub const LAUNCH_TOKEN_ENV: &str = "SURF_ACE_COMPOSITOR_LAUNCH_TOKEN";
pub const NATIVE_PANE_CONTENT_ID_ENV: &str = "SURF_ACE_NATIVE_PANE_CONTENT_ID";
pub const NATIVE_PANE_BINDING_ID_ENV: &str = "SURF_ACE_NATIVE_PANE_BINDING_ID";
pub const NATIVE_PANE_REVISION_ENV: &str = "SURF_ACE_NATIVE_PANE_REVISION";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneRuntimeState {
    geometry: crate::model::PaneGeometry,
    render_mode: PaneRenderMode,
    external_native_state: ExternalNativeLifecycleState,
    native_host_content_id: Option<String>,
    native_host_binding_id: Option<String>,
    native_host_revision: u64,
    external_native_surface_id: Option<u32>,
    external_native_binding_evidence: Option<SurfaceBindingEvidence>,
    external_native_launch_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainAppBindingExpectation {
    pub pid: u32,
    pub binding: MainAppSurfaceBinding,
    pub launch_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePaneBindingExpectation {
    pub pane_id: PaneId,
    pub pid: u32,
    pub launch_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverlaySurfaceKey {
    surface_id: String,
    window_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct OverlayRegionSnapshot {
    key: OverlaySurfaceKey,
    revision: u64,
    topology_epoch: String,
    update_reason: Option<OverlayRegionUpdateReason>,
    last_updated_at: u64,
    regions: Vec<OverlayRegionStatus>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    #[error("pane not found: {0:?}")]
    PaneNotFound(PaneId),
    #[error("invalid process spec: command must not be empty")]
    InvalidProcessSpec,
    #[error("invalid main app launch intent: {0}")]
    InvalidMainAppLaunchIntent(String),
    #[error("pane is not in external/native mode: {0:?}")]
    PaneNotExternalNative(PaneId),
    #[error("cannot switch pane to external/native mode: {0:?}")]
    AlreadyExternalNative(PaneId),
    #[error("{0}")]
    PrototypePolicy(#[from] PrototypePolicyError),
    #[error("{0}")]
    Process(String),
    #[error("{0}")]
    ShellOverlayUnavailable(String),
    #[error("invalid overlay region: {0}")]
    InvalidOverlayRegion(String),
    #[error("stale overlay region revision: {incoming} <= {active}")]
    StaleOverlayRegionRevision { incoming: u64, active: u64 },
    #[error("stale overlay topology epoch: {incoming} != {active}")]
    StaleOverlayTopologyEpoch { incoming: String, active: String },
    #[error("too many overlay regions: {count} > {max}")]
    TooManyOverlayRegions { count: usize, max: usize },
}

pub struct CompositorState {
    host_mode_active: bool,
    output_rotation: OutputRotation,
    panes: HashMap<PaneId, PaneRuntimeState>,
    shell_overlay_process: Option<ProcessSpec>,
    shell_overlay_lifecycle: ExternalNativeLifecycleState,
    shell_overlay_focus_on_attach: bool,
    overlay_regions: Option<OverlayRegionSnapshot>,
    topology_epoch_counter: u64,
    topology_epoch: String,
    prototype_overlay_policy: PrototypeOverlayPolicy,
    runtime: RuntimeStatus,
    process_controller: Box<dyn ProcessController>,
    launch_token_counter: u64,
}

impl CompositorState {
    fn next_launch_token(&mut self, scope: &str) -> String {
        self.launch_token_counter = self.launch_token_counter.saturating_add(1);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut hasher);
        self.launch_token_counter.hash(&mut hasher);
        now.hash(&mut hasher);
        scope.hash(&mut hasher);
        format!(
            "sa-launch-{:016x}-{:016x}",
            self.launch_token_counter,
            hasher.finish()
        )
    }

    fn clear_runtime_session_status(&mut self) {
        self.runtime.wayland_socket = None;
        self.runtime.window_width = None;
        self.runtime.window_height = None;
        self.runtime.main_app_surface_id = None;
        self.runtime.main_app_binding_evidence = None;
        self.runtime.overlay_surface_id = None;
        self.runtime.overlay_bound_pane_id = None;
        self.runtime.active_focus_target = None;
    }

    fn clear_host_runtime_route_status(&mut self) {
        self.runtime.host_seat_name = None;
        self.runtime.host_detected_drm_device_count = 0;
        self.runtime.host_opened_drm_device_count = 0;
        self.runtime.host_output_ownership = false;
        self.runtime.host_primary_drm_path = None;
        self.runtime.host_active_connector_name = None;
        self.runtime.host_active_connector_id = None;
        self.runtime.host_last_selection_attempt = None;
        self.runtime.host_last_selection_result = None;
        self.runtime.host_present_ownership = RuntimeHostPresentOwnership::None;
        self.runtime.host_atomic_commit_enabled = false;
        self.runtime.host_overlay_plane_capable = false;
        self.runtime.host_last_queued_present_source = RuntimeHostQueuedPresentSource::None;
        self.runtime.host_last_queued_atomic_commit = false;
        self.runtime.host_last_queued_overlay_plane = false;
        self.runtime.host_last_queued_primary_dmabuf_format = None;
        self.runtime.host_last_queued_overlay_dmabuf_format = None;
        self.runtime.dmabuf_protocol_enabled = false;
        self.runtime.dmabuf_protocol_formats.clear();
    }

    pub fn new(host_mode_active: bool, process_controller: Box<dyn ProcessController>) -> Self {
        Self {
            host_mode_active,
            output_rotation: OutputRotation::Deg0,
            panes: HashMap::new(),
            shell_overlay_process: None,
            shell_overlay_lifecycle: ExternalNativeLifecycleState::Absent,
            shell_overlay_focus_on_attach: false,
            overlay_regions: None,
            topology_epoch_counter: 0,
            topology_epoch: "topology-0".to_string(),
            prototype_overlay_policy: PrototypeOverlayPolicy::default(),
            runtime: RuntimeStatus::default(),
            process_controller,
            launch_token_counter: 0,
        }
    }

    pub fn host_mode_active(&self) -> bool {
        self.host_mode_active
    }

    pub fn set_output_rotation(&mut self, rotation: OutputRotation) {
        self.output_rotation = rotation;
    }

    pub fn output_rotation(&self) -> OutputRotation {
        self.output_rotation
    }

    pub fn topology_epoch(&self) -> &str {
        &self.topology_epoch
    }

    pub fn runtime_main_app_launch_intent(&self) -> Option<&MainAppLaunchIntent> {
        self.runtime.main_app_launch_intent.as_ref()
    }

    pub fn select_main_app_launch_intent(
        &mut self,
        intent: MainAppLaunchIntent,
    ) -> Result<(), StateError> {
        intent
            .validate()
            .map_err(|err| StateError::InvalidMainAppLaunchIntent(err.to_string()))?;

        self.terminate_running_main_app_process();
        self.runtime.main_app_surface_id = None;
        self.runtime.main_app_binding_evidence = None;
        self.runtime.main_app_launch_token = None;
        self.runtime.main_app_launch_intent = Some(intent);
        self.runtime.main_app_launch_state = MainAppLaunchState::WaitingForRuntime;
        self.launch_configured_main_app_if_runtime_ready();
        Ok(())
    }

    pub fn set_shell_overlay_toggle_shortcut(&mut self, shortcut: impl Into<String>) {
        self.runtime.shell_overlay_toggle_shortcut = shortcut.into();
    }

    pub fn configure_shell_overlay_process(&mut self, process: Option<ProcessSpec>) {
        self.shell_overlay_process = process;
    }

    pub fn mark_runtime_starting(&mut self, backend: RuntimeBackend) {
        self.prepare_main_app_for_runtime_reset();
        self.runtime.backend = backend;
        self.runtime.phase = RuntimePhase::Starting;
        self.runtime.host_start_request_pending = false;
        self.runtime.runtime_operator_action_needed = false;
        self.runtime.runtime_operator_action_reason = None;
        self.clear_runtime_session_status();
        self.clear_host_runtime_route_status();
    }

    pub fn mark_host_runtime_start_requested(&mut self, trigger: HostRuntimeStartTrigger) {
        self.prepare_main_app_for_runtime_reset();
        self.runtime.backend = RuntimeBackend::HostDrm;
        self.runtime.host_start_request_pending = true;
        self.runtime.host_start_attempt_count =
            self.runtime.host_start_attempt_count.saturating_add(1);
        self.runtime.host_last_start_trigger = Some(trigger);
        self.runtime.runtime_operator_action_needed = false;
        self.runtime.runtime_operator_action_reason = None;
        self.clear_runtime_session_status();
        self.clear_host_runtime_route_status();
    }

    pub fn mark_runtime_host_preflight_ready(&mut self, wayland_socket: Option<String>) {
        self.runtime.backend = RuntimeBackend::HostDrm;
        self.runtime.phase = RuntimePhase::PreflightReady;
        self.runtime.host_start_request_pending = false;
        self.runtime.wayland_socket = wayland_socket;
        self.runtime.runtime_operator_action_needed = false;
        self.runtime.runtime_operator_action_reason = None;
        self.runtime.window_width = None;
        self.runtime.window_height = None;
        self.runtime.main_app_surface_id = None;
        self.runtime.main_app_binding_evidence = None;
        self.runtime.overlay_surface_id = None;
        self.runtime.overlay_bound_pane_id = None;
        self.runtime.active_focus_target = None;
        self.runtime.host_output_ownership = false;
        self.runtime.host_active_connector_name = None;
        self.runtime.host_active_connector_id = None;
        self.runtime.host_present_ownership = RuntimeHostPresentOwnership::None;
        self.runtime.host_atomic_commit_enabled = false;
        self.runtime.host_overlay_plane_capable = false;
        self.runtime.host_last_queued_present_source = RuntimeHostQueuedPresentSource::None;
        self.runtime.host_last_queued_atomic_commit = false;
        self.runtime.host_last_queued_overlay_plane = false;
        self.runtime.host_last_queued_primary_dmabuf_format = None;
        self.runtime.host_last_queued_overlay_dmabuf_format = None;
        self.runtime.dmabuf_protocol_enabled = false;
        self.runtime.dmabuf_protocol_formats.clear();
    }

    pub fn mark_runtime_running(
        &mut self,
        backend: RuntimeBackend,
        wayland_socket: Option<String>,
        window_width: i32,
        window_height: i32,
    ) {
        self.runtime.backend = backend;
        self.runtime.phase = RuntimePhase::Running;
        self.runtime.host_start_request_pending = false;
        self.runtime.wayland_socket = wayland_socket;
        self.runtime.window_width = Some(window_width);
        self.runtime.window_height = Some(window_height);
        self.runtime.last_error = None;
        self.runtime.runtime_operator_action_needed = false;
        self.runtime.runtime_operator_action_reason = None;
        self.runtime.host_output_ownership = matches!(backend, RuntimeBackend::HostDrm);
        self.launch_configured_main_app_if_runtime_ready();
    }

    pub fn mark_runtime_host_running(
        &mut self,
        wayland_socket: String,
        window_width: i32,
        window_height: i32,
        seat_name: Option<String>,
        detected_drm_device_count: usize,
        opened_drm_device_count: usize,
        primary_drm_path: Option<String>,
        active_connector_name: Option<String>,
        active_connector_id: Option<u32>,
        last_selection_attempt: Option<String>,
        last_selection_result: Option<String>,
        present_ownership: RuntimeHostPresentOwnership,
        atomic_commit_enabled: bool,
        overlay_plane_capable: bool,
    ) {
        self.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some(wayland_socket),
            window_width,
            window_height,
        );
        self.set_runtime_host_backend_snapshot(
            seat_name,
            detected_drm_device_count,
            opened_drm_device_count,
            primary_drm_path,
        );
        self.set_runtime_host_route_selection_status(
            active_connector_name,
            active_connector_id,
            last_selection_attempt,
            last_selection_result,
        );
        self.set_runtime_host_present_capabilities(
            present_ownership,
            atomic_commit_enabled,
            overlay_plane_capable,
        );
    }

    pub fn set_runtime_selection_status(
        &mut self,
        mode: RuntimeSelectionMode,
        operator_action_needed: bool,
        operator_action_reason: Option<String>,
        last_selection_attempt: Option<String>,
        last_selection_result: Option<String>,
    ) {
        self.runtime.runtime_selection_mode = mode;
        self.runtime.runtime_operator_action_needed = operator_action_needed;
        self.runtime.runtime_operator_action_reason = operator_action_reason;
        self.runtime.runtime_last_selection_attempt = last_selection_attempt;
        self.runtime.runtime_last_selection_result = last_selection_result;
    }

    pub fn set_runtime_host_selection_overrides(
        &mut self,
        forced_drm_path: Option<String>,
        forced_output_name: Option<String>,
        device_selection_state: RuntimeHostSelectionState,
        output_selection_state: RuntimeHostSelectionState,
    ) {
        self.runtime.host_forced_drm_path = forced_drm_path;
        self.runtime.host_forced_output_name = forced_output_name;
        self.runtime.host_device_selection_state = device_selection_state;
        self.runtime.host_output_selection_state = output_selection_state;
    }

    pub fn set_runtime_host_route_selection_status(
        &mut self,
        active_connector_name: Option<String>,
        active_connector_id: Option<u32>,
        last_selection_attempt: Option<String>,
        last_selection_result: Option<String>,
    ) {
        self.runtime.host_active_connector_name = active_connector_name;
        self.runtime.host_active_connector_id = active_connector_id;
        self.runtime.host_last_selection_attempt = last_selection_attempt;
        self.runtime.host_last_selection_result = last_selection_result;
    }

    pub fn set_runtime_host_backend_snapshot(
        &mut self,
        seat_name: Option<String>,
        detected_drm_device_count: usize,
        opened_drm_device_count: usize,
        primary_drm_path: Option<String>,
    ) {
        self.runtime.host_seat_name = seat_name;
        self.runtime.host_detected_drm_device_count =
            detected_drm_device_count.try_into().unwrap_or(u32::MAX);
        self.runtime.host_opened_drm_device_count =
            opened_drm_device_count.try_into().unwrap_or(u32::MAX);
        self.runtime.host_primary_drm_path = primary_drm_path;
    }

    pub fn set_runtime_dmabuf_protocol_formats(&mut self, formats: Vec<RuntimeDmabufFormatStatus>) {
        self.runtime.dmabuf_protocol_enabled = !formats.is_empty();
        self.runtime.dmabuf_protocol_formats = formats;
    }

    pub fn set_runtime_host_present_capabilities(
        &mut self,
        ownership: RuntimeHostPresentOwnership,
        atomic_commit_enabled: bool,
        overlay_plane_capable: bool,
    ) {
        self.runtime.host_present_ownership = ownership;
        self.runtime.host_atomic_commit_enabled = atomic_commit_enabled;
        self.runtime.host_overlay_plane_capable = overlay_plane_capable;
        if matches!(ownership, RuntimeHostPresentOwnership::None) {
            self.runtime.host_last_queued_present_source = RuntimeHostQueuedPresentSource::None;
            self.runtime.host_last_queued_atomic_commit = false;
            self.runtime.host_last_queued_overlay_plane = false;
            self.runtime.host_last_queued_primary_dmabuf_format = None;
            self.runtime.host_last_queued_overlay_dmabuf_format = None;
        }
    }

    pub fn set_runtime_last_queued_present(
        &mut self,
        source: RuntimeHostQueuedPresentSource,
        atomic_commit: bool,
        overlay_plane: bool,
        primary_dmabuf_format: Option<RuntimeDmabufFormatStatus>,
        overlay_dmabuf_format: Option<RuntimeDmabufFormatStatus>,
    ) {
        self.runtime.host_last_queued_present_source = source;
        self.runtime.host_last_queued_atomic_commit = atomic_commit;
        self.runtime.host_last_queued_overlay_plane = overlay_plane;
        self.runtime.host_last_queued_primary_dmabuf_format = primary_dmabuf_format;
        self.runtime.host_last_queued_overlay_dmabuf_format = overlay_dmabuf_format;
    }

    pub fn mark_runtime_resize(&mut self, window_width: i32, window_height: i32) {
        self.runtime.window_width = Some(window_width);
        self.runtime.window_height = Some(window_height);
    }

    pub fn mark_runtime_redraw(&mut self) {
        self.runtime.redraw_count += 1;
    }

    pub fn mark_runtime_input_event(&mut self) {
        self.runtime.input_event_count += 1;
    }

    pub fn set_runtime_surface_roles(
        &mut self,
        main_app_surface_id: Option<u32>,
        overlay_surface_id: Option<u32>,
        overlay_bound_pane_id: Option<PaneId>,
    ) {
        self.runtime.main_app_surface_id = main_app_surface_id;
        self.runtime.overlay_surface_id = overlay_surface_id;
        self.runtime.overlay_bound_pane_id = overlay_bound_pane_id;
    }

    pub fn set_overlay_regions(
        &mut self,
        surface_id: String,
        window_id: Option<String>,
        revision: u64,
        topology_epoch: String,
        update_reason: Option<OverlayRegionUpdateReason>,
        coordinate_space: OverlayCoordinateSpace,
        regions: Vec<OverlayRegionRequest>,
    ) -> Result<(), StateError> {
        let key = validate_overlay_surface_key(surface_id, window_id)?;
        if !matches!(coordinate_space, OverlayCoordinateSpace::SurfaceLogical) {
            return Err(StateError::InvalidOverlayRegion(
                "coordinateSpace must be surface_logical".to_string(),
            ));
        }
        if regions.len() > OVERLAY_REGIONS_MAX_COUNT {
            return Err(StateError::TooManyOverlayRegions {
                count: regions.len(),
                max: OVERLAY_REGIONS_MAX_COUNT,
            });
        }
        if topology_epoch != self.topology_epoch {
            return Err(StateError::StaleOverlayTopologyEpoch {
                incoming: topology_epoch,
                active: self.topology_epoch.clone(),
            });
        }
        if let Some(active) = &self.overlay_regions {
            if active.key == key && revision <= active.revision {
                return Err(StateError::StaleOverlayRegionRevision {
                    incoming: revision,
                    active: active.revision,
                });
            }
        }

        let mut next_regions = Vec::with_capacity(regions.len());
        for region in regions {
            validate_overlay_region(&region)?;
            let Some(pane) = self.panes.get(&region.pane_id) else {
                return Err(StateError::PaneNotFound(region.pane_id));
            };
            if !pane_allows_overlay_regions(pane) {
                return Err(StateError::InvalidOverlayRegion(format!(
                    "pane {:?} is not a live native-hosted pane",
                    region.pane_id
                )));
            }
            let live_instance_id = pane_instance_id(&region.pane_id, pane);
            if region.pane_instance_id != live_instance_id {
                return Err(StateError::InvalidOverlayRegion(format!(
                    "pane_instance_id '{}' does not match live pane instance '{}'",
                    region.pane_instance_id, live_instance_id
                )));
            }
            let (rect, clamped) = clamp_overlay_rect_to_runtime_bounds(
                region.rect,
                self.runtime.window_width,
                self.runtime.window_height,
            )?;
            next_regions.push(OverlayRegionStatus {
                region_id: region.region_id,
                pane_id: region.pane_id,
                pane_instance_id: region.pane_instance_id,
                kind: region.kind,
                rect,
                z_index: region.z_index,
                captures: region.captures,
                clamped,
            });
        }

        self.overlay_regions = Some(OverlayRegionSnapshot {
            key,
            revision,
            topology_epoch: self.topology_epoch.clone(),
            update_reason,
            last_updated_at: current_unix_millis(),
            regions: next_regions,
        });
        Ok(())
    }

    pub fn clear_overlay_regions(
        &mut self,
        surface_id: String,
        window_id: Option<String>,
    ) -> Result<(), StateError> {
        let key = validate_overlay_surface_key(surface_id, window_id)?;
        if self
            .overlay_regions
            .as_ref()
            .is_some_and(|snapshot| snapshot.key == key)
        {
            self.overlay_regions = None;
        }
        Ok(())
    }

    pub fn overlay_regions_status(&self) -> OverlayRegionsStatus {
        let Some(snapshot) = &self.overlay_regions else {
            return OverlayRegionsStatus {
                topology_epoch: Some(self.topology_epoch.clone()),
                ..OverlayRegionsStatus::default()
            };
        };
        OverlayRegionsStatus {
            surface_id: Some(snapshot.key.surface_id.clone()),
            window_id: snapshot.key.window_id.clone(),
            active_revision: Some(snapshot.revision),
            region_count: snapshot.regions.len(),
            topology_epoch: Some(self.topology_epoch.clone()),
            update_reason: snapshot.update_reason.clone(),
            last_updated_at: Some(snapshot.last_updated_at),
            regions: snapshot.regions.clone(),
        }
    }

    pub fn set_runtime_focus_target(&mut self, target: Option<RuntimeFocusTarget>) {
        self.runtime.active_focus_target = target;
    }

    pub fn set_overlay_region_debug_borders(&mut self, enabled: bool) {
        self.runtime.overlay_region_debug_borders = enabled;
    }

    pub fn runtime_expected_main_app_binding(&self) -> Option<(u32, MainAppSurfaceBinding)> {
        self.runtime_expected_main_app_binding_with_token()
            .map(|expectation| (expectation.pid, expectation.binding))
    }

    pub fn runtime_expected_main_app_binding_with_token(
        &self,
    ) -> Option<MainAppBindingExpectation> {
        let binding = self
            .runtime
            .main_app_launch_intent
            .as_ref()
            .map(|intent| intent.binding.clone())?;
        match self.runtime.main_app_launch_state {
            MainAppLaunchState::Launching { pid } | MainAppLaunchState::Attached { pid } => {
                Some(MainAppBindingExpectation {
                    pid,
                    binding,
                    launch_token: self.runtime.main_app_launch_token.clone(),
                })
            }
            MainAppLaunchState::Exited { pid, .. }
                if self.runtime.main_app_launch_token.is_some() =>
            {
                Some(MainAppBindingExpectation {
                    pid,
                    binding,
                    launch_token: self.runtime.main_app_launch_token.clone(),
                })
            }
            MainAppLaunchState::NotRequested
            | MainAppLaunchState::WaitingForRuntime
            | MainAppLaunchState::Failed { .. }
            | MainAppLaunchState::Exited { .. } => None,
        }
    }

    pub fn runtime_expected_native_pane_bindings(&self) -> Vec<NativePaneBindingExpectation> {
        self.panes
            .iter()
            .filter_map(|(pane_id, pane)| {
                if !matches!(pane.render_mode, PaneRenderMode::ExternalNative { .. }) {
                    return None;
                }
                match pane.external_native_state {
                    ExternalNativeLifecycleState::Launching { pid }
                    | ExternalNativeLifecycleState::Attached { pid } => {
                        Some(NativePaneBindingExpectation {
                            pane_id: pane_id.clone(),
                            pid,
                            launch_token: pane.external_native_launch_token.clone(),
                        })
                    }
                    ExternalNativeLifecycleState::Exited { pid, .. }
                        if pane.external_native_launch_token.is_some() =>
                    {
                        Some(NativePaneBindingExpectation {
                            pane_id: pane_id.clone(),
                            pid,
                            launch_token: pane.external_native_launch_token.clone(),
                        })
                    }
                    ExternalNativeLifecycleState::Absent
                    | ExternalNativeLifecycleState::Failed { .. }
                    | ExternalNativeLifecycleState::Exited { .. } => None,
                }
            })
            .collect()
    }

    pub fn runtime_mark_main_app_surface_attached_for_pid(&mut self, client_pid: u32) -> bool {
        self.runtime_mark_main_app_surface_attached_for_pid_with_evidence(client_pid, None)
    }

    pub fn runtime_mark_main_app_surface_attached_for_pid_with_evidence(
        &mut self,
        client_pid: u32,
        evidence: Option<SurfaceBindingEvidence>,
    ) -> bool {
        self.runtime_mark_main_app_surface_attached_for_launch_pid_with_evidence(
            client_pid, client_pid, evidence,
        )
    }

    pub fn runtime_mark_main_app_surface_attached_for_launch_pid_with_evidence(
        &mut self,
        launch_pid: u32,
        client_pid: u32,
        evidence: Option<SurfaceBindingEvidence>,
    ) -> bool {
        match self.runtime.main_app_launch_state {
            MainAppLaunchState::Launching { pid } if pid == launch_pid => {
                self.runtime.main_app_launch_state =
                    MainAppLaunchState::Attached { pid: client_pid };
                self.runtime.main_app_binding_evidence = evidence;
                true
            }
            MainAppLaunchState::Exited { pid, .. }
                if pid == launch_pid && self.runtime.main_app_launch_token.is_some() =>
            {
                self.runtime.main_app_launch_state =
                    MainAppLaunchState::Attached { pid: client_pid };
                self.runtime.main_app_binding_evidence = evidence;
                true
            }
            MainAppLaunchState::Attached { pid } if pid == client_pid => {
                if evidence.is_some() {
                    self.runtime.main_app_binding_evidence = evidence;
                }
                true
            }
            MainAppLaunchState::NotRequested
            | MainAppLaunchState::WaitingForRuntime
            | MainAppLaunchState::Launching { .. }
            | MainAppLaunchState::Attached { .. }
            | MainAppLaunchState::Failed { .. }
            | MainAppLaunchState::Exited { .. } => false,
        }
    }

    pub fn runtime_mark_main_app_surface_detached_for_pid(&mut self, client_pid: u32) -> bool {
        match self.runtime.main_app_launch_state {
            MainAppLaunchState::Attached { pid } if pid == client_pid => {
                self.runtime.main_app_launch_state = MainAppLaunchState::Launching { pid };
                self.runtime.main_app_binding_evidence = None;
                true
            }
            MainAppLaunchState::Launching { pid } if pid == client_pid => true,
            MainAppLaunchState::NotRequested
            | MainAppLaunchState::WaitingForRuntime
            | MainAppLaunchState::Launching { .. }
            | MainAppLaunchState::Attached { .. }
            | MainAppLaunchState::Failed { .. }
            | MainAppLaunchState::Exited { .. } => false,
        }
    }

    pub fn increment_runtime_denied_toplevel(&mut self) {
        self.runtime.denied_toplevel_count += 1;
    }

    pub fn mark_runtime_failed(&mut self, error: impl Into<String>) {
        self.prepare_main_app_for_runtime_reset();
        let error = error.into();
        self.runtime.phase = RuntimePhase::Failed;
        self.runtime.host_start_request_pending = false;
        self.runtime.last_error = Some(error.clone());
        self.runtime.runtime_operator_action_needed = matches!(
            self.runtime.backend,
            RuntimeBackend::HostDrm | RuntimeBackend::None
        ) && self.host_mode_active;
        self.runtime.runtime_operator_action_reason = self
            .runtime
            .runtime_operator_action_needed
            .then(|| format!("host runtime failed; explicit recovery required: {error}"));
        self.clear_runtime_session_status();
        self.clear_host_runtime_route_status();
    }

    pub fn mark_runtime_stopped(&mut self) {
        self.prepare_main_app_for_runtime_reset();
        self.runtime.phase = RuntimePhase::Stopped;
        self.runtime.host_start_request_pending = false;
        self.clear_runtime_session_status();
        self.clear_host_runtime_route_status();
    }

    pub fn apply_provider_snapshot(
        &mut self,
        provider_panes: Vec<ProviderPaneSnapshot>,
    ) -> Result<(), StateError> {
        self.bump_topology_epoch();
        let mut incoming = HashMap::new();
        for pane in provider_panes {
            let previous = self.panes.get(&pane.id);
            let runtime = match previous {
                Some(prev) => PaneRuntimeState {
                    geometry: pane.geometry,
                    render_mode: prev.render_mode.clone(),
                    external_native_state: prev.external_native_state.clone(),
                    native_host_content_id: prev.native_host_content_id.clone(),
                    native_host_binding_id: prev.native_host_binding_id.clone(),
                    native_host_revision: prev.native_host_revision,
                    external_native_surface_id: prev.external_native_surface_id,
                    external_native_binding_evidence: prev.external_native_binding_evidence.clone(),
                    external_native_launch_token: prev.external_native_launch_token.clone(),
                },
                None => PaneRuntimeState {
                    geometry: pane.geometry,
                    render_mode: PaneRenderMode::SurfAceRendered,
                    external_native_state: ExternalNativeLifecycleState::Absent,
                    native_host_content_id: None,
                    native_host_binding_id: None,
                    native_host_revision: 0,
                    external_native_surface_id: None,
                    external_native_binding_evidence: None,
                    external_native_launch_token: None,
                },
            };
            incoming.insert(pane.id, runtime);
        }

        for (old_id, old_state) in &self.panes {
            if incoming.contains_key(old_id) {
                continue;
            }
            if let Some(pid) = running_pid(&old_state.external_native_state) {
                self.process_controller
                    .terminate(pid)
                    .map_err(StateError::Process)?;
            }
            self.prototype_overlay_policy.release_if_matches(old_id);
        }

        self.panes = incoming;
        self.prototype_overlay_policy.clear_if_removed(|pane_id| {
            self.panes.contains_key(pane_id) || is_shell_overlay_pane_id(pane_id)
        });
        self.prune_stale_overlay_regions();
        Ok(())
    }

    pub fn switch_pane_to_external_native(
        &mut self,
        pane_id: &PaneId,
        target: NativeTargetClass,
        process: ProcessSpec,
    ) -> Result<(), StateError> {
        if process.command.trim().is_empty() {
            return Err(StateError::InvalidProcessSpec);
        }

        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;

        if matches!(
            pane.external_native_state,
            ExternalNativeLifecycleState::Launching { .. }
                | ExternalNativeLifecycleState::Attached { .. }
        ) {
            return Err(StateError::AlreadyExternalNative(pane_id.clone()));
        }

        self.prototype_overlay_policy.reserve_for(pane_id)?;

        let launch_token = self.next_launch_token(&format!("native:{}", pane_id.0));
        let mut extra_env = BTreeMap::new();
        extra_env.insert("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string());
        extra_env.insert("SURF_ACE_PANE_ID".to_string(), pane_id.0.clone());
        extra_env.insert(LAUNCH_TOKEN_ENV.to_string(), launch_token.clone());
        if let Some(wayland_socket) = self.runtime.wayland_socket.clone() {
            extra_env.insert("WAYLAND_DISPLAY".to_string(), wayland_socket);
        }

        match self.process_controller.spawn(&process, &extra_env) {
            Ok(pid) => {
                let pane = self
                    .panes
                    .get_mut(pane_id)
                    .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                pane.render_mode = PaneRenderMode::ExternalNative {
                    target,
                    process: process.clone(),
                };
                pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
                pane.external_native_launch_token = Some(launch_token);
                Ok(())
            }
            Err(err) => {
                let pane = self
                    .panes
                    .get_mut(pane_id)
                    .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                pane.render_mode = PaneRenderMode::ExternalNative {
                    target,
                    process: process.clone(),
                };
                pane.external_native_state = ExternalNativeLifecycleState::Failed {
                    reason: err.clone(),
                };
                pane.external_native_launch_token = None;
                Err(StateError::Process(err))
            }
        }
    }

    pub fn apply_native_pane_host_plan(
        &mut self,
        requests: Vec<NativePaneHostRequest>,
    ) -> Result<(), StateError> {
        self.bump_topology_epoch();
        for request in &requests {
            if request.process.command.trim().is_empty() {
                return Err(StateError::InvalidProcessSpec);
            }
        }

        for request in requests {
            let pane = self
                .panes
                .entry(request.id)
                .or_insert_with(|| PaneRuntimeState {
                    geometry: request.geometry,
                    render_mode: PaneRenderMode::SurfAceRendered,
                    external_native_state: ExternalNativeLifecycleState::Absent,
                    native_host_content_id: None,
                    native_host_binding_id: None,
                    native_host_revision: 0,
                    external_native_surface_id: None,
                    external_native_binding_evidence: None,
                    external_native_launch_token: None,
                });
            let next_mode = PaneRenderMode::ExternalNative {
                target: request.target,
                process: request.process,
            };
            pane.geometry = request.geometry;
            let host_identity_changed = pane.render_mode != next_mode
                || pane.native_host_content_id != request.content_id
                || pane.native_host_binding_id != request.binding_id;
            if host_identity_changed {
                if let Some(pid) = running_pid(&pane.external_native_state) {
                    self.process_controller
                        .terminate(pid)
                        .map_err(StateError::Process)?;
                }
                pane.external_native_state = ExternalNativeLifecycleState::Absent;
                pane.external_native_surface_id = None;
                pane.external_native_binding_evidence = None;
                pane.external_native_launch_token = None;
            }
            pane.render_mode = next_mode;
            pane.native_host_content_id = request.content_id;
            pane.native_host_binding_id = request.binding_id;
            pane.native_host_revision = request.revision;
        }

        self.prune_stale_overlay_regions();
        Ok(())
    }

    pub fn launch_native_pane_hosts(&mut self, pane_ids: Vec<PaneId>) -> Result<(), StateError> {
        let selected_pane_ids = if pane_ids.is_empty() {
            let mut ids: Vec<_> = self
                .panes
                .iter()
                .filter_map(|(pane_id, pane)| match pane.render_mode {
                    PaneRenderMode::ExternalNative { .. } => Some(pane_id.clone()),
                    PaneRenderMode::SurfAceRendered => None,
                })
                .collect();
            ids.sort();
            ids
        } else {
            pane_ids
        };

        for pane_id in selected_pane_ids {
            let (process, should_launch) = {
                let pane = self
                    .panes
                    .get(&pane_id)
                    .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                let PaneRenderMode::ExternalNative { process, .. } = &pane.render_mode else {
                    return Err(StateError::PaneNotExternalNative(pane_id.clone()));
                };
                let should_launch = matches!(
                    pane.external_native_state,
                    ExternalNativeLifecycleState::Absent
                        | ExternalNativeLifecycleState::Failed { .. }
                        | ExternalNativeLifecycleState::Exited { .. }
                );
                (process.clone(), should_launch)
            };

            if !should_launch {
                continue;
            }

            let (content_id, binding_id, revision) = {
                let pane = self
                    .panes
                    .get(&pane_id)
                    .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                (
                    pane.native_host_content_id.clone(),
                    pane.native_host_binding_id.clone(),
                    pane.native_host_revision,
                )
            };
            let token_scope = format!(
                "native:{}:{}:{}:{}",
                pane_id.0,
                content_id.as_deref().unwrap_or(""),
                binding_id.as_deref().unwrap_or(""),
                revision
            );
            let launch_token = self.next_launch_token(&token_scope);
            let mut extra_env = BTreeMap::new();
            extra_env.insert("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string());
            extra_env.insert("SURF_ACE_PANE_ID".to_string(), pane_id.0.clone());
            extra_env.insert(LAUNCH_TOKEN_ENV.to_string(), launch_token.clone());
            if let Some(content_id) = &content_id {
                extra_env.insert(NATIVE_PANE_CONTENT_ID_ENV.to_string(), content_id.clone());
            }
            if let Some(binding_id) = &binding_id {
                extra_env.insert(NATIVE_PANE_BINDING_ID_ENV.to_string(), binding_id.clone());
            }
            extra_env.insert(NATIVE_PANE_REVISION_ENV.to_string(), revision.to_string());
            if let Some(wayland_socket) = self.runtime.wayland_socket.clone() {
                extra_env.insert("WAYLAND_DISPLAY".to_string(), wayland_socket);
            }

            match self.process_controller.spawn(&process, &extra_env) {
                Ok(pid) => {
                    let pane = self
                        .panes
                        .get_mut(&pane_id)
                        .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                    pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
                    pane.external_native_surface_id = None;
                    pane.external_native_binding_evidence = None;
                    pane.external_native_launch_token = Some(launch_token);
                }
                Err(err) => {
                    let pane = self
                        .panes
                        .get_mut(&pane_id)
                        .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;
                    pane.external_native_state = ExternalNativeLifecycleState::Failed {
                        reason: err.clone(),
                    };
                    pane.external_native_launch_token = None;
                    return Err(StateError::Process(err));
                }
            }
        }

        Ok(())
    }

    pub fn mark_external_surface_attached(&mut self, pane_id: &PaneId) -> Result<(), StateError> {
        self.mark_external_surface_attached_with_evidence(pane_id, None)
    }

    pub fn mark_external_surface_attached_with_evidence(
        &mut self,
        pane_id: &PaneId,
        evidence: Option<SurfaceBindingEvidence>,
    ) -> Result<(), StateError> {
        let pane = self
            .panes
            .get_mut(pane_id)
            .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;

        let ExternalNativeLifecycleState::Launching { pid } = pane.external_native_state else {
            return Err(StateError::PaneNotExternalNative(pane_id.clone()));
        };

        pane.external_native_state = ExternalNativeLifecycleState::Attached { pid };
        pane.external_native_binding_evidence = evidence;
        Ok(())
    }

    pub fn runtime_mark_native_pane_surface_attached_for_pid(
        &mut self,
        client_pid: u32,
        surface_id: Option<u32>,
        evidence: Option<SurfaceBindingEvidence>,
    ) -> Option<PaneId> {
        self.runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
            client_pid, client_pid, surface_id, evidence,
        )
    }

    pub fn runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
        &mut self,
        launch_pid: u32,
        client_pid: u32,
        surface_id: Option<u32>,
        evidence: Option<SurfaceBindingEvidence>,
    ) -> Option<PaneId> {
        for (pane_id, pane) in &mut self.panes {
            if !matches!(pane.render_mode, PaneRenderMode::ExternalNative { .. }) {
                continue;
            }
            match pane.external_native_state {
                ExternalNativeLifecycleState::Launching { pid } if pid == launch_pid => {
                    pane.external_native_state =
                        ExternalNativeLifecycleState::Attached { pid: client_pid };
                    pane.external_native_surface_id = surface_id;
                    pane.external_native_binding_evidence = evidence;
                    return Some(pane_id.clone());
                }
                ExternalNativeLifecycleState::Exited { pid, .. }
                    if pid == launch_pid && pane.external_native_launch_token.is_some() =>
                {
                    pane.external_native_state =
                        ExternalNativeLifecycleState::Attached { pid: client_pid };
                    pane.external_native_surface_id = surface_id;
                    pane.external_native_binding_evidence = evidence;
                    return Some(pane_id.clone());
                }
                ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => {
                    pane.external_native_surface_id = surface_id;
                    pane.external_native_binding_evidence = evidence;
                    return Some(pane_id.clone());
                }
                ExternalNativeLifecycleState::Absent
                | ExternalNativeLifecycleState::Launching { .. }
                | ExternalNativeLifecycleState::Attached { .. }
                | ExternalNativeLifecycleState::Failed { .. }
                | ExternalNativeLifecycleState::Exited { .. } => {}
            }
        }
        None
    }

    pub fn runtime_mark_native_pane_surface_detached_for_pid(&mut self, client_pid: u32) -> bool {
        for pane in self.panes.values_mut() {
            match pane.external_native_state {
                ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => {
                    pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
                    pane.external_native_surface_id = None;
                    pane.external_native_binding_evidence = None;
                    self.prune_stale_overlay_regions();
                    return true;
                }
                ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => {
                    pane.external_native_surface_id = None;
                    pane.external_native_binding_evidence = None;
                    self.prune_stale_overlay_regions();
                    return true;
                }
                ExternalNativeLifecycleState::Absent
                | ExternalNativeLifecycleState::Launching { .. }
                | ExternalNativeLifecycleState::Attached { .. }
                | ExternalNativeLifecycleState::Failed { .. }
                | ExternalNativeLifecycleState::Exited { .. } => {}
            }
        }
        false
    }

    pub fn switch_pane_to_surf_ace(&mut self, pane_id: &PaneId) -> Result<(), StateError> {
        let pane = self
            .panes
            .get_mut(pane_id)
            .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;

        if let Some(pid) = running_pid(&pane.external_native_state) {
            self.process_controller
                .terminate(pid)
                .map_err(StateError::Process)?;
        }

        pane.render_mode = PaneRenderMode::SurfAceRendered;
        pane.external_native_state = ExternalNativeLifecycleState::Absent;
        pane.native_host_content_id = None;
        pane.native_host_binding_id = None;
        pane.native_host_revision = 0;
        pane.external_native_surface_id = None;
        pane.external_native_binding_evidence = None;
        pane.external_native_launch_token = None;
        self.prototype_overlay_policy.release_if_matches(pane_id);
        self.prune_stale_overlay_regions();
        Ok(())
    }

    pub fn release_native_pane_hosts(&mut self, pane_ids: Vec<PaneId>) -> Result<(), StateError> {
        let selected_pane_ids = if pane_ids.is_empty() {
            let mut ids: Vec<_> = self.panes.keys().cloned().collect();
            ids.sort();
            ids
        } else {
            pane_ids
        };

        for pane_id in selected_pane_ids {
            if !self.panes.contains_key(&pane_id) {
                return Err(StateError::PaneNotFound(pane_id));
            }
            self.switch_pane_to_surf_ace(&pane_id)?;
        }

        Ok(())
    }

    pub fn toggle_shell_overlay(&mut self) -> Result<(), StateError> {
        match self.shell_overlay_lifecycle {
            ExternalNativeLifecycleState::Launching { .. }
            | ExternalNativeLifecycleState::Attached { .. } => self.dismiss_shell_overlay(),
            ExternalNativeLifecycleState::Absent
            | ExternalNativeLifecycleState::Failed { .. }
            | ExternalNativeLifecycleState::Exited { .. } => self.open_shell_overlay(),
        }
    }

    pub fn active_overlay_pane_id(&self) -> Option<PaneId> {
        self.prototype_overlay_policy.active_overlay_pane().cloned()
    }

    pub fn runtime_expected_overlay_binding(&self) -> Option<(PaneId, u32)> {
        let pane_id = self.active_overlay_pane_id()?;
        if is_shell_overlay_pane_id(&pane_id) {
            return match self.shell_overlay_lifecycle {
                ExternalNativeLifecycleState::Launching { pid }
                | ExternalNativeLifecycleState::Attached { pid } => Some((pane_id, pid)),
                ExternalNativeLifecycleState::Absent
                | ExternalNativeLifecycleState::Failed { .. }
                | ExternalNativeLifecycleState::Exited { .. } => None,
            };
        }
        let pane = self.panes.get(&pane_id)?;
        if !matches!(pane.render_mode, PaneRenderMode::ExternalNative { .. }) {
            return None;
        }
        match pane.external_native_state {
            ExternalNativeLifecycleState::Launching { pid }
            | ExternalNativeLifecycleState::Attached { pid } => Some((pane_id, pid)),
            ExternalNativeLifecycleState::Absent
            | ExternalNativeLifecycleState::Failed { .. }
            | ExternalNativeLifecycleState::Exited { .. } => None,
        }
    }

    pub fn runtime_overlay_binding_expected(&self) -> bool {
        self.runtime_expected_overlay_binding().is_some()
    }

    pub fn runtime_mark_overlay_surface_attached_for_pid(&mut self, client_pid: u32) -> bool {
        let Some((pane_id, expected_pid)) = self.runtime_expected_overlay_binding() else {
            return false;
        };
        if expected_pid != client_pid {
            return false;
        }
        if is_shell_overlay_pane_id(&pane_id) {
            return match self.shell_overlay_lifecycle {
                ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => {
                    self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Attached { pid };
                    true
                }
                ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => true,
                ExternalNativeLifecycleState::Absent
                | ExternalNativeLifecycleState::Launching { .. }
                | ExternalNativeLifecycleState::Attached { .. }
                | ExternalNativeLifecycleState::Failed { .. }
                | ExternalNativeLifecycleState::Exited { .. } => false,
            };
        }
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };

        match pane.external_native_state {
            ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => {
                pane.external_native_state = ExternalNativeLifecycleState::Attached { pid };
                pane.external_native_binding_evidence = None;
                true
            }
            ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => true,
            ExternalNativeLifecycleState::Absent
            | ExternalNativeLifecycleState::Launching { .. }
            | ExternalNativeLifecycleState::Attached { .. }
            | ExternalNativeLifecycleState::Failed { .. }
            | ExternalNativeLifecycleState::Exited { .. } => false,
        }
    }

    pub fn runtime_mark_overlay_surface_detached_for_pid(&mut self, client_pid: u32) -> bool {
        let Some((pane_id, expected_pid)) = self.runtime_expected_overlay_binding() else {
            return false;
        };
        if expected_pid != client_pid {
            return false;
        }
        if is_shell_overlay_pane_id(&pane_id) {
            return match self.shell_overlay_lifecycle {
                ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => {
                    self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Launching { pid };
                    true
                }
                ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => true,
                ExternalNativeLifecycleState::Absent
                | ExternalNativeLifecycleState::Launching { .. }
                | ExternalNativeLifecycleState::Attached { .. }
                | ExternalNativeLifecycleState::Failed { .. }
                | ExternalNativeLifecycleState::Exited { .. } => false,
            };
        }
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };

        match pane.external_native_state {
            ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => {
                pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
                pane.external_native_surface_id = None;
                pane.external_native_binding_evidence = None;
                true
            }
            ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => true,
            ExternalNativeLifecycleState::Absent
            | ExternalNativeLifecycleState::Launching { .. }
            | ExternalNativeLifecycleState::Attached { .. }
            | ExternalNativeLifecycleState::Failed { .. }
            | ExternalNativeLifecycleState::Exited { .. } => false,
        }
    }

    pub fn poll_processes(&mut self) {
        for process_exit in self.process_controller.reap_exited() {
            self.record_process_exit(process_exit.pid, process_exit.exit_code);
        }
    }

    pub fn record_process_exit(&mut self, pid: u32, exit_code: Option<i32>) {
        match self.runtime.main_app_launch_state {
            MainAppLaunchState::Launching { pid: current_pid }
            | MainAppLaunchState::Attached { pid: current_pid }
                if current_pid == pid =>
            {
                self.runtime.main_app_launch_state = MainAppLaunchState::Exited { pid, exit_code };
                self.runtime.main_app_surface_id = None;
                self.runtime.main_app_binding_evidence = None;
                if self.runtime.active_focus_target == Some(RuntimeFocusTarget::MainApp) {
                    self.runtime.active_focus_target = None;
                }
            }
            MainAppLaunchState::NotRequested
            | MainAppLaunchState::WaitingForRuntime
            | MainAppLaunchState::Launching { .. }
            | MainAppLaunchState::Attached { .. }
            | MainAppLaunchState::Failed { .. }
            | MainAppLaunchState::Exited { .. } => {}
        }
        if running_pid(&self.shell_overlay_lifecycle) == Some(pid) {
            self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Exited { pid, exit_code };
            self.shell_overlay_focus_on_attach = false;
        }
        for pane in self.panes.values_mut() {
            if running_pid(&pane.external_native_state) == Some(pid) {
                pane.external_native_state =
                    ExternalNativeLifecycleState::Exited { pid, exit_code };
                pane.external_native_surface_id = None;
                pane.external_native_binding_evidence = None;
            }
        }
        self.prune_stale_overlay_regions();
    }

    pub fn shell_overlay_focus_requested(&self) -> bool {
        self.shell_overlay_focus_on_attach
    }

    pub fn mark_shell_overlay_focus_applied(&mut self) {
        self.shell_overlay_focus_on_attach = false;
    }

    pub fn status_snapshot(&self) -> StatusSnapshot {
        let mut panes: Vec<PaneStatus> = self
            .panes
            .iter()
            .map(|(id, state)| PaneStatus {
                id: id.clone(),
                geometry: state.geometry,
                render_mode: state.render_mode.clone(),
                external_native_state: state.external_native_state.clone(),
                native_host: match &state.render_mode {
                    PaneRenderMode::ExternalNative { process, .. } => Some(NativePaneHostStatus {
                        pane_id: id.clone(),
                        content_id: state.native_host_content_id.clone(),
                        binding_id: state.native_host_binding_id.clone(),
                        revision: state.native_host_revision,
                        surface_id: state.external_native_surface_id,
                        lifecycle: state.external_native_state.clone(),
                        process: process.clone(),
                        binding_evidence: state.external_native_binding_evidence.clone(),
                    }),
                    PaneRenderMode::SurfAceRendered => None,
                },
                external_native_binding_evidence: state.external_native_binding_evidence.clone(),
                external_native_event_contract: match state.render_mode {
                    PaneRenderMode::ExternalNative { .. } => {
                        Some(ExternalNativeEventContract::terminal_v1())
                    }
                    PaneRenderMode::SurfAceRendered => None,
                },
            })
            .collect();
        panes.sort_by(|left, right| left.id.cmp(&right.id));

        StatusSnapshot {
            host_mode_active: self.host_mode_active,
            output_rotation: self.output_rotation,
            panes,
            overlay_regions: self.overlay_regions_status(),
            prototype_policy: self.prototype_overlay_policy.status(),
            runtime: self.runtime.clone(),
        }
    }

    fn open_shell_overlay(&mut self) -> Result<(), StateError> {
        let process = self.shell_overlay_process.clone().ok_or_else(|| {
            StateError::ShellOverlayUnavailable(
                "shell overlay process is not configured".to_string(),
            )
        })?;
        let pane_id = shell_overlay_pane_id();
        self.prototype_overlay_policy.reserve_for(&pane_id)?;

        let mut extra_env = BTreeMap::new();
        extra_env.insert("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string());
        extra_env.insert("SURF_ACE_PANE_ID".to_string(), pane_id.0.clone());
        if let Some(wayland_socket) = self.runtime.wayland_socket.clone() {
            extra_env.insert("WAYLAND_DISPLAY".to_string(), wayland_socket);
        }

        match self.process_controller.spawn(&process, &extra_env) {
            Ok(pid) => {
                self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Launching { pid };
                self.shell_overlay_focus_on_attach = true;
                Ok(())
            }
            Err(err) => {
                self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Failed {
                    reason: err.clone(),
                };
                self.shell_overlay_focus_on_attach = false;
                Err(StateError::Process(err))
            }
        }
    }

    fn dismiss_shell_overlay(&mut self) -> Result<(), StateError> {
        if let Some(pid) = running_pid(&self.shell_overlay_lifecycle) {
            self.process_controller
                .terminate(pid)
                .map_err(StateError::Process)?;
        }

        self.shell_overlay_lifecycle = ExternalNativeLifecycleState::Absent;
        self.shell_overlay_focus_on_attach = false;
        self.prototype_overlay_policy
            .release_if_matches(&shell_overlay_pane_id());
        self.runtime.active_focus_target = self
            .runtime
            .main_app_surface_id
            .map(|_| RuntimeFocusTarget::MainApp);
        Ok(())
    }

    fn launch_configured_main_app_if_runtime_ready(&mut self) {
        if !matches!(self.runtime.phase, RuntimePhase::Running) {
            if self.runtime.main_app_launch_intent.is_some()
                && !matches!(
                    self.runtime.main_app_launch_state,
                    MainAppLaunchState::Launching { .. } | MainAppLaunchState::Attached { .. }
                )
            {
                self.runtime.main_app_launch_state = MainAppLaunchState::WaitingForRuntime;
            }
            return;
        }

        let Some(intent) = self.runtime.main_app_launch_intent.clone() else {
            self.runtime.main_app_launch_state = MainAppLaunchState::NotRequested;
            return;
        };

        if matches!(
            self.runtime.main_app_launch_state,
            MainAppLaunchState::Launching { .. } | MainAppLaunchState::Attached { .. }
        ) {
            return;
        }

        let launch_token = self.next_launch_token("main");
        let mut extra_env = BTreeMap::new();
        extra_env.insert("SURF_ACE_COMPOSITOR_MAIN_APP".to_string(), "1".to_string());
        extra_env.insert(LAUNCH_TOKEN_ENV.to_string(), launch_token.clone());
        if self.host_mode_active {
            extra_env.insert("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string());
        }
        if let Some(wayland_socket) = self.runtime.wayland_socket.clone() {
            extra_env.insert("WAYLAND_DISPLAY".to_string(), wayland_socket);
        }

        match self.process_controller.spawn(&intent.process, &extra_env) {
            Ok(pid) => {
                self.runtime.main_app_launch_state = MainAppLaunchState::Launching { pid };
                self.runtime.main_app_surface_id = None;
                self.runtime.main_app_binding_evidence = None;
                self.runtime.main_app_launch_token = Some(launch_token);
            }
            Err(err) => {
                self.runtime.main_app_launch_state = MainAppLaunchState::Failed {
                    reason: err.clone(),
                };
                self.runtime.main_app_surface_id = None;
                self.runtime.main_app_binding_evidence = None;
                self.runtime.main_app_launch_token = None;
            }
        }
    }

    fn prepare_main_app_for_runtime_reset(&mut self) {
        self.terminate_running_main_app_process();
        self.runtime.main_app_surface_id = None;
        self.runtime.main_app_binding_evidence = None;
        self.runtime.main_app_launch_token = None;
        self.runtime.main_app_launch_state = if self.runtime.main_app_launch_intent.is_some() {
            MainAppLaunchState::WaitingForRuntime
        } else {
            MainAppLaunchState::NotRequested
        };
    }

    fn terminate_running_main_app_process(&mut self) {
        let pid = match self.runtime.main_app_launch_state {
            MainAppLaunchState::Launching { pid } | MainAppLaunchState::Attached { pid } => {
                Some(pid)
            }
            MainAppLaunchState::NotRequested
            | MainAppLaunchState::WaitingForRuntime
            | MainAppLaunchState::Failed { .. }
            | MainAppLaunchState::Exited { .. } => None,
        };

        if let Some(pid) = pid {
            let _ = self.process_controller.terminate(pid);
        }
    }

    fn prune_stale_overlay_regions(&mut self) {
        let Some(snapshot) = self.overlay_regions.as_mut() else {
            return;
        };
        snapshot.regions.retain(|region| {
            let Some(pane) = self.panes.get(&region.pane_id) else {
                return false;
            };
            pane_allows_overlay_regions(pane)
                && pane_instance_id(&region.pane_id, pane) == region.pane_instance_id
        });
        if snapshot.regions.is_empty() {
            self.overlay_regions = None;
        }
    }

    fn bump_topology_epoch(&mut self) {
        self.topology_epoch_counter = self.topology_epoch_counter.saturating_add(1);
        self.topology_epoch = format!("topology-{}", self.topology_epoch_counter);
    }
}

fn validate_overlay_region(region: &OverlayRegionRequest) -> Result<(), StateError> {
    if region.region_id.trim().is_empty() {
        return Err(StateError::InvalidOverlayRegion(
            "regionId must not be empty".to_string(),
        ));
    }
    if region.pane_id.0.trim().is_empty() {
        return Err(StateError::InvalidOverlayRegion(
            "pane_id must not be empty".to_string(),
        ));
    }
    if region.pane_instance_id.trim().is_empty() {
        return Err(StateError::InvalidOverlayRegion(
            "paneInstanceId must not be empty".to_string(),
        ));
    }
    if region.captures.is_empty() {
        return Err(StateError::InvalidOverlayRegion(
            "captures must not be empty".to_string(),
        ));
    }
    validate_overlay_rect(region.rect)
}

fn validate_overlay_rect(rect: OverlayRect) -> Result<(), StateError> {
    if !rect.x.is_finite()
        || !rect.y.is_finite()
        || !rect.width.is_finite()
        || !rect.height.is_finite()
    {
        return Err(StateError::InvalidOverlayRegion(
            "rect coordinates must be finite".to_string(),
        ));
    }
    if rect.width < 0.0 || rect.height < 0.0 {
        return Err(StateError::InvalidOverlayRegion(
            "rect width and height must not be negative".to_string(),
        ));
    }
    if rect.width == 0.0 || rect.height == 0.0 {
        return Err(StateError::InvalidOverlayRegion(
            "rect width and height must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

fn validate_overlay_surface_key(
    surface_id: String,
    window_id: Option<String>,
) -> Result<OverlaySurfaceKey, StateError> {
    if surface_id.trim().is_empty() {
        return Err(StateError::InvalidOverlayRegion(
            "surfaceId must not be empty".to_string(),
        ));
    }
    if window_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(StateError::InvalidOverlayRegion(
            "windowId must not be empty".to_string(),
        ));
    }
    Ok(OverlaySurfaceKey {
        surface_id,
        window_id,
    })
}

fn clamp_overlay_rect_to_runtime_bounds(
    rect: OverlayRect,
    window_width: Option<i32>,
    window_height: Option<i32>,
) -> Result<(OverlayRect, bool), StateError> {
    validate_overlay_rect(rect)?;
    let Some(max_width) = window_width.filter(|value| *value > 0).map(i64::from) else {
        return Ok((rect, false));
    };
    let Some(max_height) = window_height.filter(|value| *value > 0).map(i64::from) else {
        return Ok((rect, false));
    };

    let max_width = max_width as f64;
    let max_height = max_height as f64;
    let left = rect.x.clamp(0.0, (max_width - 1.0).max(0.0));
    let top = rect.y.clamp(0.0, (max_height - 1.0).max(0.0));
    let right = (rect.x + rect.width).clamp(left + 1.0, max_width);
    let bottom = (rect.y + rect.height).clamp(top + 1.0, max_height);
    let clamped = left != rect.x
        || top != rect.y
        || (right - left) != rect.width
        || (bottom - top) != rect.height;

    Ok((
        OverlayRect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        },
        clamped,
    ))
}

fn pane_allows_overlay_regions(pane: &PaneRuntimeState) -> bool {
    matches!(pane.render_mode, PaneRenderMode::ExternalNative { .. })
        && matches!(
            pane.external_native_state,
            ExternalNativeLifecycleState::Attached { .. }
        )
}

fn pane_instance_id(pane_id: &PaneId, pane: &PaneRuntimeState) -> String {
    pane.native_host_binding_id
        .clone()
        .unwrap_or_else(|| format!("{}:{}", pane_id.0, pane.native_host_revision))
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn running_pid(state: &ExternalNativeLifecycleState) -> Option<u32> {
    match state {
        ExternalNativeLifecycleState::Launching { pid } => Some(*pid),
        ExternalNativeLifecycleState::Attached { pid } => Some(*pid),
        ExternalNativeLifecycleState::Absent
        | ExternalNativeLifecycleState::Failed { .. }
        | ExternalNativeLifecycleState::Exited { .. } => None,
    }
}

fn shell_overlay_pane_id() -> PaneId {
    PaneId::new(SHELL_OVERLAY_PANE_ID)
}

fn is_shell_overlay_pane_id(pane_id: &PaneId) -> bool {
    pane_id.0 == SHELL_OVERLAY_PANE_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        CompositorOverlayKind, MainAppLaunchIntent, MainAppLaunchState, MainAppSurfaceBinding,
        OverlayCaptureCapability, PaneGeometry, ProviderPaneSnapshot, SurfaceBindingEvidence,
        SurfaceBindingEvidenceOutcome,
    };
    use crate::process_manager::{ProcessController, ProcessExit};
    use std::collections::{BTreeMap, HashSet};
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeProcessController {
        inner: Arc<Mutex<FakeProcessControllerInner>>,
    }

    #[derive(Default)]
    struct FakeProcessControllerInner {
        next_pid: u32,
        running: HashSet<u32>,
        terminated: Vec<u32>,
        queued_exits: Vec<ProcessExit>,
        fail_spawn: bool,
        spawned_env: Vec<BTreeMap<String, String>>,
    }

    impl FakeProcessController {
        fn with_fail_spawn(self, value: bool) -> Self {
            self.inner.lock().expect("lock").fail_spawn = value;
            self
        }

        fn terminated(&self) -> Vec<u32> {
            self.inner.lock().expect("lock").terminated.clone()
        }

        fn queue_exit(&self, pid: u32, exit_code: Option<i32>) {
            self.inner
                .lock()
                .expect("lock")
                .queued_exits
                .push(ProcessExit { pid, exit_code });
        }

        fn spawned_env(&self) -> Vec<BTreeMap<String, String>> {
            self.inner.lock().expect("lock").spawned_env.clone()
        }
    }

    impl ProcessController for FakeProcessController {
        fn spawn(
            &mut self,
            _spec: &ProcessSpec,
            extra_env: &BTreeMap<String, String>,
        ) -> Result<u32, String> {
            let mut inner = self.inner.lock().expect("lock");
            if inner.fail_spawn {
                return Err("spawn failed".to_string());
            }
            inner.spawned_env.push(extra_env.clone());
            inner.next_pid += 1;
            let pid = inner.next_pid;
            inner.running.insert(pid);
            Ok(pid)
        }

        fn terminate(&mut self, pid: u32) -> Result<(), String> {
            let mut inner = self.inner.lock().expect("lock");
            if inner.running.remove(&pid) {
                inner.terminated.push(pid);
                Ok(())
            } else {
                Err(format!("unknown pid: {pid}"))
            }
        }

        fn reap_exited(&mut self) -> Vec<ProcessExit> {
            let mut inner = self.inner.lock().expect("lock");
            std::mem::take(&mut inner.queued_exits)
        }
    }

    fn pane(id: &str, x: i32, y: i32, width: u32, height: u32) -> ProviderPaneSnapshot {
        ProviderPaneSnapshot {
            id: PaneId::new(id),
            geometry: PaneGeometry {
                x,
                y,
                width,
                height,
            },
        }
    }

    fn terminal_process() -> ProcessSpec {
        ProcessSpec {
            command: "foot".to_string(),
            args: vec![],
            cwd: None,
            env: BTreeMap::new(),
        }
    }

    fn overlay_region(
        region_id: &str,
        pane_id: &str,
        pane_instance_id: &str,
        rect: OverlayRect,
    ) -> OverlayRegionRequest {
        OverlayRegionRequest {
            region_id: region_id.to_string(),
            pane_id: PaneId::new(pane_id),
            pane_instance_id: pane_instance_id.to_string(),
            kind: CompositorOverlayKind::PaneBadge,
            rect,
            z_index: None,
            captures: vec![
                OverlayCaptureCapability::PointerHover,
                OverlayCaptureCapability::PointerButton,
                OverlayCaptureCapability::PointerAxis,
            ],
        }
    }

    fn main_app_intent() -> MainAppLaunchIntent {
        MainAppLaunchIntent {
            process: ProcessSpec {
                command: "foot".to_string(),
                args: vec!["--app-id".to_string(), "surf-ace-main".to_string()],
                cwd: None,
                env: BTreeMap::new(),
            },
            binding: MainAppSurfaceBinding::AppId {
                app_id: "surf-ace-main".to_string(),
            },
        }
    }

    #[test]
    fn provider_snapshot_is_geometry_authority() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 200, 100)])
            .expect("provider snapshot should apply");

        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");

        let status = state.status_snapshot();
        assert_eq!(status.panes[0].geometry.width, 200);
        assert_eq!(status.panes[0].geometry.height, 100);

        state
            .apply_provider_snapshot(vec![pane("p-1", 10, 5, 320, 180)])
            .expect("provider snapshot update should apply");
        let status = state.status_snapshot();
        assert_eq!(status.panes[0].geometry.width, 320);
        assert_eq!(status.panes[0].geometry.height, 180);
    }

    #[test]
    fn dynamic_switch_is_live_and_reversible() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");

        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");
        state
            .mark_external_surface_attached(&PaneId::new("p-1"))
            .expect("attach should work");

        let status = state.status_snapshot();
        assert!(matches!(
            status.panes[0].render_mode,
            PaneRenderMode::ExternalNative { .. }
        ));
        assert!(matches!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { .. }
        ));

        state
            .switch_pane_to_surf_ace(&PaneId::new("p-1"))
            .expect("revert to surf ace should work");
        let status = state.status_snapshot();
        assert!(matches!(
            status.panes[0].render_mode,
            PaneRenderMode::SurfAceRendered
        ));
        assert!(matches!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Absent
        ));
    }

    #[test]
    fn prototype_policy_allows_only_one_active_overlay_pane() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![
                pane("p-1", 0, 0, 100, 100),
                pane("p-2", 100, 0, 100, 100),
            ])
            .expect("provider snapshot should apply");

        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("first overlay pane should work");
        let err = state
            .switch_pane_to_external_native(
                &PaneId::new("p-2"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect_err("second concurrent overlay pane should be denied");

        assert!(matches!(err, StateError::PrototypePolicy(_)));
    }

    #[test]
    fn native_pane_host_plan_records_multiple_provider_owned_rectangles_without_overlay_claim() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("surf", 0, 720, 1280, 720)])
            .expect("existing Surf Ace pane should apply");

        state
            .apply_native_pane_host_plan(vec![
                NativePaneHostRequest {
                    id: PaneId::new("left"),
                    content_id: Some("content-left".to_string()),
                    binding_id: Some("binding-left".to_string()),
                    revision: 1,
                    geometry: PaneGeometry {
                        x: 0,
                        y: 0,
                        width: 640,
                        height: 720,
                    },
                    target: NativeTargetClass::Terminal,
                    process: terminal_process(),
                },
                NativePaneHostRequest {
                    id: PaneId::new("right"),
                    content_id: Some("content-right".to_string()),
                    binding_id: Some("binding-right".to_string()),
                    revision: 1,
                    geometry: PaneGeometry {
                        x: 640,
                        y: 0,
                        width: 640,
                        height: 720,
                    },
                    target: NativeTargetClass::Terminal,
                    process: ProcessSpec {
                        command: "ghostty".to_string(),
                        args: vec!["-e".to_string(), "top".to_string()],
                        cwd: None,
                        env: BTreeMap::new(),
                    },
                },
            ])
            .expect("provider-authored native pane plan should apply");

        let status = state.status_snapshot();
        assert_eq!(status.panes.len(), 3);
        assert_eq!(status.panes[0].id, PaneId::new("left"));
        assert_eq!(status.panes[0].geometry.width, 640);
        assert!(matches!(
            status.panes[0].render_mode,
            PaneRenderMode::ExternalNative { .. }
        ));
        assert!(matches!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Absent
        ));
        assert_eq!(status.panes[1].id, PaneId::new("right"));
        assert_eq!(status.panes[1].geometry.x, 640);
        assert!(matches!(
            status.panes[1].render_mode,
            PaneRenderMode::ExternalNative { .. }
        ));
        assert_eq!(status.panes[2].id, PaneId::new("surf"));
        assert!(matches!(
            status.panes[2].render_mode,
            PaneRenderMode::SurfAceRendered
        ));
        assert!(status.prototype_policy.active_overlay_pane.is_none());
    }

    #[test]
    fn native_pane_hosts_launch_and_report_lifecycle_per_pane() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1280,
            720,
        );
        state
            .apply_native_pane_host_plan(vec![
                NativePaneHostRequest {
                    id: PaneId::new("left"),
                    content_id: Some("content-left".to_string()),
                    binding_id: Some("binding-left".to_string()),
                    revision: 1,
                    geometry: PaneGeometry {
                        x: 0,
                        y: 0,
                        width: 640,
                        height: 720,
                    },
                    target: NativeTargetClass::Terminal,
                    process: terminal_process(),
                },
                NativePaneHostRequest {
                    id: PaneId::new("right"),
                    content_id: Some("content-right".to_string()),
                    binding_id: Some("binding-right".to_string()),
                    revision: 1,
                    geometry: PaneGeometry {
                        x: 640,
                        y: 0,
                        width: 640,
                        height: 720,
                    },
                    target: NativeTargetClass::Terminal,
                    process: ProcessSpec {
                        command: "ghostty".to_string(),
                        args: vec!["-e".to_string(), "top".to_string()],
                        cwd: None,
                        env: BTreeMap::new(),
                    },
                },
            ])
            .expect("native pane host plan should apply");

        state
            .launch_native_pane_hosts(Vec::new())
            .expect("empty launch set should launch all planned native panes");

        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Launching { pid: 1 }
        );
        assert_eq!(
            status.panes[1].external_native_state,
            ExternalNativeLifecycleState::Launching { pid: 2 }
        );
        assert_eq!(process_view.spawned_env().len(), 2);
        assert_eq!(
            process_view.spawned_env()[0].get("SURF_ACE_PANE_ID"),
            Some(&"left".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[0].get("WAYLAND_DISPLAY"),
            Some(&"wayland-77".to_string())
        );
        assert!(
            process_view.spawned_env()[0]
                .get(LAUNCH_TOKEN_ENV)
                .is_some_and(|token| token.starts_with("sa-launch-"))
        );
        assert_eq!(
            process_view.spawned_env()[0].get(NATIVE_PANE_CONTENT_ID_ENV),
            Some(&"content-left".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[0].get(NATIVE_PANE_BINDING_ID_ENV),
            Some(&"binding-left".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[0].get(NATIVE_PANE_REVISION_ENV),
            Some(&"1".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[1].get("SURF_ACE_PANE_ID"),
            Some(&"right".to_string())
        );
        assert_ne!(
            process_view.spawned_env()[0].get(LAUNCH_TOKEN_ENV),
            process_view.spawned_env()[1].get(LAUNCH_TOKEN_ENV)
        );

        state
            .mark_external_surface_attached(&PaneId::new("left"))
            .expect("left pane should attach independently");
        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { pid: 1 }
        );
        assert_eq!(
            status.panes[1].external_native_state,
            ExternalNativeLifecycleState::Launching { pid: 2 }
        );
    }

    #[test]
    fn native_pane_surface_reconciles_by_launched_pid_and_reports_evidence() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1280,
            720,
        );
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("left"),
                content_id: Some("content-left".to_string()),
                binding_id: Some("binding-left".to_string()),
                revision: 1,
                geometry: PaneGeometry {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 720,
                },
                target: NativeTargetClass::Terminal,
                process: terminal_process(),
            }])
            .expect("native pane host plan should apply");
        state
            .launch_native_pane_hosts(Vec::new())
            .expect("native pane should launch");

        let evidence = SurfaceBindingEvidence {
            app_id: Some("unexpected-terminal-id".to_string()),
            title: None,
            launch_token: None,
            outcome: SurfaceBindingEvidenceOutcome::NotRequired,
        };
        assert_eq!(
            state.runtime_mark_native_pane_surface_attached_for_pid(
                1,
                Some(101),
                Some(evidence.clone())
            ),
            Some(PaneId::new("left"))
        );

        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { pid: 1 }
        );
        assert_eq!(
            status.panes[0].external_native_binding_evidence,
            Some(evidence)
        );
        let native_host = status.panes[0]
            .native_host
            .as_ref()
            .expect("native host status should be present");
        assert_eq!(native_host.pane_id, PaneId::new("left"));
        assert_eq!(native_host.content_id.as_deref(), Some("content-left"));
        assert_eq!(native_host.binding_id.as_deref(), Some("binding-left"));
        assert_eq!(native_host.revision, 1);
        assert_eq!(native_host.surface_id, Some(101));
        assert!(
            state
                .runtime_mark_native_pane_surface_attached_for_pid(99, None, None)
                .is_none()
        );
    }

    #[test]
    fn native_pane_surface_tracks_descendant_client_pid() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("surface:2"),
                content_id: Some("ct_top".to_string()),
                binding_id: Some("surface:2:ct_top".to_string()),
                revision: 1,
                geometry: PaneGeometry {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                target: NativeTargetClass::Terminal,
                process: terminal_process(),
            }])
            .expect("native pane host plan should apply");
        state
            .launch_native_pane_hosts(Vec::new())
            .expect("native pane host should launch");

        assert_eq!(
            state.runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
                1,
                77,
                Some(303),
                None,
            ),
            Some(PaneId::new("surface:2"))
        );
        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { pid: 77 }
        );
        assert_eq!(
            status.panes[0]
                .native_host
                .as_ref()
                .expect("native host status should be present")
                .surface_id,
            Some(303)
        );
    }

    #[test]
    fn native_pane_launch_token_allows_late_detached_client_after_launcher_exit() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("surface:2"),
                content_id: Some("ct_top".to_string()),
                binding_id: Some("surface:2:ct_top:7".to_string()),
                revision: 7,
                geometry: PaneGeometry {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                target: NativeTargetClass::Terminal,
                process: terminal_process(),
            }])
            .expect("native pane host plan should apply");
        state
            .launch_native_pane_hosts(Vec::new())
            .expect("native pane host should launch");

        let token = process_view.spawned_env()[0]
            .get(LAUNCH_TOKEN_ENV)
            .expect("native pane launch token should be emitted")
            .clone();
        process_view.queue_exit(1, Some(0));
        state.poll_processes();
        assert_eq!(
            state.status_snapshot().panes[0].external_native_state,
            ExternalNativeLifecycleState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        );
        assert_eq!(
            state.runtime_expected_native_pane_bindings()[0].launch_token,
            Some(token)
        );

        let evidence = SurfaceBindingEvidence {
            app_id: Some("detached-terminal".to_string()),
            title: Some("top".to_string()),
            launch_token: Some(crate::model::LaunchTokenEvidence::Matched),
            outcome: SurfaceBindingEvidenceOutcome::NotRequired,
        };
        assert_eq!(
            state.runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
                1,
                77,
                Some(303),
                Some(evidence.clone()),
            ),
            Some(PaneId::new("surface:2"))
        );
        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { pid: 77 }
        );
        assert_eq!(
            status.panes[0].external_native_binding_evidence,
            Some(evidence)
        );
    }

    #[test]
    fn native_pane_surface_detach_clears_surface_truth_without_releasing_pane_plan() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("surface:2"),
                content_id: Some("ct_top".to_string()),
                binding_id: Some("surface:2:ct_top".to_string()),
                revision: 1,
                geometry: PaneGeometry {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                target: NativeTargetClass::Terminal,
                process: terminal_process(),
            }])
            .expect("native pane host plan should apply");
        state
            .launch_native_pane_hosts(Vec::new())
            .expect("native pane host should launch");
        assert_eq!(
            state.runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
                1,
                77,
                Some(303),
                None,
            ),
            Some(PaneId::new("surface:2"))
        );

        assert!(state.runtime_mark_native_pane_surface_detached_for_pid(77));
        let status = state.status_snapshot();
        assert_eq!(
            status.panes[0].external_native_state,
            ExternalNativeLifecycleState::Launching { pid: 77 }
        );
        let native_host = status.panes[0]
            .native_host
            .as_ref()
            .expect("native host status remains present for reattach/rehost");
        assert_eq!(native_host.surface_id, None);
        assert_eq!(native_host.content_id.as_deref(), Some("ct_top"));
        assert_eq!(native_host.binding_id.as_deref(), Some("surface:2:ct_top"));
    }

    #[test]
    fn external_native_mode_is_explicit_not_html() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");

        let status = state.status_snapshot();
        let as_json = serde_json::to_string(&status.panes[0].render_mode)
            .expect("render mode should serialize");
        assert!(as_json.contains("\"kind\":\"external_native\""));
        assert!(!as_json.contains("html"));
    }

    #[test]
    fn removing_pane_terminates_external_process() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");

        state
            .apply_provider_snapshot(vec![])
            .expect("removing pane snapshot should apply");
        assert_eq!(process_view.terminated(), vec![1]);
        assert!(state.status_snapshot().panes.is_empty());
        assert!(
            state
                .status_snapshot()
                .prototype_policy
                .active_overlay_pane
                .is_none()
        );
    }

    #[test]
    fn overlay_regions_store_region_data_only_and_clamp_to_runtime_bounds() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("pane should become native-hosted");
        state
            .mark_external_surface_attached(&PaneId::new("p-1"))
            .expect("native pane should attach");
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        let topology_epoch = state.topology_epoch().to_string();

        state
            .set_overlay_regions(
                "main-surface".to_string(),
                Some("window-1".to_string()),
                1,
                topology_epoch,
                Some(OverlayRegionUpdateReason::Initial),
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "badge",
                    "p-1",
                    "p-1:0",
                    OverlayRect {
                        x: -20.0,
                        y: 590.0,
                        width: 900.0,
                        height: 40.0,
                    },
                )],
            )
            .expect("valid overlay region should store");

        let status = state.status_snapshot();
        assert_eq!(
            status.prototype_policy.active_overlay_pane,
            Some(PaneId::new("p-1"))
        );
        assert!(status.runtime.overlay_bound_pane_id.is_none());
        assert_eq!(
            status.overlay_regions.surface_id.as_deref(),
            Some("main-surface")
        );
        assert_eq!(
            status.overlay_regions.window_id.as_deref(),
            Some("window-1")
        );
        assert_eq!(status.overlay_regions.active_revision, Some(1));
        assert_eq!(status.overlay_regions.region_count, 1);
        assert_eq!(
            status.overlay_regions.update_reason,
            Some(OverlayRegionUpdateReason::Initial)
        );
        assert!(status.overlay_regions.last_updated_at.is_some());
        assert_eq!(status.overlay_regions.regions.len(), 1);
        let region = &status.overlay_regions.regions[0];
        assert_eq!(region.region_id, "badge");
        assert_eq!(region.pane_id, PaneId::new("p-1"));
        assert_eq!(region.pane_instance_id, "p-1:0");
        assert_eq!(
            region.rect,
            OverlayRect {
                x: 0.0,
                y: 590.0,
                width: 800.0,
                height: 10.0,
            }
        );
        assert!(region.clamped);
    }

    #[test]
    fn overlay_regions_reject_empty_pane_or_empty_geometry() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("pane should become native-hosted");
        state
            .mark_external_surface_attached(&PaneId::new("p-1"))
            .expect("native pane should attach");
        let topology_epoch = state.topology_epoch().to_string();

        let empty_pane = state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                1,
                topology_epoch.clone(),
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "badge",
                    "  ",
                    "p-1:0",
                    OverlayRect {
                        x: 0.0,
                        y: 0.0,
                        width: 10.0,
                        height: 10.0,
                    },
                )],
            )
            .expect_err("empty pane id should be rejected");
        assert!(matches!(empty_pane, StateError::InvalidOverlayRegion(_)));

        let empty_rect = state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                1,
                topology_epoch,
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "badge",
                    "p-1",
                    "p-1:0",
                    OverlayRect {
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 10.0,
                    },
                )],
            )
            .expect_err("zero width should be rejected");
        assert!(matches!(empty_rect, StateError::InvalidOverlayRegion(_)));
        assert!(state.status_snapshot().overlay_regions.regions.is_empty());
    }

    #[test]
    fn overlay_regions_reject_unknown_panes_without_replacing_existing_set() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("left", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("left"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("pane should become native-hosted");
        state
            .mark_external_surface_attached(&PaneId::new("left"))
            .expect("native pane should attach");
        let topology_epoch = state.topology_epoch().to_string();
        state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                1,
                topology_epoch.clone(),
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "left-badge",
                    "left",
                    "left:0",
                    OverlayRect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                )],
            )
            .expect("known pane region should store");
        let before = state.status_snapshot().overlay_regions;

        let error = state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                2,
                topology_epoch,
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![
                    overlay_region(
                        "left-badge",
                        "left",
                        "left:0",
                        OverlayRect {
                            x: 10.0,
                            y: 10.0,
                            width: 50.0,
                            height: 50.0,
                        },
                    ),
                    overlay_region(
                        "missing-badge",
                        "missing",
                        "missing:0",
                        OverlayRect {
                            x: 0.0,
                            y: 0.0,
                            width: 10.0,
                            height: 10.0,
                        },
                    ),
                ],
            )
            .expect_err("unknown pane should reject the whole overlay region set");

        assert!(matches!(error, StateError::PaneNotFound(id) if id == PaneId::new("missing")));
        assert_eq!(state.status_snapshot().overlay_regions, before);
    }

    #[test]
    fn overlay_regions_reject_stale_revision_and_topology_epoch_without_mutation() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("left", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("left"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("pane should become native-hosted");
        state
            .mark_external_surface_attached(&PaneId::new("left"))
            .expect("native pane should attach");
        let epoch = state.topology_epoch().to_string();
        state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                7,
                epoch.clone(),
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "left-badge",
                    "left",
                    "left:0",
                    OverlayRect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                )],
            )
            .expect("initial region should store");
        let before = state.status_snapshot().overlay_regions;

        let stale_revision = state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                7,
                epoch.clone(),
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "left-badge",
                    "left",
                    "left:0",
                    OverlayRect {
                        x: 10.0,
                        y: 10.0,
                        width: 10.0,
                        height: 10.0,
                    },
                )],
            )
            .expect_err("same revision should be stale");
        assert!(matches!(
            stale_revision,
            StateError::StaleOverlayRegionRevision {
                incoming: 7,
                active: 7
            }
        ));
        assert_eq!(state.status_snapshot().overlay_regions, before);

        state
            .apply_provider_snapshot(vec![pane("left", 0, 0, 100, 100)])
            .expect("topology replay should advance epoch");
        let stale_epoch = state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                8,
                epoch,
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "left-badge",
                    "left",
                    "left:0",
                    OverlayRect {
                        x: 10.0,
                        y: 10.0,
                        width: 10.0,
                        height: 10.0,
                    },
                )],
            )
            .expect_err("old topology epoch should be stale");
        assert!(matches!(
            stale_epoch,
            StateError::StaleOverlayTopologyEpoch { .. }
        ));
        let after = state.status_snapshot().overlay_regions;
        assert_eq!(after.surface_id, before.surface_id);
        assert_eq!(after.window_id, before.window_id);
        assert_eq!(after.active_revision, before.active_revision);
        assert_eq!(after.region_count, before.region_count);
        assert_eq!(after.regions, before.regions);
        assert_eq!(
            after.topology_epoch.as_deref(),
            Some(state.topology_epoch())
        );
    }

    #[test]
    fn overlay_regions_clear_and_prune_removed_panes() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![
                pane("left", 0, 0, 100, 100),
                pane("right", 100, 0, 100, 100),
            ])
            .expect("provider snapshot should apply");
        state
            .apply_native_pane_host_plan(vec![
                NativePaneHostRequest {
                    id: PaneId::new("left"),
                    content_id: None,
                    binding_id: None,
                    revision: 0,
                    geometry: PaneGeometry {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                    },
                    target: NativeTargetClass::Terminal,
                    process: terminal_process(),
                },
                NativePaneHostRequest {
                    id: PaneId::new("right"),
                    content_id: None,
                    binding_id: None,
                    revision: 0,
                    geometry: PaneGeometry {
                        x: 100,
                        y: 0,
                        width: 100,
                        height: 100,
                    },
                    target: NativeTargetClass::Terminal,
                    process: terminal_process(),
                },
            ])
            .expect("native pane host plan should apply");
        state
            .launch_native_pane_hosts(Vec::new())
            .expect("native panes should launch");
        state
            .mark_external_surface_attached(&PaneId::new("left"))
            .expect("left native surface should attach");
        state
            .mark_external_surface_attached(&PaneId::new("right"))
            .expect("right native surface should attach");
        let topology_epoch = state.topology_epoch().to_string();
        state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                1,
                topology_epoch,
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![
                    overlay_region(
                        "left-badge",
                        "left",
                        "left:0",
                        OverlayRect {
                            x: 0.0,
                            y: 0.0,
                            width: 100.0,
                            height: 100.0,
                        },
                    ),
                    overlay_region(
                        "right-badge",
                        "right",
                        "right:0",
                        OverlayRect {
                            x: 100.0,
                            y: 0.0,
                            width: 100.0,
                            height: 100.0,
                        },
                    ),
                ],
            )
            .expect("overlay regions should store");

        assert!(state.runtime_mark_native_pane_surface_detached_for_pid(1));
        assert_eq!(state.status_snapshot().overlay_regions.regions.len(), 1);
        assert_eq!(
            state.status_snapshot().overlay_regions.regions[0].pane_id,
            PaneId::new("right")
        );

        state
            .apply_provider_snapshot(vec![pane("left", 0, 0, 100, 100)])
            .expect("provider snapshot should prune stale right region");
        assert!(state.status_snapshot().overlay_regions.regions.is_empty());

        state
            .mark_external_surface_attached(&PaneId::new("left"))
            .expect("left native surface should reattach");
        let topology_epoch = state.topology_epoch().to_string();
        state
            .set_overlay_regions(
                "main-surface".to_string(),
                None,
                1,
                topology_epoch,
                None,
                OverlayCoordinateSpace::SurfaceLogical,
                vec![overlay_region(
                    "left-badge",
                    "left",
                    "left:0",
                    OverlayRect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                )],
            )
            .expect("overlay region should store again");
        state
            .clear_overlay_regions("main-surface".to_string(), None)
            .expect("empty clear set should clear all regions");
        assert!(state.status_snapshot().overlay_regions.regions.is_empty());
    }

    #[test]
    fn failed_spawn_records_external_native_failed_state() {
        let process = FakeProcessController::default().with_fail_spawn(true);
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");

        let error = state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect_err("spawn should fail");
        assert!(matches!(error, StateError::Process(_)));
        assert!(matches!(
            state.status_snapshot().panes[0].external_native_state,
            ExternalNativeLifecycleState::Failed { .. }
        ));
    }

    #[test]
    fn missing_pane_does_not_reserve_overlay_slot() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");

        let error = state
            .switch_pane_to_external_native(
                &PaneId::new("does-not-exist"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect_err("missing pane should fail");
        assert!(matches!(error, StateError::PaneNotFound(_)));
        assert!(
            state
                .status_snapshot()
                .prototype_policy
                .active_overlay_pane
                .is_none()
        );
    }

    #[test]
    fn shell_overlay_toggle_launches_without_claiming_attached_binding_truth() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1280,
            800,
        );
        state.configure_shell_overlay_process(Some(terminal_process()));
        state.set_shell_overlay_toggle_shortcut("Super+`");

        state
            .toggle_shell_overlay()
            .expect("shell overlay launch should succeed");

        assert_eq!(
            state
                .status_snapshot()
                .runtime
                .shell_overlay_toggle_shortcut,
            "Super+`"
        );
        assert_eq!(
            state.status_snapshot().prototype_policy.active_overlay_pane,
            Some(PaneId::new(SHELL_OVERLAY_PANE_ID))
        );
        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Launching { pid: 1 }
        ));
        assert_eq!(
            state.runtime_expected_overlay_binding(),
            Some((PaneId::new(SHELL_OVERLAY_PANE_ID), 1))
        );
        assert!(
            state
                .status_snapshot()
                .runtime
                .overlay_bound_pane_id
                .is_none()
        );
        assert!(state.shell_overlay_focus_requested());
        assert_eq!(
            process_view.spawned_env(),
            vec![BTreeMap::from([
                ("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string()),
                (
                    "SURF_ACE_PANE_ID".to_string(),
                    SHELL_OVERLAY_PANE_ID.to_string()
                ),
                ("WAYLAND_DISPLAY".to_string(), "wayland-77".to_string()),
            ])]
        );
    }

    #[test]
    fn shell_overlay_bridge_requires_expected_pid_and_allows_reopen_after_exit() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state.configure_shell_overlay_process(Some(terminal_process()));

        state
            .toggle_shell_overlay()
            .expect("shell overlay launch should succeed");
        assert!(!state.runtime_mark_overlay_surface_attached_for_pid(99));
        assert!(state.runtime_mark_overlay_surface_attached_for_pid(1));
        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Attached { pid: 1 }
        ));

        process_view.queue_exit(1, Some(0));
        state.poll_processes();
        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        ));
        assert!(!state.shell_overlay_focus_requested());
        assert!(state.runtime_expected_overlay_binding().is_none());
        assert!(
            state
                .status_snapshot()
                .runtime
                .overlay_bound_pane_id
                .is_none()
        );

        state
            .toggle_shell_overlay()
            .expect("shell overlay should reopen after exit");
        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Launching { pid: 2 }
        ));
        assert_eq!(
            state.runtime_expected_overlay_binding(),
            Some((PaneId::new(SHELL_OVERLAY_PANE_ID), 2))
        );
    }

    #[test]
    fn shell_overlay_toggle_dismisses_cleanly_and_returns_focus_to_main_app() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state.configure_shell_overlay_process(Some(terminal_process()));
        state
            .toggle_shell_overlay()
            .expect("shell overlay launch should succeed");
        state.set_runtime_surface_roles(Some(101), None, None);
        state.set_runtime_focus_target(Some(RuntimeFocusTarget::OverlayNative));

        state
            .toggle_shell_overlay()
            .expect("second toggle should dismiss shell overlay");

        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Absent
        ));
        assert_eq!(process_view.terminated(), vec![1]);
        assert!(
            state
                .status_snapshot()
                .prototype_policy
                .active_overlay_pane
                .is_none()
        );
        assert!(!state.shell_overlay_focus_requested());
        assert_eq!(
            state.status_snapshot().runtime.active_focus_target,
            Some(RuntimeFocusTarget::MainApp)
        );
    }

    #[test]
    fn main_app_launch_intent_waits_for_runtime_then_spawns_with_exact_contract_env() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));

        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::WaitingForRuntime
        );

        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1280,
            800,
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.main_app_launch_intent, Some(main_app_intent()));
        assert_eq!(
            runtime.main_app_launch_state,
            MainAppLaunchState::Launching { pid: 1 }
        );
        assert_eq!(runtime.main_app_surface_id, None);
        assert_eq!(process_view.spawned_env().len(), 1);
        assert_eq!(
            process_view.spawned_env()[0].get("SURF_ACE_COMPOSITOR_HOST_MODE"),
            Some(&"1".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[0].get("SURF_ACE_COMPOSITOR_MAIN_APP"),
            Some(&"1".to_string())
        );
        assert_eq!(
            process_view.spawned_env()[0].get("WAYLAND_DISPLAY"),
            Some(&"wayland-77".to_string())
        );
        assert!(
            process_view.spawned_env()[0]
                .get(LAUNCH_TOKEN_ENV)
                .is_some_and(|token| token.starts_with("sa-launch-"))
        );
        assert_eq!(
            state.runtime_expected_main_app_binding(),
            Some((
                1,
                MainAppSurfaceBinding::AppId {
                    app_id: "surf-ace-main".to_string()
                }
            ))
        );
    }

    #[test]
    fn main_app_surface_binding_requires_expected_pid_and_tracks_detach() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );

        assert!(!state.runtime_mark_main_app_surface_attached_for_pid(99));
        assert!(state.runtime_mark_main_app_surface_attached_for_pid(1));
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 1 }
        );
        assert!(state.runtime_mark_main_app_surface_detached_for_pid(1));
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Launching { pid: 1 }
        );

        process_view.queue_exit(1, Some(0));
        state.poll_processes();
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        );
    }

    #[test]
    fn main_app_surface_binding_accepts_launched_descendant_client_pid() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );

        assert!(
            state.runtime_mark_main_app_surface_attached_for_launch_pid_with_evidence(1, 77, None,)
        );
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 77 }
        );
        assert_eq!(
            state.runtime_expected_main_app_binding(),
            Some((
                77,
                MainAppSurfaceBinding::AppId {
                    app_id: "surf-ace-main".to_string()
                }
            ))
        );

        process_view.queue_exit(1, Some(0));
        state.poll_processes();
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 77 }
        );
    }

    #[test]
    fn main_app_launch_token_allows_late_detached_client_after_launcher_exit() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );

        let token = process_view.spawned_env()[0]
            .get(LAUNCH_TOKEN_ENV)
            .expect("main app launch token should be emitted")
            .clone();
        process_view.queue_exit(1, Some(0));
        state.poll_processes();
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        );
        assert_eq!(
            state
                .runtime_expected_main_app_binding_with_token()
                .expect("exited launcher should keep token expectation")
                .launch_token,
            Some(token)
        );

        let evidence = SurfaceBindingEvidence {
            app_id: Some("detached.app".to_string()),
            title: Some("Detached".to_string()),
            launch_token: Some(crate::model::LaunchTokenEvidence::Matched),
            outcome: SurfaceBindingEvidenceOutcome::MismatchesIntent,
        };
        assert!(
            state.runtime_mark_main_app_surface_attached_for_launch_pid_with_evidence(
                1,
                77,
                Some(evidence.clone()),
            )
        );
        let runtime = state.status_snapshot().runtime;
        assert_eq!(
            runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 77 }
        );
        assert_eq!(runtime.main_app_binding_evidence, Some(evidence));
    }

    #[test]
    fn launched_main_app_records_binding_evidence_without_making_app_id_authority() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );

        let evidence = SurfaceBindingEvidence {
            app_id: Some("com.mitchellh.ghostty".to_string()),
            title: Some("top".to_string()),
            launch_token: None,
            outcome: SurfaceBindingEvidenceOutcome::MismatchesIntent,
        };

        assert!(
            state.runtime_mark_main_app_surface_attached_for_pid_with_evidence(
                1,
                Some(evidence.clone())
            )
        );
        let runtime = state.status_snapshot().runtime;
        assert_eq!(
            runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 1 }
        );
        assert_eq!(runtime.main_app_binding_evidence, Some(evidence));
    }

    #[test]
    fn exited_main_app_clears_main_focus_and_shell_overlay_can_open_for_recovery() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state.configure_shell_overlay_process(Some(terminal_process()));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-55".to_string()),
            1280,
            800,
        );
        assert!(state.runtime_mark_main_app_surface_attached_for_pid(1));
        state.set_runtime_surface_roles(Some(101), None, None);
        state.set_runtime_focus_target(Some(RuntimeFocusTarget::MainApp));

        process_view.queue_exit(1, Some(0));
        state.poll_processes();

        let runtime = state.status_snapshot().runtime;
        assert_eq!(
            runtime.main_app_launch_state,
            MainAppLaunchState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        );
        assert!(runtime.main_app_surface_id.is_none());
        assert!(runtime.active_focus_target.is_none());

        state
            .toggle_shell_overlay()
            .expect("shell overlay remains available for recovery after main app exit");
        assert!(matches!(
            state.shell_overlay_lifecycle,
            ExternalNativeLifecycleState::Launching { pid: 2 }
        ));
        assert_eq!(
            state.runtime_expected_overlay_binding(),
            Some((PaneId::new(SHELL_OVERLAY_PANE_ID), 2))
        );
        assert!(state.shell_overlay_focus_requested());
    }

    #[test]
    fn selecting_new_main_app_intent_terminates_previous_running_process() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );
        assert!(state.runtime_mark_main_app_surface_attached_for_pid(1));

        let replacement = MainAppLaunchIntent {
            process: ProcessSpec {
                command: "foot".to_string(),
                args: vec!["--app-id".to_string(), "surf-ace-next".to_string()],
                cwd: None,
                env: BTreeMap::new(),
            },
            binding: MainAppSurfaceBinding::AppId {
                app_id: "surf-ace-next".to_string(),
            },
        };
        state
            .select_main_app_launch_intent(replacement.clone())
            .expect("replacement main app intent should be accepted");

        assert_eq!(process_view.terminated(), vec![1]);
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_intent,
            Some(replacement)
        );
        assert_eq!(
            state.status_snapshot().runtime.main_app_launch_state,
            MainAppLaunchState::Launching { pid: 2 }
        );
        assert!(
            state
                .status_snapshot()
                .runtime
                .main_app_surface_id
                .is_none()
        );
    }

    #[test]
    fn runtime_status_transitions_are_explicit() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));

        state.mark_runtime_starting(RuntimeBackend::Winit);
        state
            .select_main_app_launch_intent(main_app_intent())
            .expect("main app intent should be accepted");
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );
        state.mark_runtime_input_event();
        state.mark_runtime_redraw();
        state.mark_runtime_resize(1024, 600);
        state.set_runtime_host_backend_snapshot(
            Some("seat0".to_string()),
            2,
            1,
            Some("/dev/dri/card0".to_string()),
        );
        state.set_runtime_surface_roles(Some(101), Some(202), Some(PaneId::new("pane-1")));
        assert!(state.runtime_mark_main_app_surface_attached_for_pid(1));
        state.set_runtime_focus_target(Some(RuntimeFocusTarget::OverlayNative));
        state.increment_runtime_denied_toplevel();

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::Winit);
        assert_eq!(runtime.phase, RuntimePhase::Running);
        assert_eq!(runtime.wayland_socket.as_deref(), Some("wayland-55"));
        assert_eq!(runtime.main_app_launch_intent, Some(main_app_intent()));
        assert_eq!(
            runtime.main_app_launch_state,
            MainAppLaunchState::Attached { pid: 1 }
        );
        assert_eq!(runtime.window_width, Some(1024));
        assert_eq!(runtime.window_height, Some(600));
        assert_eq!(runtime.redraw_count, 1);
        assert_eq!(runtime.input_event_count, 1);
        assert_eq!(runtime.host_seat_name.as_deref(), Some("seat0"));
        assert_eq!(runtime.host_detected_drm_device_count, 2);
        assert_eq!(runtime.host_opened_drm_device_count, 1);
        assert!(!runtime.host_output_ownership);
        assert_eq!(runtime.host_start_attempt_count, 0);
        assert!(!runtime.host_start_request_pending);
        assert!(runtime.host_last_start_trigger.is_none());
        assert_eq!(
            runtime.host_primary_drm_path.as_deref(),
            Some("/dev/dri/card0")
        );
        assert_eq!(runtime.main_app_surface_id, Some(101));
        assert_eq!(runtime.overlay_surface_id, Some(202));
        assert_eq!(runtime.overlay_bound_pane_id, Some(PaneId::new("pane-1")));
        assert_eq!(
            runtime.active_focus_target,
            Some(RuntimeFocusTarget::OverlayNative)
        );
        assert_eq!(runtime.denied_toplevel_count, 1);
    }

    #[test]
    fn runtime_overlay_bridge_transitions_follow_surface_lifecycle_with_pid_attestation() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");

        assert!(!state.runtime_mark_overlay_surface_attached_for_pid(99));
        assert!(state.runtime_mark_overlay_surface_attached_for_pid(1));
        assert!(matches!(
            state.status_snapshot().panes[0].external_native_state,
            ExternalNativeLifecycleState::Attached { .. }
        ));

        assert!(!state.runtime_mark_overlay_surface_detached_for_pid(99));
        assert!(state.runtime_mark_overlay_surface_detached_for_pid(1));
        assert!(matches!(
            state.status_snapshot().panes[0].external_native_state,
            ExternalNativeLifecycleState::Launching { .. }
        ));
    }

    #[test]
    fn host_preflight_ready_is_not_running_or_output_owned() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
        state.mark_runtime_host_preflight_ready(Some("wayland-77".to_string()));
        state.set_runtime_host_backend_snapshot(
            Some("seat0".to_string()),
            2,
            1,
            Some("/dev/dri/card0".to_string()),
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.phase, RuntimePhase::PreflightReady);
        assert_eq!(runtime.wayland_socket.as_deref(), Some("wayland-77"));
        assert_eq!(runtime.host_opened_drm_device_count, 1);
        assert!(!runtime.host_output_ownership);
        assert_eq!(runtime.host_start_attempt_count, 1);
        assert!(!runtime.host_start_request_pending);
        assert_eq!(
            runtime.host_last_start_trigger,
            Some(HostRuntimeStartTrigger::Bootstrap)
        );
    }

    #[test]
    fn host_start_request_tracking_is_monotonic_and_explicit() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
        state.mark_runtime_failed("forced");
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::ControlRetry);

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.host_start_attempt_count, 2);
        assert!(runtime.host_start_request_pending);
        assert_eq!(
            runtime.host_last_start_trigger,
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
        assert_eq!(runtime.last_error.as_deref(), Some("forced"));
    }

    #[test]
    fn runtime_failure_clears_stale_runtime_bindings_and_requires_operator_recovery() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1280,
            800,
        );
        state.set_runtime_surface_roles(Some(101), Some(202), Some(PaneId::new("pane-1")));
        state.set_runtime_focus_target(Some(RuntimeFocusTarget::OverlayNative));
        state.set_runtime_host_present_capabilities(
            RuntimeHostPresentOwnership::DirectGbm,
            true,
            true,
        );

        state.mark_runtime_failed("forced");

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.phase, RuntimePhase::Failed);
        assert!(runtime.runtime_operator_action_needed);
        assert_eq!(runtime.last_error.as_deref(), Some("forced"));
        assert!(runtime.wayland_socket.is_none());
        assert!(runtime.window_width.is_none());
        assert!(runtime.window_height.is_none());
        assert!(runtime.main_app_surface_id.is_none());
        assert!(runtime.overlay_surface_id.is_none());
        assert!(runtime.overlay_bound_pane_id.is_none());
        assert!(runtime.active_focus_target.is_none());
        assert_eq!(
            runtime.host_present_ownership,
            RuntimeHostPresentOwnership::None
        );
    }

    #[test]
    fn runtime_starting_clears_pending_retry_but_preserves_last_failure_until_running() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_failed("forced");
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::ControlRetry);

        state.mark_runtime_starting(RuntimeBackend::HostDrm);

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.phase, RuntimePhase::Starting);
        assert!(!runtime.host_start_request_pending);
        assert_eq!(runtime.last_error.as_deref(), Some("forced"));
        assert!(!runtime.runtime_operator_action_needed);
    }

    #[test]
    fn host_preflight_ready_preserves_last_failure_until_running() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state.mark_runtime_failed("forced");
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::ControlRetry);
        state.mark_runtime_starting(RuntimeBackend::HostDrm);

        state.mark_runtime_host_preflight_ready(Some("wayland-77".to_string()));

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.phase, RuntimePhase::PreflightReady);
        assert_eq!(runtime.wayland_socket.as_deref(), Some("wayland-77"));
        assert_eq!(runtime.last_error.as_deref(), Some("forced"));
        assert!(!runtime.host_start_request_pending);
        assert!(!runtime.host_output_ownership);
        assert!(!runtime.runtime_operator_action_needed);

        state.mark_runtime_host_running(
            "wayland-77".to_string(),
            1280,
            800,
            Some("seat0".to_string()),
            2,
            1,
            Some("/dev/dri/card0".to_string()),
            Some("HDMI-A-1".to_string()),
            Some(7),
            Some("selected connector HDMI-A-1".to_string()),
            Some("claimed output ownership on HDMI-A-1".to_string()),
            RuntimeHostPresentOwnership::DirectGbm,
            true,
            true,
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.phase, RuntimePhase::Running);
        assert_eq!(runtime.last_error, None);
        assert!(runtime.host_output_ownership);
        assert_eq!(runtime.host_seat_name.as_deref(), Some("seat0"));
        assert_eq!(
            runtime.host_primary_drm_path.as_deref(),
            Some("/dev/dri/card0")
        );
        assert_eq!(
            runtime.host_active_connector_name.as_deref(),
            Some("HDMI-A-1")
        );
        assert_eq!(runtime.host_active_connector_id, Some(7));
        assert_eq!(
            runtime.host_last_selection_result.as_deref(),
            Some("claimed output ownership on HDMI-A-1")
        );
        assert_eq!(
            runtime.host_present_ownership,
            RuntimeHostPresentOwnership::DirectGbm
        );
        assert!(runtime.host_atomic_commit_enabled);
        assert!(runtime.host_overlay_plane_capable);
    }

    #[test]
    fn runtime_dmabuf_protocol_status_is_explicit_and_fail_closed() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));

        state.set_runtime_dmabuf_protocol_formats(vec![RuntimeDmabufFormatStatus {
            code: 0x34325241,
            modifier: 0,
        }]);
        let runtime = state.status_snapshot().runtime;
        assert!(runtime.dmabuf_protocol_enabled);
        assert_eq!(runtime.dmabuf_protocol_formats.len(), 1);

        state.mark_runtime_failed("forced");
        let runtime = state.status_snapshot().runtime;
        assert!(!runtime.dmabuf_protocol_enabled);
        assert!(runtime.dmabuf_protocol_formats.is_empty());
    }

    #[test]
    fn runtime_host_present_queue_status_is_explicit_and_fail_closed() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));

        state.set_runtime_host_present_capabilities(
            RuntimeHostPresentOwnership::DirectGbm,
            true,
            true,
        );
        state.set_runtime_last_queued_present(
            RuntimeHostQueuedPresentSource::DirectGbm,
            true,
            true,
            Some(RuntimeDmabufFormatStatus {
                code: 0x34325258,
                modifier: 0,
            }),
            Some(RuntimeDmabufFormatStatus {
                code: 0x34325241,
                modifier: 0,
            }),
        );
        let runtime = state.status_snapshot().runtime;
        assert_eq!(
            runtime.host_present_ownership,
            RuntimeHostPresentOwnership::DirectGbm
        );
        assert!(runtime.host_atomic_commit_enabled);
        assert!(runtime.host_overlay_plane_capable);
        assert_eq!(
            runtime.host_last_queued_present_source,
            RuntimeHostQueuedPresentSource::DirectGbm
        );
        assert!(runtime.host_last_queued_atomic_commit);
        assert!(runtime.host_last_queued_overlay_plane);
        assert_eq!(
            runtime.host_last_queued_primary_dmabuf_format,
            Some(RuntimeDmabufFormatStatus {
                code: 0x34325258,
                modifier: 0
            })
        );
        assert_eq!(
            runtime.host_last_queued_overlay_dmabuf_format,
            Some(RuntimeDmabufFormatStatus {
                code: 0x34325241,
                modifier: 0
            })
        );

        state.mark_runtime_stopped();
        let runtime = state.status_snapshot().runtime;
        assert_eq!(
            runtime.host_present_ownership,
            RuntimeHostPresentOwnership::None
        );
        assert!(!runtime.host_atomic_commit_enabled);
        assert!(!runtime.host_overlay_plane_capable);
        assert_eq!(
            runtime.host_last_queued_present_source,
            RuntimeHostQueuedPresentSource::None
        );
        assert!(!runtime.host_last_queued_atomic_commit);
        assert!(!runtime.host_last_queued_overlay_plane);
        assert!(runtime.host_last_queued_primary_dmabuf_format.is_none());
        assert!(runtime.host_last_queued_overlay_dmabuf_format.is_none());
    }

    #[test]
    fn runtime_overlay_binding_expected_tracks_active_overlay_pane_state() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");

        assert!(!state.runtime_overlay_binding_expected());
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");
        assert!(state.runtime_overlay_binding_expected());

        state
            .switch_pane_to_surf_ace(&PaneId::new("p-1"))
            .expect("switch back should work");
        assert!(!state.runtime_overlay_binding_expected());
    }

    #[test]
    fn poll_process_exit_marks_overlay_exited_and_clears_binding_expectation() {
        let process = FakeProcessController::default();
        let process_view = process.clone();
        let mut state = CompositorState::new(true, Box::new(process));
        state
            .apply_provider_snapshot(vec![pane("p-1", 0, 0, 100, 100)])
            .expect("provider snapshot should apply");
        state
            .switch_pane_to_external_native(
                &PaneId::new("p-1"),
                NativeTargetClass::Terminal,
                terminal_process(),
            )
            .expect("external switch should work");
        assert!(state.runtime_overlay_binding_expected());
        assert!(state.runtime_mark_overlay_surface_attached_for_pid(1));
        assert!(state.runtime_overlay_binding_expected());

        process_view.queue_exit(1, Some(0));
        state.poll_processes();

        assert!(matches!(
            state.status_snapshot().panes[0].external_native_state,
            ExternalNativeLifecycleState::Exited {
                pid: 1,
                exit_code: Some(0)
            }
        ));
        assert!(state.runtime_expected_overlay_binding().is_none());
        assert!(!state.runtime_overlay_binding_expected());
        assert!(!state.runtime_mark_overlay_surface_attached_for_pid(1));
        assert!(!state.runtime_mark_overlay_surface_detached_for_pid(1));
    }
}
