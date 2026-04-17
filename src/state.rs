use crate::model::{
    ExternalNativeEventContract, ExternalNativeLifecycleState, HostRuntimeStartTrigger,
    NativeTargetClass, OutputRotation, PaneId, PaneRenderMode, PaneStatus, ProcessSpec,
    ProviderPaneSnapshot, RuntimeBackend, RuntimeDmabufFormatStatus, RuntimeFocusTarget,
    RuntimeHostPresentOwnership, RuntimeHostQueuedPresentSource, RuntimeHostSelectionState,
    RuntimePhase, RuntimeSelectionMode, RuntimeStatus, StatusSnapshot,
};
use crate::policy::{PrototypeOverlayPolicy, PrototypePolicyError};
use crate::process_manager::ProcessController;
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneRuntimeState {
    geometry: crate::model::PaneGeometry,
    render_mode: PaneRenderMode,
    external_native_state: ExternalNativeLifecycleState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    #[error("pane not found: {0:?}")]
    PaneNotFound(PaneId),
    #[error("invalid process spec: command must not be empty")]
    InvalidProcessSpec,
    #[error("pane is not in external/native mode: {0:?}")]
    PaneNotExternalNative(PaneId),
    #[error("cannot switch pane to external/native mode: {0:?}")]
    AlreadyExternalNative(PaneId),
    #[error("{0}")]
    PrototypePolicy(#[from] PrototypePolicyError),
    #[error("{0}")]
    Process(String),
}

pub struct CompositorState {
    host_mode_active: bool,
    output_rotation: OutputRotation,
    panes: HashMap<PaneId, PaneRuntimeState>,
    prototype_overlay_policy: PrototypeOverlayPolicy,
    runtime: RuntimeStatus,
    process_controller: Box<dyn ProcessController>,
}

impl CompositorState {
    pub fn new(host_mode_active: bool, process_controller: Box<dyn ProcessController>) -> Self {
        Self {
            host_mode_active,
            output_rotation: OutputRotation::Deg0,
            panes: HashMap::new(),
            prototype_overlay_policy: PrototypeOverlayPolicy::default(),
            runtime: RuntimeStatus::default(),
            process_controller,
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

    pub fn runtime_main_app_match_hint(&self) -> &str {
        &self.runtime.main_app_match_hint
    }

    pub fn set_runtime_main_app_match_hint(&mut self, hint: impl Into<String>) {
        self.runtime.main_app_match_hint = hint.into();
    }

    pub fn mark_runtime_starting(&mut self, backend: RuntimeBackend) {
        self.runtime.backend = backend;
        self.runtime.phase = RuntimePhase::Starting;
        self.runtime.last_error = None;
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

    pub fn mark_host_runtime_start_requested(&mut self, trigger: HostRuntimeStartTrigger) {
        self.mark_runtime_starting(RuntimeBackend::HostDrm);
        self.runtime.host_start_attempt_count =
            self.runtime.host_start_attempt_count.saturating_add(1);
        self.runtime.host_last_start_trigger = Some(trigger);
    }

    pub fn mark_runtime_host_preflight_ready(&mut self, wayland_socket: Option<String>) {
        self.runtime.backend = RuntimeBackend::HostDrm;
        self.runtime.phase = RuntimePhase::PreflightReady;
        self.runtime.wayland_socket = wayland_socket;
        self.runtime.last_error = None;
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
        self.runtime.wayland_socket = wayland_socket;
        self.runtime.window_width = Some(window_width);
        self.runtime.window_height = Some(window_height);
        self.runtime.last_error = None;
        self.runtime.host_output_ownership = matches!(backend, RuntimeBackend::HostDrm);
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

    pub fn set_runtime_focus_target(&mut self, target: Option<RuntimeFocusTarget>) {
        self.runtime.active_focus_target = target;
    }

    pub fn increment_runtime_denied_toplevel(&mut self) {
        self.runtime.denied_toplevel_count += 1;
    }

    pub fn mark_runtime_failed(&mut self, error: impl Into<String>) {
        self.runtime.phase = RuntimePhase::Failed;
        self.runtime.last_error = Some(error.into());
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

    pub fn mark_runtime_stopped(&mut self) {
        self.runtime.phase = RuntimePhase::Stopped;
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

    pub fn apply_provider_snapshot(
        &mut self,
        provider_panes: Vec<ProviderPaneSnapshot>,
    ) -> Result<(), StateError> {
        let mut incoming = HashMap::new();
        for pane in provider_panes {
            let previous = self.panes.get(&pane.id);
            let runtime = match previous {
                Some(prev) => PaneRuntimeState {
                    geometry: pane.geometry,
                    render_mode: prev.render_mode.clone(),
                    external_native_state: prev.external_native_state.clone(),
                },
                None => PaneRuntimeState {
                    geometry: pane.geometry,
                    render_mode: PaneRenderMode::SurfAceRendered,
                    external_native_state: ExternalNativeLifecycleState::Absent,
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
        self.prototype_overlay_policy
            .clear_if_removed(|pane_id| self.panes.contains_key(pane_id));
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

        let pane = self
            .panes
            .get_mut(pane_id)
            .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;

        pane.render_mode = PaneRenderMode::ExternalNative {
            target,
            process: process.clone(),
        };

        let mut extra_env = BTreeMap::new();
        extra_env.insert("SURF_ACE_COMPOSITOR_HOST_MODE".to_string(), "1".to_string());
        extra_env.insert("SURF_ACE_PANE_ID".to_string(), pane_id.0.clone());

        match self.process_controller.spawn(&process, &extra_env) {
            Ok(pid) => {
                pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
                Ok(())
            }
            Err(err) => {
                pane.external_native_state = ExternalNativeLifecycleState::Failed {
                    reason: err.clone(),
                };
                Err(StateError::Process(err))
            }
        }
    }

    pub fn mark_external_surface_attached(&mut self, pane_id: &PaneId) -> Result<(), StateError> {
        let pane = self
            .panes
            .get_mut(pane_id)
            .ok_or_else(|| StateError::PaneNotFound(pane_id.clone()))?;

        let ExternalNativeLifecycleState::Launching { pid } = pane.external_native_state else {
            return Err(StateError::PaneNotExternalNative(pane_id.clone()));
        };

        pane.external_native_state = ExternalNativeLifecycleState::Attached { pid };
        Ok(())
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
        self.prototype_overlay_policy.release_if_matches(pane_id);
        Ok(())
    }

    pub fn active_overlay_pane_id(&self) -> Option<PaneId> {
        self.prototype_overlay_policy.active_overlay_pane().cloned()
    }

    pub fn runtime_expected_overlay_binding(&self) -> Option<(PaneId, u32)> {
        let pane_id = self.active_overlay_pane_id()?;
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
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };

        match pane.external_native_state {
            ExternalNativeLifecycleState::Launching { pid } if pid == client_pid => {
                pane.external_native_state = ExternalNativeLifecycleState::Attached { pid };
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
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };

        match pane.external_native_state {
            ExternalNativeLifecycleState::Attached { pid } if pid == client_pid => {
                pane.external_native_state = ExternalNativeLifecycleState::Launching { pid };
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
        for pane in self.panes.values_mut() {
            if running_pid(&pane.external_native_state) == Some(pid) {
                pane.external_native_state =
                    ExternalNativeLifecycleState::Exited { pid, exit_code };
            }
        }
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
            prototype_policy: self.prototype_overlay_policy.status(),
            runtime: self.runtime.clone(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PaneGeometry, ProviderPaneSnapshot};
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
    }

    impl ProcessController for FakeProcessController {
        fn spawn(
            &mut self,
            _spec: &ProcessSpec,
            _extra_env: &BTreeMap<String, String>,
        ) -> Result<u32, String> {
            let mut inner = self.inner.lock().expect("lock");
            if inner.fail_spawn {
                return Err("spawn failed".to_string());
            }
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
    fn runtime_status_transitions_are_explicit() {
        let process = FakeProcessController::default();
        let mut state = CompositorState::new(true, Box::new(process));

        state.mark_runtime_starting(RuntimeBackend::Winit);
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-55".to_string()),
            1280,
            800,
        );
        state.mark_runtime_input_event();
        state.mark_runtime_redraw();
        state.mark_runtime_resize(1024, 600);
        state.set_runtime_main_app_match_hint("surf");
        state.set_runtime_host_backend_snapshot(
            Some("seat0".to_string()),
            2,
            1,
            Some("/dev/dri/card0".to_string()),
        );
        state.set_runtime_surface_roles(Some(101), Some(202), Some(PaneId::new("pane-1")));
        state.set_runtime_focus_target(Some(RuntimeFocusTarget::OverlayNative));
        state.increment_runtime_denied_toplevel();

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::Winit);
        assert_eq!(runtime.phase, RuntimePhase::Running);
        assert_eq!(runtime.wayland_socket.as_deref(), Some("wayland-55"));
        assert_eq!(runtime.main_app_match_hint, "surf");
        assert_eq!(runtime.window_width, Some(1024));
        assert_eq!(runtime.window_height, Some(600));
        assert_eq!(runtime.redraw_count, 1);
        assert_eq!(runtime.input_event_count, 1);
        assert_eq!(runtime.host_seat_name.as_deref(), Some("seat0"));
        assert_eq!(runtime.host_detected_drm_device_count, 2);
        assert_eq!(runtime.host_opened_drm_device_count, 1);
        assert!(!runtime.host_output_ownership);
        assert_eq!(runtime.host_start_attempt_count, 0);
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
        assert_eq!(
            runtime.host_last_start_trigger,
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
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
