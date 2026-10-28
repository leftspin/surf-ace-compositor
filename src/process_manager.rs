use crate::child_supervisor::{self, HelperHello, LaunchRequest, SupervisorEvent};
use crate::model::{ProcessLaunchObservation, ProcessSpec};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const UNIT_NAME_MAX_BYTES: usize = 255;
const HELPER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_START_TIMEOUT: Duration = Duration::from_secs(10);
const SYSTEMD_STDERR_CAPTURE_MAX: usize = 16 * 1024;
const DESCRIPTION_PREFIX: &str = "surf-ace-compositor-child-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub observation: Option<ProcessLaunchObservation>,
}

pub type ProcessRequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessControllerEvent {
    SpawnStarted {
        request_id: ProcessRequestId,
        pid: u32,
    },
    SpawnStartedUnconfirmed {
        request_id: ProcessRequestId,
        pid: u32,
        observation: ProcessLaunchObservation,
    },
    SpawnUnconfirmed {
        request_id: ProcessRequestId,
        reason: String,
    },
    SpawnFailed {
        request_id: ProcessRequestId,
        reason: String,
    },
    TerminatedUnconfirmed {
        request_id: ProcessRequestId,
        result: Result<(), String>,
    },
    UnconfirmedExited {
        request_id: ProcessRequestId,
    },
    Terminated {
        pid: u32,
        result: Result<(), String>,
    },
    Exited(ProcessExit),
}

pub trait ProcessController: Send {
    fn spawn(
        &mut self,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<ProcessRequestId, String>;
    fn terminate(&mut self, pid: u32) -> Result<(), String>;
    fn terminate_unconfirmed(&mut self, request_id: ProcessRequestId) -> Result<(), String>;
    fn poll_events(&mut self) -> Vec<ProcessControllerEvent>;
}

enum ProcessWorkerCommand {
    Spawn {
        request_id: ProcessRequestId,
        spec: ProcessSpec,
        extra_env: BTreeMap<String, String>,
    },
    Terminate {
        pid: u32,
    },
    TerminateUnconfirmed {
        request_id: ProcessRequestId,
    },
    Shutdown,
}

pub struct LocalProcessController {
    commands: Sender<ProcessWorkerCommand>,
    events: Receiver<ProcessControllerEvent>,
    shutdown: Arc<AtomicBool>,
    next_request_id: ProcessRequestId,
}

#[derive(Default)]
struct SystemdProcessManager {
    children: HashMap<u32, UnitChild>,
    unconfirmed_children: HashMap<ProcessRequestId, UnitChild>,
}

trait ProcessManagerOperations: Send + 'static {
    fn spawn(
        &mut self,
        request_id: ProcessRequestId,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<ManagedSpawn, String>;
    fn terminate(&mut self, pid: u32) -> Result<(), String>;
    fn terminate_unconfirmed(&mut self, request_id: ProcessRequestId) -> Result<(), String>;
    fn reap_exited(&mut self) -> Vec<ManagedExit>;
}

enum ManagedSpawn {
    Started {
        pid: u32,
        replaced_pid: Option<u32>,
        observation: Option<ProcessLaunchObservation>,
    },
    Unconfirmed,
}

enum ManagedExit {
    Started(ProcessExit),
    Unconfirmed(ProcessRequestId),
}

struct UnitChild {
    app_pid: Option<u32>,
    unit_name: String,
    slot: u32,
    child: Child,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    events: Receiver<Result<SupervisorEvent, ()>>,
    event_reader: Option<JoinHandle<()>>,
    final_event: Option<(Option<i32>, bool)>,
    app_exit_reported: bool,
    event_stream_closed: bool,
    supervisor_status: Option<ExitStatus>,
    secrets: Vec<String>,
    unitless: bool,
    exit_observation_failure_reported: bool,
    launch_observation: Option<ProcessLaunchObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DegradationWarning {
    stage: &'static str,
    reason: String,
}

fn degradation_warning(
    stage: &'static str,
    reason: &str,
    secrets: &[String],
) -> DegradationWarning {
    DegradationWarning {
        stage,
        reason: child_supervisor::sanitize_record(reason, secrets),
    }
}

fn unitless_warning(stage: &'static str, reason: &str, secrets: &[String]) -> DegradationWarning {
    degradation_warning(stage, &format!("{reason} unitless_launch"), secrets)
}

fn child_kept_running_warning(
    stage: &'static str,
    detail: &str,
    secrets: &[String],
) -> DegradationWarning {
    degradation_warning(stage, &format!("child_kept_running {detail}"), secrets)
}

fn child_start_unconfirmed_warning(
    stage: &'static str,
    detail: &str,
    secrets: &[String],
) -> DegradationWarning {
    degradation_warning(stage, &format!("child_start_unconfirmed {detail}"), secrets)
}

fn shortened_unit_warning(original_basename: &str, secrets: &[String]) -> DegradationWarning {
    degradation_warning(
        "unit_name_shortened",
        &format!("original_basename={original_basename}"),
        secrets,
    )
}

fn application_pid_collision_warning(
    previous_unit: &str,
    new_unit: &str,
    secrets: &[String],
) -> DegradationWarning {
    degradation_warning(
        "application_pid_collision",
        &format!("stale_tracking_replaced previous_unit={previous_unit} new_unit={new_unit}"),
        secrets,
    )
}

fn emit_warning(
    slot: u32,
    unit_name: &str,
    warning: &DegradationWarning,
    secrets: &[String],
) -> bool {
    child_supervisor::emit_compositor_diagnostic(
        slot,
        unit_name,
        warning.stage,
        &warning.reason,
        secrets,
    )
}

fn started_child_observation(
    unit_observed: bool,
    event_reader_started: bool,
) -> Option<ProcessLaunchObservation> {
    (!unit_observed || !event_reader_started).then_some(ProcessLaunchObservation::ChildKeptRunning)
}

fn replace_stale_pid_tracking<T>(
    tracked: &mut HashMap<u32, T>,
    pid: u32,
    replacement: T,
    unit_name: impl FnOnce(&T) -> String,
) -> Option<String> {
    let previous = tracked.remove(&pid);
    let previous_unit = previous.as_ref().map(unit_name);
    drop(previous);
    tracked.insert(pid, replacement);
    previous_unit
}

fn unitless_command(
    spec: &ProcessSpec,
    environment: &[child_supervisor::EnvironmentEntry],
) -> Command {
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    command.env_clear();
    for entry in environment {
        command.env(
            std::ffi::OsString::from_vec(entry.key.clone()),
            std::ffi::OsString::from_vec(entry.value.clone()),
        );
    }
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

struct SocketEndpoint {
    directory: std::path::PathBuf,
    socket_path: std::path::PathBuf,
    nonce: String,
    listener: UnixListener,
}

impl Drop for SocketEndpoint {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

enum StartAttempt {
    Started(UnitChild),
    Unconfirmed(UnitChild),
    Collision,
    Failure { stage: &'static str, reason: String },
}

fn unconfirmed_unit_child(
    child: Child,
    stderr_reader: JoinHandle<Vec<u8>>,
    unit_name: &str,
    slot: u32,
    secrets: Vec<String>,
) -> UnitChild {
    let (event_tx, events) = mpsc::channel();
    drop(event_tx);
    UnitChild {
        app_pid: None,
        unit_name: unit_name.to_string(),
        slot,
        child,
        stderr_reader: Some(stderr_reader),
        events,
        event_reader: None,
        final_event: None,
        app_exit_reported: false,
        event_stream_closed: true,
        supervisor_status: None,
        secrets,
        unitless: false,
        exit_observation_failure_reported: false,
        launch_observation: None,
    }
}

impl Default for LocalProcessController {
    fn default() -> Self {
        Self::with_manager(SystemdProcessManager::default())
    }
}

impl LocalProcessController {
    fn with_manager<M: ProcessManagerOperations>(manager: M) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        thread::Builder::new()
            .name("child-process-manager".to_string())
            .spawn(move || process_worker(command_rx, event_tx, manager, worker_shutdown))
            .expect("child process manager worker thread should start");
        Self {
            commands: command_tx,
            events: event_rx,
            shutdown,
            next_request_id: 0,
        }
    }
}

impl Drop for LocalProcessController {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.commands.send(ProcessWorkerCommand::Shutdown);
    }
}

impl ProcessController for LocalProcessController {
    fn spawn(
        &mut self,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<ProcessRequestId, String> {
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request_id = self.next_request_id;
        self.commands
            .send(ProcessWorkerCommand::Spawn {
                request_id,
                spec: spec.clone(),
                extra_env: extra_env.clone(),
            })
            .map_err(|_| "child process manager worker is unavailable".to_string())?;
        Ok(request_id)
    }

    fn terminate(&mut self, pid: u32) -> Result<(), String> {
        self.commands
            .send(ProcessWorkerCommand::Terminate { pid })
            .map_err(|_| "child process manager worker is unavailable".to_string())
    }

    fn terminate_unconfirmed(&mut self, request_id: ProcessRequestId) -> Result<(), String> {
        self.commands
            .send(ProcessWorkerCommand::TerminateUnconfirmed { request_id })
            .map_err(|_| "child process manager worker is unavailable".to_string())
    }

    fn poll_events(&mut self) -> Vec<ProcessControllerEvent> {
        self.events.try_iter().collect()
    }
}

fn process_worker(
    commands: Receiver<ProcessWorkerCommand>,
    events: Sender<ProcessControllerEvent>,
    mut manager: impl ProcessManagerOperations,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        match commands.recv_timeout(Duration::from_millis(25)) {
            Ok(ProcessWorkerCommand::Spawn {
                request_id,
                spec,
                extra_env,
            }) => {
                if shutdown.load(Ordering::Acquire) {
                    continue;
                }
                match manager.spawn(request_id, &spec, &extra_env) {
                    Ok(ManagedSpawn::Started {
                        pid,
                        replaced_pid,
                        observation,
                    }) => {
                        if let Some(replaced_pid) = replaced_pid {
                            if events
                                .send(ProcessControllerEvent::Exited(ProcessExit {
                                    pid: replaced_pid,
                                    exit_code: None,
                                    observation: None,
                                }))
                                .is_err()
                            {
                                return;
                            }
                        }
                        let event = match observation {
                            Some(observation) => ProcessControllerEvent::SpawnStartedUnconfirmed {
                                request_id,
                                pid,
                                observation,
                            },
                            None => ProcessControllerEvent::SpawnStarted { request_id, pid },
                        };
                        if shutdown.load(Ordering::Acquire) || events.send(event).is_err() {
                            return;
                        }
                    }
                    Ok(ManagedSpawn::Unconfirmed) => {
                        if shutdown.load(Ordering::Acquire)
                            || events
                                .send(ProcessControllerEvent::SpawnUnconfirmed {
                                    request_id,
                                    reason: "child_start_unconfirmed".to_string(),
                                })
                                .is_err()
                        {
                            return;
                        }
                    }
                    Err(reason) => {
                        if shutdown.load(Ordering::Acquire)
                            || events
                                .send(ProcessControllerEvent::SpawnFailed { request_id, reason })
                                .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            Ok(ProcessWorkerCommand::Terminate { pid }) => {
                if events
                    .send(ProcessControllerEvent::Terminated {
                        pid,
                        result: manager.terminate(pid),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(ProcessWorkerCommand::TerminateUnconfirmed { request_id }) => {
                if events
                    .send(ProcessControllerEvent::TerminatedUnconfirmed {
                        request_id,
                        result: manager.terminate_unconfirmed(request_id),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(ProcessWorkerCommand::Shutdown) => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        for exit in manager.reap_exited() {
            let event = match exit {
                ManagedExit::Started(exit) => ProcessControllerEvent::Exited(exit),
                ManagedExit::Unconfirmed(request_id) => {
                    ProcessControllerEvent::UnconfirmedExited { request_id }
                }
            };
            if events.send(event).is_err() {
                return;
            }
        }
    }
}

impl SystemdProcessManager {
    fn spawn(
        &mut self,
        request_id: ProcessRequestId,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<ManagedSpawn, String> {
        let child_basename = Path::new(&spec.command)
            .file_name()
            .map(|name| name.as_bytes().to_vec())
            .filter(|name| !name.is_empty());

        let environment = child_supervisor::effective_child_environment(spec, extra_env);
        let secrets = child_supervisor::sensitive_environment_values(&environment);
        let Some(child_basename) = child_basename else {
            return self.start_unitless(
                spec,
                &environment,
                &secrets,
                0,
                "unavailable",
                "unit_name_invalid",
                "requested executable has no non-empty basename",
            );
        };
        let encoded_basename = escape_unit_component(&child_basename);
        let shortened =
            encoded_basename.len() + "surface0-".len() + ".service".len() > UNIT_NAME_MAX_BYTES;
        let max_slot = if shortened {
            u32::MAX
        } else if let Some(max_slot) = max_slot_for_child(&encoded_basename) {
            max_slot
        } else {
            return self.start_unitless(
                spec,
                &environment,
                &secrets,
                0,
                "unavailable",
                "unit_name_invalid",
                "child basename cannot fit a complete systemd unit name",
            );
        };
        let main_app = extra_env
            .get("SURF_ACE_COMPOSITOR_MAIN_APP")
            .is_some_and(|value| value == "1");
        let occupied_slots: HashSet<u32> = self
            .children
            .values()
            .filter_map(|child| unit_slot_for_name(&child.unit_name))
            .collect();
        let mut attempted_slots = HashSet::new();
        let mut collision_unit = None;

        loop {
            let Some(slot) =
                next_candidate_slot(main_app, max_slot, &occupied_slots, &attempted_slots)
            else {
                let unit_name = collision_unit.as_deref().unwrap_or("unavailable");
                let stage = if collision_unit.is_some() {
                    "unit_name_collision"
                } else {
                    "unit_name_exhausted"
                };
                let reason = "no valid unused child unit name remains";
                return self.start_unitless(
                    spec,
                    &environment,
                    &secrets,
                    0,
                    unit_name,
                    stage,
                    reason,
                );
            };
            attempted_slots.insert(slot);
            let child_component = if shortened {
                shorten_unit_component(&child_basename, slot)
            } else {
                encoded_basename.clone()
            };
            let unit_name = format!("surface{slot}-{child_component}.service");
            let request = LaunchRequest {
                nonce: String::new(),
                unit_name: unit_name.clone(),
                slot,
                spec: spec.clone(),
                environment: environment.clone(),
            };

            match start_unit(slot, &unit_name, request) {
                StartAttempt::Started(mut child) => {
                    child.secrets = secrets.clone();
                    let observation = child.launch_observation;
                    if let Some(previous) = collision_unit {
                        let _ = child_supervisor::emit_compositor_diagnostic(
                            slot,
                            &unit_name,
                            "unit_name_collision_reallocated",
                            &format!("previous_unit={previous}"),
                            &secrets,
                        );
                    }
                    if shortened {
                        let original = String::from_utf8_lossy(&child_basename);
                        let warning = shortened_unit_warning(&original, &secrets);
                        emit_warning(slot, &unit_name, &warning, &secrets);
                    }
                    let pid = child.app_pid.expect("started child has an application pid");
                    let child_unit_name = child.unit_name.clone();
                    if let Some(previous_unit) =
                        replace_stale_pid_tracking(&mut self.children, pid, child, |old| {
                            old.unit_name.clone()
                        })
                    {
                        let warning = application_pid_collision_warning(
                            &previous_unit,
                            &child_unit_name,
                            &secrets,
                        );
                        emit_warning(slot, &child_unit_name, &warning, &secrets);
                        return Ok(ManagedSpawn::Started {
                            pid,
                            replaced_pid: Some(pid),
                            observation,
                        });
                    }
                    let _ = child_supervisor::emit_compositor_diagnostic(
                        slot,
                        &unit_name,
                        "child_launch_started",
                        &format!("application_pid={pid}"),
                        &secrets,
                    );
                    return Ok(ManagedSpawn::Started {
                        pid,
                        replaced_pid: None,
                        observation,
                    });
                }
                StartAttempt::Unconfirmed(mut child) => {
                    child.secrets = secrets.clone();
                    let unit_name = child.unit_name.clone();
                    self.unconfirmed_children.insert(request_id, child);
                    let warning = child_start_unconfirmed_warning(
                        "child_start_unconfirmed",
                        "start_report_missing",
                        &secrets,
                    );
                    emit_warning(slot, &unit_name, &warning, &secrets);
                    return Ok(ManagedSpawn::Unconfirmed);
                }
                StartAttempt::Collision => {
                    collision_unit = Some(unit_name);
                }
                StartAttempt::Failure { stage, reason } => {
                    let safe_reason = child_supervisor::sanitize_record(&reason, &secrets);
                    let _ = child_supervisor::emit_compositor_diagnostic(
                        slot,
                        &unit_name,
                        stage,
                        &safe_reason,
                        &secrets,
                    );
                    if stage == "child_exec_failure" {
                        return Err(format!("{stage}: {safe_reason}"));
                    }
                    return self.start_unitless(
                        spec,
                        &environment,
                        &secrets,
                        slot,
                        &unit_name,
                        stage,
                        &safe_reason,
                    );
                }
            }
        }
    }

    fn start_unitless(
        &mut self,
        spec: &ProcessSpec,
        environment: &[child_supervisor::EnvironmentEntry],
        secrets: &[String],
        slot: u32,
        unit_name: &str,
        stage: &'static str,
        reason: &str,
    ) -> Result<ManagedSpawn, String> {
        let mut command = unitless_command(spec, environment);
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let safe_reason = child_supervisor::sanitize_record(
                    &format!("requested executable failed to start: {error}"),
                    secrets,
                );
                let _ = child_supervisor::emit_compositor_diagnostic(
                    slot,
                    unit_name,
                    "child_exec_failure",
                    &safe_reason,
                    secrets,
                );
                return Err(format!("child_exec_failure: {safe_reason}"));
            }
        };
        let pid = child.id();
        let (event_tx, events) = mpsc::channel();
        drop(event_tx);
        let previous_unit = replace_stale_pid_tracking(
            &mut self.children,
            pid,
            UnitChild {
                app_pid: Some(pid),
                unit_name: unit_name.to_string(),
                slot,
                child,
                stderr_reader: None,
                events,
                event_reader: None,
                final_event: None,
                app_exit_reported: false,
                event_stream_closed: true,
                supervisor_status: None,
                secrets: secrets.to_vec(),
                unitless: true,
                exit_observation_failure_reported: false,
                launch_observation: None,
            },
            |old| old.unit_name.clone(),
        );
        let warning = unitless_warning(stage, reason, secrets);
        emit_warning(slot, unit_name, &warning, secrets);
        if let Some(previous_unit) = previous_unit.as_ref() {
            let warning = application_pid_collision_warning(previous_unit, unit_name, secrets);
            emit_warning(slot, unit_name, &warning, secrets);
        }
        Ok(ManagedSpawn::Started {
            pid,
            replaced_pid: previous_unit.as_ref().map(|_| pid),
            observation: None,
        })
    }

    fn terminate(&mut self, pid: u32) -> Result<(), String> {
        let Some(child) = self.children.get_mut(&pid) else {
            return Err(format!("unknown pid: {pid}"));
        };
        let unit_name = child.unit_name.clone();
        let slot = child.slot;
        let secrets = child.secrets.clone();
        let _ = child_supervisor::emit_compositor_diagnostic(
            slot,
            &unit_name,
            "child_stop_requested",
            &format!("application_pid={pid}"),
            &secrets,
        );
        if child.unitless {
            if let Some(group) = rustix::process::Pid::from_raw(pid as i32) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            }
            let _ = child.child.kill();
            let _ = child.child.wait();
            self.children.remove(&pid);
            let _ = child_supervisor::emit_compositor_diagnostic(
                slot,
                &unit_name,
                "child_stop_complete",
                "unitless_process_group_stopped",
                &secrets,
            );
            return Ok(());
        }
        if !systemctl_stop(&unit_name) {
            let reason = "systemctl_user_stop_failed";
            let _ = child_supervisor::emit_compositor_diagnostic(
                slot,
                &unit_name,
                "child_stop_failure",
                reason,
                &secrets,
            );
            self.children.remove(&pid);
            return Ok(());
        }

        if let Some(child) = self.children.get_mut(&pid) {
            let _ = child.child.wait();
        }
        let Some(mut child) = self.children.remove(&pid) else {
            return Ok(());
        };
        if let Some(reader) = child.stderr_reader.take() {
            let _ = reader.join();
        }
        child.event_stream_closed = true;
        if let Some(reader) = child.event_reader.take() {
            let _ = reader.join();
        }
        let collected = wait_for_collection(&unit_name);
        let (stage, reason) = if collected {
            ("child_stop_complete", "unit_stopped_and_collected")
        } else {
            (
                "unit_collection_unobserved",
                "unit_remained_or_state_unavailable",
            )
        };
        let _ =
            child_supervisor::emit_compositor_diagnostic(slot, &unit_name, stage, reason, &secrets);
        Ok(())
    }

    fn reap_exited(&mut self) -> Vec<ManagedExit> {
        let pids: Vec<_> = self.children.keys().copied().collect();
        let mut to_remove = Vec::new();
        let mut exited = Vec::new();

        for pid in pids {
            let Some(child) = self.children.get_mut(&pid) else {
                continue;
            };
            if child.unitless {
                match child.child.try_wait() {
                    Ok(Some(status)) => {
                        let exit_code = status.code();
                        let _ = child_supervisor::emit_compositor_diagnostic(
                            child.slot,
                            &child.unit_name,
                            "child_exit_observed",
                            &format!("application_pid={pid} exit_code={exit_code:?}"),
                            &child.secrets,
                        );
                        exited.push(ManagedExit::Started(ProcessExit {
                            pid,
                            exit_code,
                            observation: None,
                        }));
                        to_remove.push(pid);
                    }
                    Ok(None) => {}
                    Err(_) if !child.exit_observation_failure_reported => {
                        child.exit_observation_failure_reported = true;
                        let _ = child_supervisor::emit_compositor_diagnostic(
                            child.slot,
                            &child.unit_name,
                            "child_exit_unobserved",
                            "child_kept_running application_exit_status_unknown",
                            &child.secrets,
                        );
                    }
                    Err(_) => {}
                }
                continue;
            }
            loop {
                match child.events.try_recv() {
                    Ok(Ok(SupervisorEvent::SanitizerFailed { reason })) => {
                        let _ = child_supervisor::emit_compositor_diagnostic(
                            child.slot,
                            &child.unit_name,
                            "sanitizer_failure",
                            &reason,
                            &child.secrets,
                        );
                    }
                    Ok(Ok(SupervisorEvent::Finished {
                        exit_code,
                        sanitizer_failed,
                    })) => {
                        child.final_event = Some((exit_code, sanitizer_failed));
                    }
                    Ok(Ok(SupervisorEvent::ChildExited { exit_code, .. })) => {
                        if !child.app_exit_reported {
                            exited.push(ManagedExit::Started(ProcessExit {
                                pid,
                                exit_code,
                                observation: None,
                            }));
                            child.app_exit_reported = true;
                        }
                    }
                    Ok(Ok(SupervisorEvent::StartFailed { reason })) => {
                        child.final_event = Some((None, true));
                        let _ = child_supervisor::emit_compositor_diagnostic(
                            child.slot,
                            &child.unit_name,
                            "child_exec_failure",
                            &reason,
                            &child.secrets,
                        );
                    }
                    Ok(Ok(SupervisorEvent::Started { .. })) => {}
                    Ok(Err(())) => {
                        child.event_stream_closed = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        child.event_stream_closed = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }

            if child.supervisor_status.is_none() {
                match child.child.try_wait() {
                    Ok(Some(status)) => child.supervisor_status = Some(status),
                    Ok(None) => {}
                    Err(_) => {
                        child.event_stream_closed = true;
                        child.supervisor_status = child.child.try_wait().ok().flatten();
                    }
                }
            }

            if child.supervisor_status.is_some()
                && child.final_event.is_none()
                && child.event_stream_closed
            {
                let status = child
                    .supervisor_status
                    .and_then(|status| {
                        status
                            .code()
                            .map(|code| format!("supervisor_exit_code={code}"))
                    })
                    .unwrap_or_else(|| "supervisor_exit_status=unknown".to_string());
                let _ = child_supervisor::emit_compositor_diagnostic(
                    child.slot,
                    &child.unit_name,
                    "child_unit_observation_failure",
                    &format!("supervisor_finished_without_child_outcome {status}"),
                    &child.secrets,
                );
            }

            if child.supervisor_status.is_some()
                && (child.final_event.is_some() || child.event_stream_closed)
            {
                if let Some((exit_code, sanitizer_failed)) = child.final_event {
                    let _ = child_supervisor::emit_compositor_diagnostic(
                        child.slot,
                        &child.unit_name,
                        "child_exit_observed",
                        &format!(
                            "application_pid={pid} exit_code={} sanitizer_failed={sanitizer_failed}",
                            exit_code
                                .map(|code| code.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        ),
                        &child.secrets,
                    );
                    if !child.app_exit_reported {
                        exited.push(ManagedExit::Started(ProcessExit {
                            pid,
                            exit_code,
                            observation: None,
                        }));
                        child.app_exit_reported = true;
                    }
                } else {
                    let _ = child_supervisor::emit_compositor_diagnostic(
                        child.slot,
                        &child.unit_name,
                        "child_exit_unobserved",
                        "application_exit_status_unknown",
                        &child.secrets,
                    );
                    if !child.app_exit_reported {
                        exited.push(ManagedExit::Started(ProcessExit {
                            pid,
                            exit_code: None,
                            observation: child.launch_observation,
                        }));
                        child.app_exit_reported = true;
                    }
                }
                if let Some(reader) = child.stderr_reader.take() {
                    let _ = reader.join();
                }
                if let Some(reader) = child.event_reader.take() {
                    let _ = reader.join();
                }
                let collected = wait_for_collection(&child.unit_name);
                let (stage, reason) = if collected {
                    ("child_unit_collected", "transient_definition_absent")
                } else {
                    (
                        "unit_collection_unobserved",
                        "unit_remained_or_state_unavailable",
                    )
                };
                let _ = child_supervisor::emit_compositor_diagnostic(
                    child.slot,
                    &child.unit_name,
                    stage,
                    reason,
                    &child.secrets,
                );
                to_remove.push(pid);
            }
        }

        for pid in to_remove {
            self.children.remove(&pid);
        }

        let request_ids: Vec<_> = self.unconfirmed_children.keys().copied().collect();
        let mut unconfirmed_to_remove = Vec::new();
        for request_id in request_ids {
            let Some(child) = self.unconfirmed_children.get_mut(&request_id) else {
                continue;
            };
            match child.child.try_wait() {
                Ok(Some(_status)) => {
                    let _ = child_supervisor::emit_compositor_diagnostic(
                        child.slot,
                        &child.unit_name,
                        "child_exit_unobserved",
                        "child_start_unconfirmed application_exit_status_unknown",
                        &child.secrets,
                    );
                    if let Some(reader) = child.stderr_reader.take() {
                        let _ = reader.join();
                    }
                    unconfirmed_to_remove.push(request_id);
                    exited.push(ManagedExit::Unconfirmed(request_id));
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
        for request_id in unconfirmed_to_remove {
            self.unconfirmed_children.remove(&request_id);
        }
        exited
    }

    fn terminate_unconfirmed(&mut self, request_id: ProcessRequestId) -> Result<(), String> {
        let Some(child) = self.unconfirmed_children.get_mut(&request_id) else {
            return Err("unknown unconfirmed child launch".to_string());
        };
        let unit_name = child.unit_name.clone();
        let slot = child.slot;
        let secrets = child.secrets.clone();
        if !systemctl_stop(&unit_name) {
            let _ = child_supervisor::emit_compositor_diagnostic(
                slot,
                &unit_name,
                "child_stop_failure",
                "systemctl_user_stop_failed child_start_unconfirmed",
                &secrets,
            );
            self.unconfirmed_children.remove(&request_id);
            return Ok(());
        }
        if let Some(child) = self.unconfirmed_children.get_mut(&request_id) {
            let _ = child.child.wait();
        }
        if let Some(mut child) = self.unconfirmed_children.remove(&request_id) {
            if let Some(reader) = child.stderr_reader.take() {
                let _ = reader.join();
            }
            let collected = wait_for_collection(&unit_name);
            let (stage, reason) = if collected {
                ("child_stop_complete", "unit_stopped_and_collected")
            } else {
                (
                    "unit_collection_unobserved",
                    "unit_remained_or_state_unavailable",
                )
            };
            let _ = child_supervisor::emit_compositor_diagnostic(
                slot, &unit_name, stage, reason, &secrets,
            );
        }
        Ok(())
    }
}

impl ProcessManagerOperations for SystemdProcessManager {
    fn spawn(
        &mut self,
        request_id: ProcessRequestId,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<ManagedSpawn, String> {
        SystemdProcessManager::spawn(self, request_id, spec, extra_env)
    }

    fn terminate(&mut self, pid: u32) -> Result<(), String> {
        SystemdProcessManager::terminate(self, pid)
    }

    fn terminate_unconfirmed(&mut self, request_id: ProcessRequestId) -> Result<(), String> {
        SystemdProcessManager::terminate_unconfirmed(self, request_id)
    }

    fn reap_exited(&mut self) -> Vec<ManagedExit> {
        SystemdProcessManager::reap_exited(self)
    }
}

fn start_unit(slot: u32, unit_name: &str, mut request: LaunchRequest) -> StartAttempt {
    let endpoint = match SocketEndpoint::new() {
        Ok(endpoint) => endpoint,
        Err(_) => {
            return StartAttempt::Failure {
                stage: "supervisor_socket_failure",
                reason: "private_supervisor_socket_unavailable".to_string(),
            };
        }
    };
    request.nonce = endpoint.nonce.clone();
    let socket_path = match endpoint.socket_path.to_str() {
        Some(path) => path.to_string(),
        None => {
            return StartAttempt::Failure {
                stage: "supervisor_socket_failure",
                reason: "supervisor_socket_path_not_utf8".to_string(),
            };
        }
    };
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => {
            return StartAttempt::Failure {
                stage: "supervisor_start_failure",
                reason: "compositor_executable_path_unavailable".to_string(),
            };
        }
    };
    let description = format!("{DESCRIPTION_PREFIX}{}", endpoint.nonce);
    let mut command = Command::new("systemd-run");
    command
        .arg("--user")
        .arg(format!("--unit={unit_name}"))
        .arg("--wait")
        .arg("--collect")
        .arg("--quiet")
        .arg("--service-type=exec")
        .arg("--property=KillMode=control-group")
        .arg("--property=TimeoutStopSec=5s")
        .arg("--property=StandardOutput=journal")
        .arg("--property=StandardError=journal")
        .arg(format!("--property=Description={description}"))
        .arg(executable)
        .arg("child-supervisor")
        .arg("--socket-path")
        .arg(&socket_path)
        .arg("--nonce")
        .arg(&endpoint.nonce)
        .arg("--unit-name")
        .arg(unit_name)
        .arg("--slot")
        .arg(slot.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut service_client = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            return StartAttempt::Failure {
                stage: "unit_creation_failure",
                reason: "systemd_run_unavailable_or_failed_to_start".to_string(),
            };
        }
    };
    let stderr = match service_client.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = service_client.kill();
            let _ = service_client.wait();
            return StartAttempt::Failure {
                stage: "unit_creation_failure",
                reason: "systemd_run_stderr_capture_unavailable".to_string(),
            };
        }
    };
    let stderr_reader = thread::spawn(move || read_bounded(stderr, SYSTEMD_STDERR_CAPTURE_MAX));
    let accept_deadline = Instant::now() + HELPER_CONNECT_TIMEOUT;
    let mut stream = loop {
        match endpoint.listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Ok(Some(status)) = service_client.try_wait() {
                    let output = stderr_reader.join().unwrap_or_default();
                    if !status.success() && is_unit_name_collision(&output) {
                        return StartAttempt::Collision;
                    }
                    return StartAttempt::Failure {
                        stage: "unit_creation_failure",
                        reason: "systemd_run_rejected_transient_child_unit".to_string(),
                    };
                }
                if Instant::now() >= accept_deadline {
                    let _ = service_client.kill();
                    let _ = service_client.wait();
                    let _ = stderr_reader.join();
                    if unit_description_matches(unit_name, &description) {
                        let _ = systemctl_stop(unit_name);
                    }
                    return StartAttempt::Failure {
                        stage: "unit_observation_failure",
                        reason: "supervisor_control_connection_timeout".to_string(),
                    };
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = service_client.kill();
                let _ = service_client.wait();
                let _ = stderr_reader.join();
                return StartAttempt::Failure {
                    stage: "unit_observation_failure",
                    reason: "supervisor_control_accept_failure".to_string(),
                };
            }
        }
    };
    let _ = stream.set_read_timeout(Some(HELPER_START_TIMEOUT));
    let hello: HelperHello = match child_supervisor::read_frame::<_, HelperHello>(&mut stream) {
        Ok(hello) if hello.nonce == endpoint.nonce => hello,
        _ => {
            let _ = service_client.kill();
            let _ = service_client.wait();
            let _ = stderr_reader.join();
            return StartAttempt::Failure {
                stage: "supervisor_protocol_failure",
                reason: "supervisor_nonce_handshake_failed".to_string(),
            };
        }
    };
    if hello.nonce != request.nonce {
        let _ = service_client.kill();
        let _ = service_client.wait();
        let _ = stderr_reader.join();
        return StartAttempt::Failure {
            stage: "supervisor_protocol_failure",
            reason: "supervisor_nonce_mismatch_before_launch_request".to_string(),
        };
    }
    if child_supervisor::write_frame(&mut stream, &request).is_err() {
        let secrets = child_supervisor::sensitive_environment_values(&request.environment);
        let warning = child_start_unconfirmed_warning(
            "supervisor_protocol_failure",
            "launch_request_write_failed",
            &secrets,
        );
        emit_warning(slot, unit_name, &warning, &secrets);
        return StartAttempt::Unconfirmed(unconfirmed_unit_child(
            service_client,
            stderr_reader,
            unit_name,
            slot,
            child_supervisor::sensitive_environment_values(&request.environment),
        ));
    }
    let started: SupervisorEvent = match child_supervisor::read_frame(&mut stream) {
        Ok(event) => event,
        Err(_) => {
            let secrets = child_supervisor::sensitive_environment_values(&request.environment);
            let warning = child_start_unconfirmed_warning(
                "child_start_unconfirmed",
                "supervisor_started_event_missing",
                &secrets,
            );
            emit_warning(slot, unit_name, &warning, &secrets);
            return StartAttempt::Unconfirmed(unconfirmed_unit_child(
                service_client,
                stderr_reader,
                unit_name,
                slot,
                child_supervisor::sensitive_environment_values(&request.environment),
            ));
        }
    };
    let (pid, supervisor_pid) = match started {
        SupervisorEvent::Started {
            pid,
            supervisor_pid,
        } if pid > 0 && supervisor_pid > 0 => (pid, supervisor_pid),
        SupervisorEvent::StartFailed { reason } => {
            let _ = service_client.wait();
            let _ = stderr_reader.join();
            return StartAttempt::Failure {
                stage: "child_exec_failure",
                reason,
            };
        }
        _ => {
            let secrets = child_supervisor::sensitive_environment_values(&request.environment);
            let warning = child_start_unconfirmed_warning(
                "child_start_unconfirmed",
                "supervisor_started_event_invalid",
                &secrets,
            );
            emit_warning(slot, unit_name, &warning, &secrets);
            return StartAttempt::Unconfirmed(unconfirmed_unit_child(
                service_client,
                stderr_reader,
                unit_name,
                slot,
                child_supervisor::sensitive_environment_values(&request.environment),
            ));
        }
    };
    let _ = stream.set_read_timeout(None);

    let unit_observed = unit_is_observed(unit_name, &description, supervisor_pid);
    if !unit_observed {
        let warning = child_kept_running_warning(
            "unit_observation_failure",
            "transient_unit_or_sanitizer_path_not_observed",
            &child_supervisor::sensitive_environment_values(&request.environment),
        );
        emit_warning(
            slot,
            unit_name,
            &warning,
            &child_supervisor::sensitive_environment_values(&request.environment),
        );
    }

    let (event_tx, events) = mpsc::channel();
    let event_reader = match thread::Builder::new()
        .name(format!("child-unit-events-{slot}"))
        .spawn(move || read_events(stream, event_tx))
    {
        Ok(reader) => Some(reader),
        Err(_) => {
            let warning = child_kept_running_warning(
                "supervisor_protocol_failure",
                "supervisor_event_reader_start_failure",
                &child_supervisor::sensitive_environment_values(&request.environment),
            );
            emit_warning(
                slot,
                unit_name,
                &warning,
                &child_supervisor::sensitive_environment_values(&request.environment),
            );
            None
        }
    };
    let launch_observation = started_child_observation(unit_observed, event_reader.is_some());

    let _ = child_supervisor::emit_compositor_diagnostic(
        slot,
        unit_name,
        "child_unit_observed",
        &format!("application_pid={pid} supervisor_pid={supervisor_pid}"),
        &child_supervisor::sensitive_environment_values(&request.environment),
    );
    StartAttempt::Started(UnitChild {
        app_pid: Some(pid),
        unit_name: unit_name.to_string(),
        slot,
        child: service_client,
        stderr_reader: Some(stderr_reader),
        events,
        event_reader,
        final_event: None,
        app_exit_reported: false,
        event_stream_closed: false,
        supervisor_status: None,
        secrets: Vec::new(),
        unitless: false,
        exit_observation_failure_reported: false,
        launch_observation,
    })
}

fn read_events(mut stream: UnixStream, sender: Sender<Result<SupervisorEvent, ()>>) {
    loop {
        match child_supervisor::read_frame(&mut stream) {
            Ok(event) => {
                let terminal = matches!(
                    event,
                    SupervisorEvent::Finished { .. } | SupervisorEvent::StartFailed { .. }
                );
                if sender.send(Ok(event)).is_err() || terminal {
                    return;
                }
            }
            Err(_) => {
                let _ = sender.send(Err(()));
                return;
            }
        }
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                let remaining = limit.saturating_sub(captured.len());
                captured.extend_from_slice(&chunk[..count.min(remaining)]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    captured
}

fn is_unit_name_collision(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("already exists")
        || message.contains("already loaded")
        || message.contains("unit exists")
}

fn unit_is_observed(unit_name: &str, description: &str, supervisor_pid: u32) -> bool {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "--no-pager",
            "--property=LoadState",
            "--property=ActiveState",
            "--property=MainPID",
            "--property=Description",
            unit_name,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let properties: HashMap<String, String> = output_text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    properties.get("LoadState").map(String::as_str) == Some("loaded")
        && properties.get("Description").map(String::as_str) == Some(description)
        && properties
            .get("MainPID")
            .and_then(|pid| pid.parse::<u32>().ok())
            == Some(supervisor_pid)
        && properties
            .get("ActiveState")
            .is_some_and(|state| matches!(state.as_str(), "active" | "activating" | "deactivating"))
}

fn unit_description_matches(unit_name: &str, description: &str) -> bool {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "--no-pager",
            "--property=Description",
            unit_name,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == format!("Description={description}"))
}

fn systemctl_stop(unit_name: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "stop", unit_name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_collection(unit_name: &str) -> bool {
    for _ in 0..20 {
        let result = Command::new("systemctl")
            .args([
                "--user",
                "show",
                "--no-pager",
                "--property=LoadState",
                unit_name,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match result {
            Ok(output) if output.status.success() => {
                let not_found = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line == "LoadState=not-found");
                if not_found {
                    return true;
                }
            }
            Ok(_) => return false,
            Err(_) => return false,
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

impl SocketEndpoint {
    fn new() -> io::Result<Self> {
        static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(0);
        let counter = NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = std::env::temp_dir();
        let directory = base.join(format!(
            "sac-{}-{}-{counter:x}",
            std::process::id(),
            timestamp
        ));
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let socket_path = directory.join("control.sock");
        if socket_path.as_os_str().as_bytes().len() >= 100 {
            let _ = fs::remove_dir_all(&directory);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "supervisor socket path exceeds Unix socket path limit",
            ));
        }
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            directory,
            socket_path,
            nonce: format!("{:x}-{timestamp:x}-{counter:x}", std::process::id()),
            listener,
        })
    }
}

pub fn escape_unit_component(bytes: &[u8]) -> String {
    let mut escaped = String::new();
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b':' | b'_' | b'.') {
            escaped.push(*byte as char);
        } else {
            escaped.push_str(&format!("\\x{byte:02x}"));
        }
    }
    escaped
}

fn shorten_unit_component(basename: &[u8], slot: u32) -> String {
    let mut digest = 0xcbf29ce484222325_u64;
    for byte in basename {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x100000001b3);
    }
    let suffix = format!("-{digest:016x}");
    let fixed = format!("surface{slot}-").len() + suffix.len() + ".service".len();
    let prefix_capacity = UNIT_NAME_MAX_BYTES.saturating_sub(fixed);

    let mut prefix = String::new();
    for byte in basename {
        let token = if byte.is_ascii_alphanumeric() || matches!(*byte, b':' | b'_' | b'.') {
            (*byte as char).to_string()
        } else {
            format!("\\x{byte:02x}")
        };
        if prefix.len() + token.len() > prefix_capacity {
            break;
        }
        prefix.push_str(&token);
    }
    format!("{prefix}{suffix}")
}

fn max_slot_for_child(encoded_basename: &str) -> Option<u32> {
    let fixed = "surface".len() + 1 + encoded_basename.len() + ".service".len();
    let digits = UNIT_NAME_MAX_BYTES.checked_sub(fixed)?;
    if digits == 0 {
        return None;
    }
    if digits >= 10 {
        Some(u32::MAX)
    } else {
        Some(10_u32.pow(digits as u32) - 1)
    }
}

fn unit_slot_for_name(unit_name: &str) -> Option<u32> {
    let child_name = unit_name
        .strip_suffix(".service")?
        .strip_prefix("surface")?
        .split_once('-')?
        .0;
    child_name.parse().ok()
}

fn next_candidate_slot(
    prefer_main: bool,
    max_slot: u32,
    occupied: &HashSet<u32>,
    attempted: &HashSet<u32>,
) -> Option<u32> {
    if prefer_main && !occupied.contains(&0) && !attempted.contains(&0) {
        return Some(0);
    }
    (1..=max_slot).find(|slot| !occupied.contains(slot) && !attempted.contains(slot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Instant;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "surf-ace-warning-emission-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("diagnostic capture directory");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct RestorePath(Option<OsString>);

    impl Drop for RestorePath {
        fn drop(&mut self) {
            unsafe {
                match self.0.as_ref() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn assert_warning_submitted(
        slot: u32,
        unit_name: &str,
        warning: &DegradationWarning,
        secrets: &[String],
    ) {
        static PATH_LOCK: Mutex<()> = Mutex::new(());
        let _path_lock = PATH_LOCK.lock().expect("diagnostic test PATH lock");
        let root = TestDirectory::new();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).expect("test command directory");
        let capture = root.path().join("record.txt");
        let capture_path = capture.to_string_lossy();
        assert!(!capture_path.contains('\''));
        let systemd_cat = bin.join("systemd-cat");
        std::fs::write(
            &systemd_cat,
            format!(
                "#!/bin/sh\n[ \"$1\" = \"--identifier=surf-ace-compositor\" ] || exit 31\n[ \"$2\" = \"--level-prefix=false\" ] || exit 32\nexec /bin/cat > '{capture_path}'\n"
            ),
        )
        .expect("test systemd-cat command");
        std::fs::set_permissions(&systemd_cat, std::fs::Permissions::from_mode(0o755))
            .expect("test command is executable");

        let old_path = std::env::var_os("PATH");
        let _restore_path = RestorePath(old_path.clone());
        let mut path_entries = vec![bin];
        if let Some(old_path) = old_path.as_ref() {
            path_entries.extend(std::env::split_paths(old_path));
        }
        let test_path = std::env::join_paths(path_entries).expect("test PATH");
        unsafe { std::env::set_var("PATH", test_path) };
        let submitted = emit_warning(slot, unit_name, warning, secrets);
        drop(_restore_path);

        assert!(submitted, "diagnostic emitter accepted the warning");
        let record = std::fs::read_to_string(&capture).expect("emitted warning record");
        let timestamp = record
            .split_whitespace()
            .next()
            .and_then(|field| field.strip_prefix("timestamp_ms="))
            .and_then(|value| value.parse::<u128>().ok());
        assert!(
            timestamp.is_some_and(|value| value > 0),
            "warning has a timestamp"
        );
        assert!(
            record.contains(&format!(
                "unit={unit_name} slot={slot} stage={} reason={}",
                warning.stage, warning.reason
            )),
            "warning record missing expected fields: {record}"
        );
        for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            assert!(
                !record.contains(secret),
                "warning leaked a synthetic secret"
            );
        }
    }

    struct GatedProcessManager {
        started: Sender<()>,
        release: Receiver<()>,
    }

    struct GatedStopManager {
        started: Sender<()>,
        release: Receiver<()>,
    }

    struct KeptRunningStartManager;

    impl ProcessManagerOperations for KeptRunningStartManager {
        fn spawn(
            &mut self,
            _request_id: ProcessRequestId,
            _spec: &ProcessSpec,
            _extra_env: &BTreeMap<String, String>,
        ) -> Result<ManagedSpawn, String> {
            Ok(ManagedSpawn::Started {
                pid: 77,
                replaced_pid: None,
                observation: Some(ProcessLaunchObservation::ChildKeptRunning),
            })
        }

        fn terminate(&mut self, _pid: u32) -> Result<(), String> {
            Ok(())
        }

        fn terminate_unconfirmed(&mut self, _request_id: ProcessRequestId) -> Result<(), String> {
            Ok(())
        }

        fn reap_exited(&mut self) -> Vec<ManagedExit> {
            Vec::new()
        }
    }

    impl ProcessManagerOperations for GatedProcessManager {
        fn spawn(
            &mut self,
            _request_id: ProcessRequestId,
            _spec: &ProcessSpec,
            _extra_env: &BTreeMap<String, String>,
        ) -> Result<ManagedSpawn, String> {
            let _ = self.started.send(());
            self.release
                .recv()
                .map_err(|_| "test worker release was dropped".to_string())?;
            Ok(ManagedSpawn::Started {
                pid: 77,
                replaced_pid: None,
                observation: None,
            })
        }

        fn terminate(&mut self, _pid: u32) -> Result<(), String> {
            Ok(())
        }

        fn terminate_unconfirmed(&mut self, _request_id: ProcessRequestId) -> Result<(), String> {
            Ok(())
        }

        fn reap_exited(&mut self) -> Vec<ManagedExit> {
            Vec::new()
        }
    }

    impl ProcessManagerOperations for GatedStopManager {
        fn spawn(
            &mut self,
            _request_id: ProcessRequestId,
            _spec: &ProcessSpec,
            _extra_env: &BTreeMap<String, String>,
        ) -> Result<ManagedSpawn, String> {
            Err("test stop manager does not spawn".to_string())
        }

        fn terminate(&mut self, _pid: u32) -> Result<(), String> {
            let _ = self.started.send(());
            self.release
                .recv()
                .map_err(|_| "test stop release was dropped".to_string())
        }

        fn terminate_unconfirmed(&mut self, _request_id: ProcessRequestId) -> Result<(), String> {
            Ok(())
        }

        fn reap_exited(&mut self) -> Vec<ManagedExit> {
            Vec::new()
        }
    }

    #[test]
    fn spawn_request_returns_while_systemd_worker_is_blocked() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut controller = LocalProcessController::with_manager(GatedProcessManager {
            started: started_tx,
            release: release_rx,
        });
        let (request_id_tx, request_id_rx) = mpsc::channel();
        let (return_controller_tx, return_controller_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let spec = ProcessSpec {
            command: "/bin/true".to_string(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        };
        let caller = thread::spawn(move || {
            let request_id = controller.spawn(&spec, &BTreeMap::new()).unwrap();
            request_id_tx.send(request_id).unwrap();
            continue_rx.recv().unwrap();
            return_controller_tx.send(controller).unwrap();
        });

        let started_at = Instant::now();
        let request_id = request_id_rx
            .recv_timeout(Duration::from_millis(300))
            .expect("spawn request is enqueued without waiting for systemd");
        started_rx
            .recv_timeout(Duration::from_millis(300))
            .expect("background worker reaches the gated systemd operation");
        assert!(started_at.elapsed() < Duration::from_millis(300));

        release_tx.send(()).expect("release background worker");
        continue_tx.send(()).expect("return the controller");
        let mut controller = return_controller_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("caller returns controller");
        caller.join().expect("caller thread completes");

        let deadline = Instant::now() + Duration::from_secs(1);
        let event = loop {
            if let Some(event) = controller.poll_events().into_iter().next() {
                break event;
            }
            if Instant::now() >= deadline {
                panic!("worker result event was not returned");
            }
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            event,
            ProcessControllerEvent::SpawnStarted {
                request_id,
                pid: 77,
            }
        );
    }

    #[test]
    fn process_worker_keeps_known_pid_on_an_unconfirmed_started_event() {
        let mut controller = LocalProcessController::with_manager(KeptRunningStartManager);
        let spec = ProcessSpec {
            command: "/bin/true".to_string(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        };
        let request_id = controller
            .spawn(&spec, &BTreeMap::new())
            .expect("spawn queues");
        let deadline = Instant::now() + Duration::from_secs(1);
        let event = loop {
            if let Some(event) = controller.poll_events().into_iter().next() {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "worker result event was not returned"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            event,
            ProcessControllerEvent::SpawnStartedUnconfirmed {
                request_id,
                pid: 77,
                observation: ProcessLaunchObservation::ChildKeptRunning,
            }
        );
    }

    #[test]
    fn terminate_request_returns_while_systemd_worker_is_blocked() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut controller = LocalProcessController::with_manager(GatedStopManager {
            started: started_tx,
            release: release_rx,
        });

        let started_at = Instant::now();
        controller
            .terminate(77)
            .expect("stop request should be enqueued");
        assert!(started_at.elapsed() < Duration::from_millis(300));
        started_rx
            .recv_timeout(Duration::from_millis(300))
            .expect("background worker reaches the gated systemd operation");

        release_tx.send(()).expect("release background worker");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(ProcessControllerEvent::Terminated {
                pid: 77,
                result: Ok(()),
            }) = controller.poll_events().into_iter().next()
            {
                break;
            }
            if Instant::now() >= deadline {
                panic!("worker stop result event was not returned");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn unit_component_encoding_is_non_lossy_and_keeps_systemd_safe_bytes() {
        let source = b"foot helper-\xff";
        let escaped = escape_unit_component(source);
        assert_eq!(escaped, r"foot\x20helper\x2d\xff");
        assert_eq!(unescape_unit_component(&escaped), source);
        assert_eq!(
            format!("surface0-{escaped}.service").len() <= UNIT_NAME_MAX_BYTES,
            true
        );
    }

    #[test]
    fn overlong_child_name_is_rejected_without_truncation_or_aliasing() {
        let escaped = escape_unit_component(&vec![b'x'; 240]);
        assert_eq!(max_slot_for_child(&escaped), None);
    }

    #[test]
    fn overlong_unit_component_is_shortened_deterministically_with_a_name_fingerprint() {
        let basename = vec![b'x'; UNIT_NAME_MAX_BYTES + 8];
        let shortened = shorten_unit_component(&basename, 0);
        let unit_name = format!("surface0-{shortened}.service");
        assert!(unit_name.len() <= UNIT_NAME_MAX_BYTES);
        assert_eq!(shortened, shorten_unit_component(&basename, 0));

        let mut digest = 0xcbf29ce484222325_u64;
        for byte in &basename {
            digest ^= u64::from(*byte);
            digest = digest.wrapping_mul(0x100000001b3);
        }
        let suffix = format!("-{digest:016x}");
        assert!(shortened.ends_with(&suffix));
        assert_ne!(
            shorten_unit_component(&basename, 0),
            shorten_unit_component(&[basename.as_slice(), b"different"].concat(), 0)
        );

        let mut escaped_basename = vec![b'x'; UNIT_NAME_MAX_BYTES];
        escaped_basename.push(0xff);
        let escaped = shorten_unit_component(&escaped_basename, 3);
        let hash_suffix_len = 17;
        let prefix = &escaped[..escaped.len() - hash_suffix_len];
        assert!(!prefix.ends_with('\\'));
        assert!(!prefix.ends_with("\\x"));
        assert!(!prefix.ends_with("\\x0"));
        assert!(format!("surface3-{escaped}.service").len() <= UNIT_NAME_MAX_BYTES);
    }

    #[test]
    fn main_slot_is_preferred_then_collision_falls_back_to_unused_positive_slot() {
        let occupied = HashSet::new();
        let attempted = HashSet::new();
        assert_eq!(next_candidate_slot(true, 4, &occupied, &attempted), Some(0));
        let attempted = HashSet::from([0]);
        assert_eq!(next_candidate_slot(true, 4, &occupied, &attempted), Some(1));
        let occupied = HashSet::from([1, 2]);
        assert_eq!(
            next_candidate_slot(false, 4, &occupied, &HashSet::new()),
            Some(3)
        );
    }

    #[test]
    fn slot_is_reserved_across_different_child_names() {
        assert_eq!(unit_slot_for_name("surface1-foot.service"), Some(1));
        assert_eq!(unit_slot_for_name("surface1-other.service"), Some(1));
        assert_eq!(unit_slot_for_name("surface1-foo\\x2dbar.service"), Some(1));

        let occupied = HashSet::from([unit_slot_for_name("surface1-foot.service").unwrap()]);
        assert_eq!(
            next_candidate_slot(false, 3, &occupied, &HashSet::new()),
            Some(2)
        );
    }

    #[test]
    fn bounded_candidate_space_reports_exhaustion_without_reusing_a_unit() {
        let occupied = HashSet::from([0, 1, 2, 3]);
        let attempted = HashSet::new();
        assert_eq!(next_candidate_slot(true, 3, &occupied, &attempted), None);
    }

    #[test]
    fn r6_invalid_and_exhausted_names_warn_and_unitless_launch_discards_all_standard_fds() {
        let invalid = unitless_warning(
            "unit_name_invalid",
            "requested executable has no non-empty basename",
            &[],
        );
        assert_eq!(invalid.stage, "unit_name_invalid");
        assert_eq!(
            invalid.reason,
            "requested executable has no non-empty basename unitless_launch"
        );
        assert_warning_submitted(0, "unavailable", &invalid, &[]);

        let unit_creation_failure = unitless_warning(
            "unit_creation_failure",
            "systemd_run_unavailable_or_failed_to_start",
            &[],
        );
        assert_eq!(unit_creation_failure.stage, "unit_creation_failure");
        assert!(unit_creation_failure.reason.contains("unitless_launch"));
        assert_warning_submitted(0, "surface0-sh.service", &unit_creation_failure, &[]);

        let exhausted = unitless_warning(
            "unit_name_exhausted",
            "no valid unused child unit name remains",
            &[],
        );
        assert_eq!(exhausted.stage, "unit_name_exhausted");
        assert!(exhausted.reason.contains("unitless_launch"));
        assert_warning_submitted(0, "unavailable", &exhausted, &[]);

        let fd_report =
            std::env::temp_dir().join(format!("surf-ace-unitless-fds-{}", std::process::id()));
        let spec = ProcessSpec {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "for fd in 0 1 2; do target=$(/usr/bin/readlink \"/proc/$$/fd/$fd\") || exit 91; printf '%s=%s\\n' \"$fd\" \"$target\" >> \"$FD_REPORT\"; done".to_string(),
            ],
            cwd: None,
            env: BTreeMap::from([(
                "FD_REPORT".to_string(),
                fd_report.to_string_lossy().into_owned(),
            )]),
        };
        let environment = child_supervisor::effective_child_environment(&spec, &BTreeMap::new());
        let status = unitless_command(&spec, &environment)
            .status()
            .expect("unitless child starts");
        let observed_fds = std::fs::read_to_string(&fd_report).expect("child reports fd targets");
        let _ = std::fs::remove_file(&fd_report);
        assert!(status.success(), "child exits successfully: {observed_fds}");
        assert_eq!(observed_fds, "0=/dev/null\n1=/dev/null\n2=/dev/null\n");
    }

    #[test]
    fn r6_shortened_name_warning_names_original_basename_and_sanitizes_it() {
        let original = "program-synthetic-name-secret";
        let secrets = vec!["synthetic-name-secret".to_string()];
        let warning = shortened_unit_warning(original, &secrets);
        assert_eq!(warning.stage, "unit_name_shortened");
        assert_eq!(warning.reason, "original_basename=program-[REDACTED]");
        assert!(!warning.reason.contains("synthetic-name-secret"));
        assert_warning_submitted(0, "surface0-program.service", &warning, &secrets);
    }

    #[test]
    fn r7_pid_collision_replaces_old_tracking_and_records_both_units() {
        let mut tracked = HashMap::from([(77_u32, "surface1-old.service")]);
        let previous =
            replace_stale_pid_tracking(&mut tracked, 77, "surface0-new.service", |old| {
                old.to_string()
            })
            .expect("prior PID tracking is replaced");
        assert_eq!(previous, "surface1-old.service");
        assert_eq!(tracked.get(&77), Some(&"surface0-new.service"));

        let warning = application_pid_collision_warning(&previous, "surface0-new.service", &[]);
        assert_eq!(warning.stage, "application_pid_collision");
        assert_eq!(
            warning.reason,
            "stale_tracking_replaced previous_unit=surface1-old.service new_unit=surface0-new.service"
        );
        assert_warning_submitted(0, "surface0-new.service", &warning, &[]);
    }

    #[test]
    fn r9_stage_three_and_four_emit_warnings_and_keep_the_correct_observation() {
        assert_eq!(
            started_child_observation(false, true),
            Some(ProcessLaunchObservation::ChildKeptRunning)
        );
        assert_eq!(
            started_child_observation(true, false),
            Some(ProcessLaunchObservation::ChildKeptRunning)
        );
        assert_eq!(started_child_observation(true, true), None);

        let unit_warning = child_kept_running_warning(
            "unit_observation_failure",
            "transient_unit_or_sanitizer_path_not_observed",
            &[],
        );
        assert_eq!(unit_warning.stage, "unit_observation_failure");
        assert!(unit_warning.reason.contains("child_kept_running"));
        assert_warning_submitted(1, "surface1-unobserved.service", &unit_warning, &[]);

        let reader_warning = child_kept_running_warning(
            "supervisor_protocol_failure",
            "supervisor_event_reader_start_failure",
            &[],
        );
        assert_eq!(reader_warning.stage, "supervisor_protocol_failure");
        assert_eq!(
            reader_warning.reason,
            "child_kept_running supervisor_event_reader_start_failure"
        );
        assert_warning_submitted(1, "surface1-reader.service", &reader_warning, &[]);

        let unconfirmed = child_start_unconfirmed_warning(
            "child_start_unconfirmed",
            "supervisor_started_event_missing",
            &[],
        );
        assert_eq!(unconfirmed.stage, "child_start_unconfirmed");
        assert!(unconfirmed.reason.contains("child_start_unconfirmed"));
        assert_warning_submitted(2, "surface2-unconfirmed.service", &unconfirmed, &[]);
    }

    #[test]
    fn collision_errors_are_classified_without_forwarding_manager_text() {
        assert!(is_unit_name_collision(
            b"Unit surface0-foot.service already exists."
        ));
        assert!(!is_unit_name_collision(
            b"Failed to connect to bus: permission denied"
        ));
    }

    fn unescape_unit_component(encoded: &str) -> Vec<u8> {
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\\'
                && bytes.get(index + 1) == Some(&b'x')
                && index + 3 < bytes.len()
            {
                let hex = std::str::from_utf8(&bytes[index + 2..index + 4]).expect("hex is ascii");
                decoded.push(u8::from_str_radix(hex, 16).expect("escape is valid hex"));
                index += 4;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        decoded
    }
}
