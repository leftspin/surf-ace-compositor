use crate::model::{
    HostRuntimeStartTrigger, NativeTargetClass, OutputRotation, PaneId, ProcessSpec,
    ProviderPaneSnapshot, RuntimeBackend, RuntimeFocusTarget, RuntimePhase, StatusSnapshot,
};
use crate::state::CompositorState;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    GetStatus,
    GetHostMode,
    SetOutputRotation {
        rotation: OutputRotation,
    },
    ApplyProviderSnapshot {
        panes: Vec<ProviderPaneSnapshot>,
    },
    SwitchPaneToExternalNative {
        pane_id: PaneId,
        target: NativeTargetClass,
        process: ProcessSpec,
    },
    MarkExternalSurfaceAttached {
        pane_id: PaneId,
    },
    SwitchPaneToSurfAce {
        pane_id: PaneId,
    },
    SetRuntimeMainAppMatchHint {
        hint: String,
    },
    SetRuntimeFocusTarget {
        target: RuntimeFocusTarget,
    },
    ClearRuntimeFocusTarget,
    StartHostRuntime,
    PollProcesses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeControlCommand {
    StartHostRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusSnapshot>,
}

impl ControlResponse {
    fn ok(status: Option<StatusSnapshot>) -> Self {
        Self {
            ok: true,
            error: None,
            status,
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            status: None,
        }
    }
}

pub fn serve(socket_path: &Path, shared_state: Arc<Mutex<CompositorState>>) -> io::Result<()> {
    serve_with_runtime_control(socket_path, shared_state, None)
}

pub fn serve_with_runtime_control(
    socket_path: &Path,
    shared_state: Arc<Mutex<CompositorState>>,
    runtime_control: Option<Sender<RuntimeControlCommand>>,
) -> io::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("failed to accept control connection: {err}");
                continue;
            }
        };
        if let Err(err) = handle_connection(stream, &shared_state, runtime_control.as_ref()) {
            eprintln!("failed to handle control request: {err}");
        }
    }

    Ok(())
}

pub fn send_request(socket_path: &Path, request: &ControlRequest) -> io::Result<ControlResponse> {
    let mut stream = UnixStream::connect(socket_path)?;
    let request_line = serde_json::to_string(request).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid request: {err}"),
        )
    })?;
    stream.write_all(request_line.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut response_line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut response_line)?;
    serde_json::from_str(&response_line).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid response payload: {err}"),
        )
    })
}

fn handle_connection(
    stream: UnixStream,
    shared_state: &Arc<Mutex<CompositorState>>,
    runtime_control: Option<&Sender<RuntimeControlCommand>>,
) -> io::Result<()> {
    let mut request_line = String::new();
    let mut reader = BufReader::new(stream.try_clone()?);
    reader.read_line(&mut request_line)?;
    if request_line.trim().is_empty() {
        return Ok(());
    }

    let response = match serde_json::from_str::<ControlRequest>(&request_line) {
        Ok(request) => {
            let mut state = lock_state(shared_state);
            handle_request(&mut state, request, runtime_control)
        }
        Err(err) => ControlResponse::err(format!("failed to parse request: {err}")),
    };

    let serialized = serde_json::to_string(&response).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to serialize response: {err}"),
        )
    })?;

    let mut stream = stream;
    stream.write_all(serialized.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn lock_state(
    shared_state: &Arc<Mutex<CompositorState>>,
) -> std::sync::MutexGuard<'_, CompositorState> {
    match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn handle_request(
    state: &mut CompositorState,
    request: ControlRequest,
    runtime_control: Option<&Sender<RuntimeControlCommand>>,
) -> ControlResponse {
    let result = match request {
        ControlRequest::GetStatus => Ok(Some(state.status_snapshot())),
        ControlRequest::GetHostMode => Ok(Some(state.status_snapshot())),
        ControlRequest::SetOutputRotation { rotation } => {
            state.set_output_rotation(rotation);
            Ok(Some(state.status_snapshot()))
        }
        ControlRequest::ApplyProviderSnapshot { panes } => state
            .apply_provider_snapshot(panes)
            .map(|_| Some(state.status_snapshot()))
            .map_err(|err| err.to_string()),
        ControlRequest::SwitchPaneToExternalNative {
            pane_id,
            target,
            process,
        } => state
            .switch_pane_to_external_native(&pane_id, target, process)
            .map(|_| Some(state.status_snapshot()))
            .map_err(|err| err.to_string()),
        ControlRequest::MarkExternalSurfaceAttached { pane_id } => state
            .mark_external_surface_attached(&pane_id)
            .map(|_| Some(state.status_snapshot()))
            .map_err(|err| err.to_string()),
        ControlRequest::SwitchPaneToSurfAce { pane_id } => state
            .switch_pane_to_surf_ace(&pane_id)
            .map(|_| Some(state.status_snapshot()))
            .map_err(|err| err.to_string()),
        ControlRequest::SetRuntimeMainAppMatchHint { hint } => {
            if hint.trim().is_empty() {
                return ControlResponse::err("main app match hint must not be empty");
            }
            state.set_runtime_main_app_match_hint(hint);
            Ok(Some(state.status_snapshot()))
        }
        ControlRequest::SetRuntimeFocusTarget { target } => {
            state.set_runtime_focus_target(Some(target));
            Ok(Some(state.status_snapshot()))
        }
        ControlRequest::ClearRuntimeFocusTarget => {
            state.set_runtime_focus_target(None);
            Ok(Some(state.status_snapshot()))
        }
        ControlRequest::StartHostRuntime => {
            if !state.host_mode_active() {
                return ControlResponse::err("host runtime control is unavailable in this mode");
            }

            let runtime = state.status_snapshot().runtime;
            if !matches!(
                runtime.backend,
                RuntimeBackend::HostDrm | RuntimeBackend::None
            ) {
                return ControlResponse::err("host runtime control is unavailable in this mode");
            }
            if runtime.backend == RuntimeBackend::None
                && !matches!(runtime.phase, RuntimePhase::Inactive | RuntimePhase::Failed)
            {
                return ControlResponse::err("host runtime control is unavailable in this mode");
            }
            if matches!(
                runtime.phase,
                RuntimePhase::Starting | RuntimePhase::PreflightReady | RuntimePhase::Running
            ) {
                return ControlResponse::err("host runtime is already active");
            }

            let Some(runtime_control) = runtime_control else {
                return ControlResponse::err("host runtime control channel unavailable");
            };
            match runtime_control.send(RuntimeControlCommand::StartHostRuntime) {
                Ok(()) => {
                    state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::ControlRetry);
                    Ok(Some(state.status_snapshot()))
                }
                Err(err) => Err(format!("failed to queue host runtime start: {err}")),
            }
        }
        ControlRequest::PollProcesses => {
            state.poll_processes();
            Ok(Some(state.status_snapshot()))
        }
    };

    match result {
        Ok(status) => ControlResponse::ok(status),
        Err(message) => ControlResponse::err(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        HostRuntimeStartTrigger, ProcessSpec, RuntimeBackend, RuntimeDmabufFormatStatus,
        RuntimeHostPresentOwnership, RuntimeHostQueuedPresentSource,
    };
    use crate::process_manager::{ProcessController, ProcessExit};
    use crate::state::CompositorState;
    use smithay::backend::allocator::Fourcc as DrmFourcc;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct NoopProcessController;

    impl ProcessController for NoopProcessController {
        fn spawn(
            &mut self,
            _spec: &ProcessSpec,
            _extra_env: &BTreeMap<String, String>,
        ) -> Result<u32, String> {
            Ok(42)
        }

        fn terminate(&mut self, _pid: u32) -> Result<(), String> {
            Ok(())
        }

        fn reap_exited(&mut self) -> Vec<ProcessExit> {
            Vec::new()
        }
    }

    #[test]
    fn start_host_runtime_control_is_rejected_outside_host_mode_without_queue_or_mutation() {
        let mut state = CompositorState::new(false, Box::new(NoopProcessController));
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control is unavailable in this mode")
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(state.status_snapshot().runtime, before);
    }

    #[test]
    fn start_host_runtime_queues_command_and_marks_starting() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_failed("previous failure");
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(response.ok);
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.backend),
            Some(RuntimeBackend::HostDrm)
        );
        assert_eq!(
            response.status.as_ref().map(|status| status.runtime.phase),
            Some(RuntimePhase::Starting)
        );
        assert_eq!(
            rx.recv().ok(),
            Some(RuntimeControlCommand::StartHostRuntime)
        );
    }

    #[test]
    fn start_host_runtime_restarts_from_host_backend_failed_state() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
        state.mark_runtime_failed("host failure");
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(response.ok);
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.backend),
            Some(RuntimeBackend::HostDrm)
        );
        assert_eq!(
            response.status.as_ref().map(|status| status.runtime.phase),
            Some(RuntimePhase::Starting)
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.host_start_attempt_count),
            Some(before.host_start_attempt_count.saturating_add(1))
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .and_then(|status| status.runtime.host_last_start_trigger),
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
        assert_eq!(
            rx.recv().ok(),
            Some(RuntimeControlCommand::StartHostRuntime)
        );
    }

    #[test]
    fn start_host_runtime_queues_command_from_inactive_host_mode() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(response.ok);
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.backend),
            Some(RuntimeBackend::HostDrm)
        );
        assert_eq!(
            response.status.as_ref().map(|status| status.runtime.phase),
            Some(RuntimePhase::Starting)
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.host_start_attempt_count),
            Some(1)
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .and_then(|status| status.runtime.host_last_start_trigger),
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
        assert_eq!(
            rx.recv().ok(),
            Some(RuntimeControlCommand::StartHostRuntime)
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::HostDrm);
        assert_eq!(runtime.phase, RuntimePhase::Starting);
        assert!(!runtime.host_output_ownership);
    }

    #[test]
    fn start_host_runtime_restarts_when_host_runtime_is_stopped() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        state.mark_runtime_stopped();
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(response.ok);
        assert_eq!(
            response.status.as_ref().map(|status| status.runtime.phase),
            Some(RuntimePhase::Starting)
        );
        assert_eq!(
            rx.recv().ok(),
            Some(RuntimeControlCommand::StartHostRuntime)
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::HostDrm);
        assert_eq!(runtime.phase, RuntimePhase::Starting);
        assert!(!runtime.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count.saturating_add(1)
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_already_active() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_starting(RuntimeBackend::HostDrm);
        let (tx, _rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();
        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime is already active")
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_host_runtime_starting_without_mutation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime is already active")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(runtime.host_output_ownership, before.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_preflight_ready() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_host_preflight_ready(Some("wayland-77".to_string()));
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime is already active")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_host_runtime_running_without_mutation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime is already active")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(runtime.host_output_ownership, before.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_while_non_host_runtime_is_active() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control is unavailable in this mode")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::Winit);
        assert_eq!(runtime.phase, RuntimePhase::Running);
        assert!(!runtime.host_output_ownership);
        assert_eq!(runtime.host_start_attempt_count, 0);
    }

    #[test]
    fn start_host_runtime_is_rejected_when_non_host_runtime_failed_without_mutation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        state.mark_runtime_failed("winit failure");
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control is unavailable in this mode")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(runtime.host_output_ownership, before.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_non_host_runtime_stopped_without_mutation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some("wayland-77".to_string()),
            800,
            600,
        );
        state.mark_runtime_stopped();
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control is unavailable in this mode")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(runtime.host_output_ownership, before.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_is_rejected_when_runtime_none_is_stopped_without_mutation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_stopped();
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control is unavailable in this mode")
        );
        assert!(rx.try_recv().is_err());

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(runtime.host_output_ownership, before.host_output_ownership);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_restarts_when_runtime_none_is_failed() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_failed("pre-host bootstrap failure");
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(response.ok);
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.backend),
            Some(RuntimeBackend::HostDrm)
        );
        assert_eq!(
            response.status.as_ref().map(|status| status.runtime.phase),
            Some(RuntimePhase::Starting)
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .map(|status| status.runtime.host_start_attempt_count),
            Some(before.host_start_attempt_count.saturating_add(1))
        );
        assert_eq!(
            response
                .status
                .as_ref()
                .and_then(|status| status.runtime.host_last_start_trigger),
            Some(HostRuntimeStartTrigger::ControlRetry)
        );
        assert_eq!(
            rx.recv().ok(),
            Some(RuntimeControlCommand::StartHostRuntime)
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, RuntimeBackend::HostDrm);
        assert_eq!(runtime.phase, RuntimePhase::Starting);
        assert!(!runtime.host_output_ownership);
    }

    #[test]
    fn start_host_runtime_does_not_mark_starting_when_control_channel_unavailable() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_failed("previous failure");
        let before = state.status_snapshot().runtime;

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, None);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("host runtime control channel unavailable")
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn start_host_runtime_does_not_mark_starting_when_control_send_fails() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_failed("previous failure");
        let before = state.status_snapshot().runtime;
        let (tx, rx) = std::sync::mpsc::channel::<RuntimeControlCommand>();
        drop(rx);

        let response = handle_request(&mut state, ControlRequest::StartHostRuntime, Some(&tx));
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("failed to queue host runtime start: sending on a closed channel")
        );

        let runtime = state.status_snapshot().runtime;
        assert_eq!(runtime.backend, before.backend);
        assert_eq!(runtime.phase, before.phase);
        assert_eq!(
            runtime.host_start_attempt_count,
            before.host_start_attempt_count
        );
        assert_eq!(
            runtime.host_last_start_trigger,
            before.host_last_start_trigger
        );
    }

    #[test]
    fn get_status_preserves_host_present_runtime_truth_fields_for_live_bringup() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some("wayland-77".to_string()),
            1920,
            1080,
        );
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
                code: DrmFourcc::Xrgb8888 as u32,
                modifier: 0,
            }),
            Some(RuntimeDmabufFormatStatus {
                code: DrmFourcc::Argb8888 as u32,
                modifier: 0,
            }),
        );

        let response = handle_request(&mut state, ControlRequest::GetStatus, None);
        assert!(response.ok);
        let runtime = response
            .status
            .expect("status payload should be present")
            .runtime;
        assert_eq!(runtime.phase, RuntimePhase::Running);
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
                code: DrmFourcc::Xrgb8888 as u32,
                modifier: 0
            })
        );
        assert_eq!(
            runtime.host_last_queued_overlay_dmabuf_format,
            Some(RuntimeDmabufFormatStatus {
                code: DrmFourcc::Argb8888 as u32,
                modifier: 0
            })
        );
    }
}
