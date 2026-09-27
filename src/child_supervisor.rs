use crate::model::ProcessSpec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub nonce: String,
    pub unit_name: String,
    pub slot: u32,
    pub spec: ProcessSpec,
    pub environment: Vec<EnvironmentEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperHello {
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum SupervisorCommand {
    AcceptLaunch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SupervisorEvent {
    Started {
        pid: u32,
        supervisor_pid: u32,
    },
    ChildExited {
        exit_code: Option<i32>,
        sanitizer_failed: bool,
    },
    StartFailed {
        reason: String,
    },
    SanitizerFailed {
        reason: String,
    },
    Finished {
        exit_code: Option<i32>,
        sanitizer_failed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdout,
    Stderr,
}

impl StreamKind {
    fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PumpFailure {
    InvalidUtf8,
    RecordTooLong,
    ReadFailure,
    JournalWriteFailure,
}

impl PumpFailure {
    fn label(self) -> &'static str {
        match self {
            Self::InvalidUtf8 => "invalid_utf8",
            Self::RecordTooLong => "record_too_long",
            Self::ReadFailure => "stream_read_failure",
            Self::JournalWriteFailure => "journal_write_failure",
        }
    }
}

pub struct RunningChild {
    child: Child,
    pid: u32,
    unit_name: String,
    slot: u32,
    failed: Arc<AtomicBool>,
    failure_rx: Receiver<(StreamKind, PumpFailure)>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildOutcome {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub sanitizer_failed: bool,
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "supervisor frame exceeds the bounded launch protocol",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "supervisor frame length is invalid",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}

pub fn run_supervisor(socket_path: &str, nonce: &str, unit_name: &str, slot: u32) -> i32 {
    let result = run_supervisor_session(socket_path, nonce, unit_name, slot);
    result.unwrap_or(125)
}

fn run_supervisor_session(
    socket_path: &str,
    expected_nonce: &str,
    expected_unit: &str,
    expected_slot: u32,
) -> Result<i32, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|_| "supervisor_control_connect_failure".to_string())?;
    write_frame(
        &mut stream,
        &HelperHello {
            nonce: expected_nonce.to_string(),
        },
    )
    .map_err(|_| "supervisor_hello_write_failure".to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let request: LaunchRequest =
        read_frame(&mut stream).map_err(|_| "supervisor_request_read_failure".to_string())?;
    if request.nonce != expected_nonce
        || request.unit_name != expected_unit
        || request.slot != expected_slot
    {
        emit_unit_record(
            expected_unit,
            expected_slot,
            "supervisor_protocol_failure",
            "request_identity_mismatch",
            &[],
            StreamKind::Stderr,
        );
        let _ = write_frame(
            &mut stream,
            &SupervisorEvent::StartFailed {
                reason: "supervisor_request_identity_mismatch".to_string(),
            },
        );
        return Ok(125);
    }

    let secrets = sensitive_environment_values(&request.environment);
    let mut child = match start_child(&request, secrets.clone()) {
        Ok(child) => child,
        Err(reason) => {
            emit_unit_record(
                &request.unit_name,
                request.slot,
                "child_exec_failure",
                &reason,
                &secrets,
                StreamKind::Stderr,
            );
            let _ = write_frame(
                &mut stream,
                &SupervisorEvent::StartFailed {
                    reason: reason.clone(),
                },
            );
            return Ok(127);
        }
    };

    emit_unit_record(
        &request.unit_name,
        request.slot,
        "child_started",
        &format!("app_pid={}", child.pid),
        &secrets,
        StreamKind::Stdout,
    );
    if write_frame(
        &mut stream,
        &SupervisorEvent::Started {
            pid: child.pid,
            supervisor_pid: std::process::id(),
        },
    )
    .is_err()
    {
        terminate_process_group(child.pid, &mut child.child);
        let outcome = finish_child(&mut child, &secrets, &mut stream);
        emit_unit_record(
            &request.unit_name,
            request.slot,
            "child_launch_aborted",
            &format!(
                "app_pid={} reason=supervisor_started_event_write_failure",
                outcome.pid
            ),
            &secrets,
            StreamKind::Stderr,
        );
        return Ok(125);
    }

    let accepted = matches!(
        read_frame::<_, SupervisorCommand>(&mut stream),
        Ok(SupervisorCommand::AcceptLaunch)
    );
    if !accepted {
        terminate_process_group(child.pid, &mut child.child);
        let outcome = finish_child(&mut child, &secrets, &mut stream);
        emit_unit_record(
            &request.unit_name,
            request.slot,
            "child_launch_aborted",
            &format!("app_pid={} reason=parent_observation_failed", outcome.pid),
            &secrets,
            StreamKind::Stderr,
        );
        return Ok(125);
    }

    let outcome = finish_child(&mut child, &secrets, &mut stream);
    emit_unit_record(
        &request.unit_name,
        request.slot,
        "child_exit",
        &format!(
            "app_pid={} exit_code={}",
            outcome.pid,
            outcome
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        &secrets,
        StreamKind::Stdout,
    );
    let _ = write_frame(
        &mut stream,
        &SupervisorEvent::Finished {
            exit_code: outcome.exit_code,
            sanitizer_failed: outcome.sanitizer_failed,
        },
    );
    Ok(supervisor_exit_code(outcome))
}

fn supervisor_exit_code(outcome: ChildOutcome) -> i32 {
    if outcome.sanitizer_failed {
        125
    } else {
        outcome.exit_code.unwrap_or(1)
    }
}

fn start_child(request: &LaunchRequest, secrets: Vec<String>) -> Result<RunningChild, String> {
    let mut command = Command::new(&request.spec.command);
    command.args(&request.spec.args);
    command.env_clear();
    for entry in &request.environment {
        command.env(
            OsString::from_vec(entry.key.clone()),
            OsString::from_vec(entry.value.clone()),
        );
    }
    if let Some(cwd) = &request.spec.cwd {
        command.current_dir(cwd);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.process_group(0);

    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to execute requested child '{}': {error}",
            request.spec.command
        )
    })?;
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        terminate_process_group(pid, &mut child);
        let _ = child.wait();
        return Err("sanitizer_stdout_pipe_unavailable".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_process_group(pid, &mut child);
        let _ = child.wait();
        return Err("sanitizer_stderr_pipe_unavailable".to_string());
    };
    let failed = Arc::new(AtomicBool::new(false));
    let (failure_tx, failure_rx) = mpsc::channel();
    let stdout_failed = failed.clone();
    let stdout_tx = failure_tx.clone();
    let unit_for_stdout = request.unit_name.clone();
    let secrets_for_stdout = secrets.clone();
    let stdout_thread = match thread::Builder::new()
        .name("child-stdout-sanitizer".to_string())
        .spawn(move || {
            pump_stream(
                stdout,
                io::stdout(),
                &unit_for_stdout,
                StreamKind::Stdout,
                &secrets_for_stdout,
                &stdout_tx,
                &stdout_failed,
            );
        }) {
        Ok(thread) => thread,
        Err(_) => {
            terminate_process_group(pid, &mut child);
            let _ = child.wait();
            return Err("sanitizer_stdout_start_failure".to_string());
        }
    };
    let stderr_failed = failed.clone();
    let unit_for_stderr = request.unit_name.clone();
    let stderr_thread = match thread::Builder::new()
        .name("child-stderr-sanitizer".to_string())
        .spawn(move || {
            pump_stream(
                stderr,
                io::stderr(),
                &unit_for_stderr,
                StreamKind::Stderr,
                &secrets,
                &failure_tx,
                &stderr_failed,
            );
        }) {
        Ok(thread) => thread,
        Err(_) => {
            let _ = rustix::process::kill_process_group(
                rustix::process::Pid::from_raw(pid as i32)
                    .expect("a spawned child pid is positive"),
                rustix::process::Signal::KILL,
            );
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_thread.join();
            return Err("sanitizer_stderr_start_failure".to_string());
        }
    };

    Ok(RunningChild {
        child,
        pid,
        unit_name: request.unit_name.clone(),
        slot: request.slot,
        failed,
        failure_rx,
        stdout_thread: Some(stdout_thread),
        stderr_thread: Some(stderr_thread),
    })
}

fn finish_child(
    child: &mut RunningChild,
    secrets: &[String],
    stream: &mut UnixStream,
) -> ChildOutcome {
    let mut exit_code = None;
    let mut child_reaped = false;
    let mut sanitizer_failed = false;
    let mut failure_reported = false;
    let mut child_exit_reported = false;

    while !child_reaped || !pump_threads_finished(child) {
        if !sanitizer_failed {
            if let Ok((stream_kind, failure)) = child.failure_rx.try_recv() {
                sanitizer_failed = true;
                child.failed.store(true, Ordering::Release);
                if !failure_reported {
                    let reason =
                        format!("stream={} reason={}", stream_kind.label(), failure.label());
                    emit_unit_record(
                        &child.unit_name,
                        child.slot,
                        "sanitizer_failure",
                        &reason,
                        secrets,
                        StreamKind::Stderr,
                    );
                    let _ = write_frame(
                        stream,
                        &SupervisorEvent::SanitizerFailed {
                            reason: reason.clone(),
                        },
                    );
                    failure_reported = true;
                }
                terminate_process_group(child.pid, &mut child.child);
            }
        } else {
            while child.failure_rx.try_recv().is_ok() {}
        }

        if !child_reaped {
            match child.child.try_wait() {
                Ok(Some(status)) => {
                    exit_code = status.code();
                    child_reaped = true;
                    let _ = write_frame(
                        stream,
                        &SupervisorEvent::ChildExited {
                            exit_code,
                            sanitizer_failed,
                        },
                    );
                    child_exit_reported = true;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    sanitizer_failed = true;
                    terminate_process_group(child.pid, &mut child.child);
                    let _ = child.child.wait();
                    child_reaped = true;
                }
            }
        } else if !pump_threads_finished(child) {
            thread::sleep(Duration::from_millis(10));
        }
    }

    let stdout_joined = child
        .stdout_thread
        .take()
        .is_some_and(|thread| thread.join().is_ok());
    let stderr_joined = child
        .stderr_thread
        .take()
        .is_some_and(|thread| thread.join().is_ok());
    if let Ok((stream_kind, failure)) = child.failure_rx.try_recv() {
        sanitizer_failed = true;
        terminate_process_group(child.pid, &mut child.child);
        if !failure_reported {
            let reason = format!("stream={} reason={}", stream_kind.label(), failure.label());
            emit_unit_record(
                &child.unit_name,
                child.slot,
                "sanitizer_failure",
                &reason,
                secrets,
                StreamKind::Stderr,
            );
            let _ = write_frame(stream, &SupervisorEvent::SanitizerFailed { reason });
        }
    }
    if !stdout_joined || !stderr_joined {
        sanitizer_failed = true;
        terminate_process_group(child.pid, &mut child.child);
        if !failure_reported {
            emit_unit_record(
                &child.unit_name,
                child.slot,
                "sanitizer_failure",
                "reason=sanitizer_thread_failure",
                secrets,
                StreamKind::Stderr,
            );
            let _ = write_frame(
                stream,
                &SupervisorEvent::SanitizerFailed {
                    reason: "reason=sanitizer_thread_failure".to_string(),
                },
            );
        }
    }
    if child_reaped && !child_exit_reported {
        let _ = write_frame(
            stream,
            &SupervisorEvent::ChildExited {
                exit_code,
                sanitizer_failed,
            },
        );
    }

    ChildOutcome {
        pid: child.pid,
        exit_code,
        sanitizer_failed,
    }
}

fn pump_threads_finished(child: &RunningChild) -> bool {
    child
        .stdout_thread
        .as_ref()
        .map(JoinHandle::is_finished)
        .unwrap_or(true)
        && child
            .stderr_thread
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
}

fn terminate_process_group(pid: u32, child: &mut Child) {
    if let Some(group) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
}

fn pump_stream<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    unit_name: &str,
    stream: StreamKind,
    secrets: &[String],
    failure_tx: &Sender<(StreamKind, PumpFailure)>,
    failed: &AtomicBool,
) {
    let mut record = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut discarding = false;
    let mut reported = false;
    loop {
        let count = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                report_pump_failure(
                    &mut writer,
                    unit_name,
                    stream,
                    PumpFailure::ReadFailure,
                    secrets,
                    failure_tx,
                    failed,
                    &mut reported,
                );
                break;
            }
        };

        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if !discarding && !reported {
                    if let Err(failure) =
                        emit_record_or_discard(&mut writer, &record, unit_name, stream, secrets)
                    {
                        report_pump_failure(
                            &mut writer,
                            unit_name,
                            stream,
                            failure,
                            secrets,
                            failure_tx,
                            failed,
                            &mut reported,
                        );
                    }
                }
                record.clear();
                discarding = false;
                continue;
            }

            if reported || discarding {
                continue;
            }
            if record.len() == MAX_RECORD_BYTES {
                record.clear();
                discarding = true;
                if let Err(failure) = emit_discarded_record(
                    &mut writer,
                    unit_name,
                    stream,
                    PumpFailure::RecordTooLong,
                ) {
                    report_pump_failure(
                        &mut writer,
                        unit_name,
                        stream,
                        failure,
                        secrets,
                        failure_tx,
                        failed,
                        &mut reported,
                    );
                }
                continue;
            }
            record.push(*byte);
        }
    }

    if !reported && !discarding && !record.is_empty() {
        if let Err(failure) =
            emit_record_or_discard(&mut writer, &record, unit_name, stream, secrets)
        {
            report_pump_failure(
                &mut writer,
                unit_name,
                stream,
                failure,
                secrets,
                failure_tx,
                failed,
                &mut reported,
            );
        }
    }
}

fn emit_record<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    unit_name: &str,
    stream: StreamKind,
    secrets: &[String],
) -> Result<(), PumpFailure> {
    let Ok(record) = std::str::from_utf8(bytes) else {
        return Err(PumpFailure::InvalidUtf8);
    };
    let safe = sanitize_record(record, secrets);
    writer
        .write_all(format!("unit={unit_name} stream={} {safe}\n", stream.label()).as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|_| PumpFailure::JournalWriteFailure)
}

fn emit_record_or_discard<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    unit_name: &str,
    stream: StreamKind,
    secrets: &[String],
) -> Result<(), PumpFailure> {
    match emit_record(writer, bytes, unit_name, stream, secrets) {
        Err(PumpFailure::InvalidUtf8) => {
            emit_discarded_record(writer, unit_name, stream, PumpFailure::InvalidUtf8)
        }
        result => result,
    }
}

fn emit_discarded_record<W: Write>(
    writer: &mut W,
    unit_name: &str,
    stream: StreamKind,
    reason: PumpFailure,
) -> Result<(), PumpFailure> {
    let reason = match reason {
        PumpFailure::InvalidUtf8 => "invalid_utf8",
        PumpFailure::RecordTooLong => "record_too_long",
        PumpFailure::ReadFailure | PumpFailure::JournalWriteFailure => {
            return Err(reason);
        }
    };
    let message = format!(
        "unit={unit_name} stream={} stage=output_record_discarded reason={reason}\n",
        stream.label()
    );
    writer
        .write_all(message.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|_| PumpFailure::JournalWriteFailure)
}

fn report_pump_failure<W: Write>(
    writer: &mut W,
    unit_name: &str,
    stream: StreamKind,
    failure: PumpFailure,
    secrets: &[String],
    failure_tx: &Sender<(StreamKind, PumpFailure)>,
    failed: &AtomicBool,
    reported: &mut bool,
) {
    if *reported {
        return;
    }
    *reported = true;
    failed.store(true, Ordering::Release);
    let message = format!(
        "unit={unit_name} stream={} stage=sanitizer_failure reason={}",
        stream.label(),
        failure.label()
    );
    let safe = sanitize_record(&message, secrets);
    let _ = writer
        .write_all(format!("{safe}\n").as_bytes())
        .and_then(|()| writer.flush());
    let _ = failure_tx.send((stream, failure));
}

pub fn sanitize_record(record: &str, secrets: &[String]) -> String {
    let mut sanitized = record.to_string();
    let mut sorted_secrets: Vec<_> = secrets.iter().filter(|secret| !secret.is_empty()).collect();
    sorted_secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in sorted_secrets {
        sanitized = sanitized.replace(secret, "[REDACTED]");
    }
    sanitized = redact_sensitive_assignments(&sanitized);
    sanitized = redact_bearer_values(&sanitized);
    redact_uri_passwords(&sanitized)
}

pub fn sensitive_environment_values(environment: &[EnvironmentEntry]) -> Vec<String> {
    let mut values = Vec::new();
    for entry in environment {
        let Ok(key) = std::str::from_utf8(&entry.key) else {
            continue;
        };
        if !has_sensitive_key(key) {
            continue;
        }
        if let Ok(value) = std::str::from_utf8(&entry.value)
            && !value.is_empty()
        {
            values.push(value.to_string());
        }
    }
    values
}

fn has_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "api-key",
        "credential",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn redact_sensitive_assignments(record: &str) -> String {
    let bytes = record.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let boundary = index == 0 || !is_key_byte(bytes[index - 1]);
        if boundary && is_key_byte(bytes[index]) {
            let key_start = index;
            while index < bytes.len() && is_key_byte(bytes[index]) {
                index += 1;
            }
            let key_end = index;
            let key = std::str::from_utf8(&bytes[key_start..key_end]).unwrap_or_default();
            if has_sensitive_key(key) {
                let mut separator = index;
                while separator < bytes.len() && bytes[separator].is_ascii_whitespace() {
                    separator += 1;
                }
                if separator < bytes.len() && matches!(bytes[separator], b'\'' | b'"') {
                    separator += 1;
                    while separator < bytes.len() && bytes[separator].is_ascii_whitespace() {
                        separator += 1;
                    }
                }
                if separator < bytes.len() && (bytes[separator] == b'=' || bytes[separator] == b':')
                {
                    output.extend_from_slice(&bytes[key_start..=separator]);
                    index = separator + 1;
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        output.push(bytes[index]);
                        index += 1;
                    }
                    let (replacement_end, replacement) =
                        sensitive_value_replacement(bytes, index, key);
                    output.extend_from_slice(replacement.as_bytes());
                    index = replacement_end;
                    continue;
                }
            }
            output.extend_from_slice(&bytes[key_start..key_end]);
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| "[REDACTED]".to_string())
}

fn sensitive_value_replacement(bytes: &[u8], start: usize, key: &str) -> (usize, String) {
    if start >= bytes.len() {
        return (start, "[REDACTED]".to_string());
    }
    if bytes[start] == b'\'' || bytes[start] == b'"' {
        let quote = bytes[start];
        let mut end = start + 1;
        while end < bytes.len() && bytes[end] != quote {
            end += 1;
        }
        let after = if end < bytes.len() { end + 1 } else { end };
        return (
            after,
            format!(
                "{}[REDACTED]{}",
                quote as char,
                if after > end {
                    (quote as char).to_string()
                } else {
                    String::new()
                }
            ),
        );
    }

    if bytes[start..]
        .get(..7)
        .is_some_and(|value| value.eq_ignore_ascii_case(b"Bearer "))
    {
        let token_start = start + 7;
        let token_end = scan_token(bytes, token_start);
        if token_end > token_start {
            return (token_end, format!("Bearer [REDACTED]"));
        }
    }

    if key.to_ascii_lowercase().contains("cookie") {
        return (bytes.len(), "[REDACTED]".to_string());
    }

    if key.to_ascii_lowercase().contains("authorization") {
        return (bytes.len(), "[REDACTED]".to_string());
    }

    let mut end = start;
    while end < bytes.len() && !is_value_boundary(bytes[end]) {
        end += 1;
    }
    if end == start {
        (start, "[REDACTED]".to_string())
    } else {
        (end, "[REDACTED]".to_string())
    }
}

fn redact_bearer_values(record: &str) -> String {
    let bytes = record.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 6 <= bytes.len()
            && bytes[index..index + 6].eq_ignore_ascii_case(b"Bearer")
            && (index == 0 || !is_key_byte(bytes[index - 1]))
            && (index + 6 == bytes.len() || bytes[index + 6].is_ascii_whitespace())
        {
            output.extend_from_slice(&bytes[index..index + 6]);
            index += 6;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                output.push(bytes[index]);
                index += 1;
            }
            let end = scan_token(bytes, index);
            if end > index {
                output.extend_from_slice(b"[REDACTED]");
                index = end;
            }
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| "[REDACTED]".to_string())
}

fn redact_uri_passwords(record: &str) -> String {
    let mut current = record.to_string();
    let mut search_from = 0;
    while let Some(scheme_end) = current[search_from..].find("://").map(|i| i + search_from) {
        let authority_start = scheme_end + 3;
        let authority_end = current[authority_start..]
            .find(|character: char| character.is_ascii_whitespace() || "/?#".contains(character))
            .map(|offset| authority_start + offset)
            .unwrap_or(current.len());
        let authority = &current[authority_start..authority_end];
        if let Some(at_offset) = authority.rfind('@') {
            let userinfo_end = authority_start + at_offset;
            let userinfo = &current[authority_start..userinfo_end];
            if let Some(password_separator) = userinfo.find(':') {
                let password_start = authority_start + password_separator + 1;
                current.replace_range(password_start..userinfo_end, "[REDACTED]");
                search_from = password_start + "[REDACTED]".len();
                continue;
            }
        }
        search_from = authority_end;
        if search_from >= current.len() {
            break;
        }
    }
    current
}

fn scan_token(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && is_bearer_byte(bytes[index]) {
        index += 1;
    }
    index
}

fn is_bearer_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte)
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn is_value_boundary(byte: u8) -> bool {
    byte.is_ascii_whitespace() || b",;)]}".contains(&byte)
}

fn emit_unit_record(
    unit_name: &str,
    slot: u32,
    stage: &str,
    reason: &str,
    secrets: &[String],
    stream: StreamKind,
) {
    let record = format!(
        "unit={unit_name} slot={slot} stage={stage} stream={} timestamp_ms={} reason={reason}",
        stream.label(),
        unix_millis()
    );
    let safe = sanitize_record(&record, secrets);
    match stream {
        StreamKind::Stdout => {
            let mut output = io::stdout();
            let _ = output
                .write_all(format!("{safe}\n").as_bytes())
                .and_then(|()| output.flush());
        }
        StreamKind::Stderr => {
            let mut output = io::stderr();
            let _ = output
                .write_all(format!("{safe}\n").as_bytes())
                .and_then(|()| output.flush());
        }
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub fn effective_child_environment(
    spec: &ProcessSpec,
    extra_env: &BTreeMap<String, String>,
) -> Vec<EnvironmentEntry> {
    let mut environment: BTreeMap<Vec<u8>, Vec<u8>> = std::env::vars_os()
        .map(|(key, value)| {
            (
                key.as_os_str().as_bytes().to_vec(),
                value.as_os_str().as_bytes().to_vec(),
            )
        })
        .collect();
    for (key, value) in &spec.env {
        environment.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
    }
    for (key, value) in extra_env {
        environment.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
    }
    environment
        .into_iter()
        .map(|(key, value)| EnvironmentEntry { key, value })
        .collect()
}

pub fn emit_compositor_diagnostic(
    slot: u32,
    unit_name: &str,
    stage: &str,
    reason: &str,
    secrets: &[String],
) -> bool {
    let record = format!(
        "timestamp_ms={} unit={unit_name} slot={slot} stage={stage} reason={reason}",
        unix_millis()
    );
    let safe = sanitize_record(&record, secrets);
    let Ok(mut logger) = Command::new("systemd-cat")
        .arg("--identifier=surf-ace-compositor")
        .arg("--level-prefix=false")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut stdin) = logger.stdin.take() else {
        let _ = logger.kill();
        let _ = logger.wait();
        return false;
    };
    let write_result = stdin
        .write_all(safe.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"));
    drop(stdin);
    write_result.is_ok() && logger.wait().is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sanitizer_redacts_required_credential_shapes_and_preserves_safe_text() {
        let record = "keep Authorization: Bearer t285-synthetic-secret url=https://alice:uri-password@example.test/path token='env secret value' end";
        let safe = sanitize_record(record, &[]);
        assert!(safe.contains("keep Authorization: Bearer [REDACTED]"));
        assert!(safe.contains("https://alice:[REDACTED]@example.test/path"));
        assert!(safe.contains("token='[REDACTED]'"));
        assert!(safe.contains("end"));
        assert!(!safe.contains("t285-synthetic-secret"));
        assert!(!safe.contains("uri-password"));
        assert!(!safe.contains("env secret value"));
    }

    #[test]
    fn sanitizer_redacts_quoted_keys_and_the_entire_cookie_header_value() {
        let json = sanitize_record(r#"{"password":"hunter2","safe":"visible"}"#, &[]);
        assert_eq!(json, r#"{"password":"[REDACTED]","safe":"visible"}"#);

        let yaml = sanitize_record("'api_key' : 'synthetic-key' safe: visible", &[]);
        assert_eq!(yaml, "'api_key' : '[REDACTED]' safe: visible");

        let cookie = sanitize_record("Cookie: theme=dark; sid=synthetic-cookie", &[]);
        assert_eq!(cookie, "Cookie: [REDACTED]");
        assert!(!cookie.contains("synthetic-cookie"));
    }

    #[test]
    fn sanitizer_redacts_exact_launch_token_even_without_sensitive_assignment() {
        let safe = sanitize_record(
            "application reported opaque-value in diagnostics",
            &["opaque-value".to_string()],
        );
        assert_eq!(safe, "application reported [REDACTED] in diagnostics");
    }

    #[test]
    fn stream_sanitizer_discards_invalid_and_oversized_records_but_keeps_safe_records() {
        let (failure_tx, failure_rx) = mpsc::channel();
        let failed = AtomicBool::new(false);
        let mut output = Vec::new();
        pump_stream(
            Cursor::new(b"token=t285-secret\nfinal=visible".to_vec()),
            &mut output,
            "surface1-foot.service",
            StreamKind::Stderr,
            &[],
            &failure_tx,
            &failed,
        );
        let output = String::from_utf8(output).expect("sanitized output is utf-8");
        assert!(output.contains("token=[REDACTED]"));
        assert!(output.contains("final=visible"));
        assert!(!output.contains("t285-secret"));
        assert!(failure_rx.try_recv().is_err());

        let (failure_tx, failure_rx) = mpsc::channel();
        let failed = AtomicBool::new(false);
        let mut output = Vec::new();
        let mut oversized = vec![b'x'; MAX_RECORD_BYTES + 1];
        oversized.extend_from_slice(b"\nnext=visible\n");
        pump_stream(
            Cursor::new(oversized),
            &mut output,
            "surface1-foot.service",
            StreamKind::Stdout,
            &["t285-secret".to_string()],
            &failure_tx,
            &failed,
        );
        let output = String::from_utf8(output).expect("discard marker is utf-8");
        assert!(output.contains("stage=output_record_discarded reason=record_too_long"));
        assert!(output.contains("next=visible"));
        assert!(!failed.load(Ordering::Acquire));
        assert!(failure_rx.try_recv().is_err());
        assert!(!output.contains(&"x".repeat(MAX_RECORD_BYTES)));

        let mut invalid = b"prefix=".to_vec();
        invalid.push(0xff);
        invalid.extend_from_slice(b"synthetic-value\nnext=also-visible");
        let (failure_tx, failure_rx) = mpsc::channel();
        let failed = AtomicBool::new(false);
        let mut output = Vec::new();
        pump_stream(
            Cursor::new(invalid),
            &mut output,
            "surface1-foot.service",
            StreamKind::Stderr,
            &[],
            &failure_tx,
            &failed,
        );
        let output = String::from_utf8(output).expect("invalid-record marker is utf-8");
        assert!(output.contains("stage=output_record_discarded reason=invalid_utf8"));
        assert!(output.contains("next=also-visible"));
        assert!(!output.contains("synthetic-value"));
        assert!(!failed.load(Ordering::Acquire));
        assert!(failure_rx.try_recv().is_err());
    }

    #[test]
    fn child_exit_is_reported_before_descendant_closes_output_pipes() {
        let spec = ProcessSpec {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "sleep 1 & exit 7".to_string()],
            cwd: None,
            env: BTreeMap::new(),
        };
        let request = LaunchRequest {
            nonce: "test-nonce".to_string(),
            unit_name: "surface1-sh.service".to_string(),
            slot: 1,
            environment: effective_child_environment(&spec, &BTreeMap::new()),
            spec,
        };
        let secrets = sensitive_environment_values(&request.environment);
        let mut child = start_child(&request, secrets.clone()).expect("shell child starts");
        let (mut supervisor, mut parent) = UnixStream::pair().expect("socket pair");
        parent
            .set_read_timeout(Some(Duration::from_millis(700)))
            .expect("read timeout");
        let started_at = std::time::Instant::now();
        let finisher = thread::spawn(move || finish_child(&mut child, &secrets, &mut supervisor));

        let first: SupervisorEvent = read_frame(&mut parent).expect("early child exit event");
        assert!(matches!(
            first,
            SupervisorEvent::ChildExited {
                exit_code: Some(7),
                sanitizer_failed: false,
            }
        ));
        assert!(started_at.elapsed() < Duration::from_millis(700));

        let outcome = finisher
            .join()
            .expect("supervisor drains descendant output");
        assert_eq!(outcome.exit_code, Some(7));
        assert!(read_frame::<_, SupervisorEvent>(&mut parent).is_err());
    }

    #[test]
    fn frame_protocol_rejects_zero_and_oversized_lengths() {
        let mut zero = Cursor::new(0_u32.to_be_bytes().to_vec());
        assert!(read_frame::<_, HelperHello>(&mut zero).is_err());
        let mut oversized = Cursor::new(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec());
        assert!(read_frame::<_, HelperHello>(&mut oversized).is_err());
    }

    #[test]
    fn sanitizer_failure_keeps_child_status_and_fails_the_unit() {
        assert_eq!(
            supervisor_exit_code(ChildOutcome {
                pid: 12,
                exit_code: Some(23),
                sanitizer_failed: false,
            }),
            23
        );
        assert_eq!(
            supervisor_exit_code(ChildOutcome {
                pid: 12,
                exit_code: Some(0),
                sanitizer_failed: true,
            }),
            125
        );
        assert_eq!(
            supervisor_exit_code(ChildOutcome {
                pid: 12,
                exit_code: None,
                sanitizer_failed: false,
            }),
            1
        );
    }

    #[test]
    fn sensitive_environment_values_include_launch_tokens_and_named_credentials() {
        let values = sensitive_environment_values(&[
            EnvironmentEntry {
                key: b"SURF_ACE_COMPOSITOR_LAUNCH_TOKEN".to_vec(),
                value: b"synthetic-launch-token".to_vec(),
            },
            EnvironmentEntry {
                key: b"SERVICE_PASSWORD".to_vec(),
                value: b"synthetic-password".to_vec(),
            },
            EnvironmentEntry {
                key: b"WAYLAND_DISPLAY".to_vec(),
                value: b"wayland-0".to_vec(),
            },
        ]);
        assert_eq!(values, vec!["synthetic-launch-token", "synthetic-password"]);
    }
}
