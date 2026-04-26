use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn wait_for_socket(socket_path: &Path) {
    for _ in 0..100 {
        if socket_path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "control socket was not created at {}",
        socket_path.display()
    );
}

fn send_control(socket_path: &Path, request_json: &str) -> Value {
    for _ in 0..50 {
        match UnixStream::connect(socket_path) {
            Ok(mut stream) => {
                stream
                    .write_all(request_json.as_bytes())
                    .expect("request write should succeed");
                stream
                    .write_all(b"\n")
                    .expect("request newline write should succeed");
                let mut response_line = String::new();
                let mut reader = BufReader::new(stream);
                reader
                    .read_line(&mut response_line)
                    .expect("response read should succeed");
                return serde_json::from_str::<Value>(&response_line)
                    .expect("response should be valid JSON");
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!(
        "failed to connect control socket at {}",
        socket_path.display()
    );
}

fn wait_for_status_where<F>(socket_path: &Path, description: &str, predicate: F) -> Value
where
    F: Fn(&Value) -> bool,
{
    for _ in 0..100 {
        let status = send_control(socket_path, "{\"type\":\"get_status\"}");
        if status["ok"] == Value::Bool(true) && predicate(&status) {
            return status;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for {description} at {}",
        socket_path.display()
    );
}

fn spawn_server_with_args(
    socket_path: &Path,
    runtime: &str,
    extra_args: &[&str],
    extra_env: &[(&str, &str)],
) -> Child {
    let bin = env!("CARGO_BIN_EXE_surf-ace-compositor");
    let mut command = Command::new(bin);
    command.args([
        "serve",
        "--runtime",
        runtime,
        "--socket-path",
        socket_path
            .to_str()
            .expect("socket path should be representable"),
    ]);
    command.args(extra_args);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("server should start")
}

fn spawn_server(socket_path: &Path, runtime: &str, extra_env: &[(&str, &str)]) -> Child {
    spawn_server_with_args(socket_path, runtime, &[], extra_env)
}

fn spawn_forced_host_failure_server(socket_path: &Path) -> Child {
    spawn_server(
        socket_path,
        "host",
        &[("SURF_ACE_HOST_RUNTIME_FORCE_FAIL", "1")],
    )
}

#[test]
fn host_start_with_launch_intent_and_overlay_debug_keeps_control_socket_reachable() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-host-launch-debug-startup-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let launch_intent = r#"{"process":{"command":"/bin/true","args":[],"env":{}},"binding":{"kind":"app_id","app_id":"surf-ace-main-app"}}"#;
    let mut child = spawn_server_with_args(
        &socket_path,
        "host",
        &[
            "--main-app-launch-intent-json",
            launch_intent,
            "--overlay-region-debug-borders",
        ],
        &[
            (
                "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_FAILURE_PHASE",
                "running",
            ),
            ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "500"),
        ],
    );
    wait_for_socket(&socket_path);

    let status = wait_for_status_where(&socket_path, "scripted host runtime status", |status| {
        status["status"]["runtime"]["backend"] == Value::String("host_drm".to_string())
            && status["status"]["runtime"]["wayland_socket"]
                == Value::String("wayland-test-running".to_string())
    });
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["overlay_region_debug_borders"],
        Value::Bool(true)
    );
    assert_eq!(
        status["status"]["runtime"]["main_app_launch_intent"]["binding"]["app_id"],
        Value::String("surf-ace-main-app".to_string())
    );
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "host serve must remain alive while the control socket is reachable"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn host_failure_keeps_control_socket_reachable_and_reports_failed_phase() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-host-failure-survivability-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut child = spawn_forced_host_failure_server(&socket_path);
    wait_for_socket(&socket_path);

    let status = send_control(&socket_path, "{\"type\":\"get_status\"}");
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["host_start_attempt_count"],
        Value::Number(1u64.into())
    );
    assert_eq!(
        status["status"]["runtime"]["host_last_start_trigger"],
        Value::String("bootstrap".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present")
            .contains("forced host runtime failure"),
        true
    );
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "server must remain alive so control path is usable for recovery"
    );

    let start_response = send_control(&socket_path, "{\"type\":\"start_host_runtime\"}");
    assert_eq!(start_response["ok"], Value::Bool(true));
    assert_eq!(
        start_response["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_start_attempt_count"],
        Value::Number(2u64.into())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_start_request_pending"],
        Value::Bool(true)
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_last_start_trigger"],
        Value::String("control_retry".to_string())
    );
    assert_eq!(
        start_response["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should remain visible while retry is only queued")
            .contains("forced host runtime failure"),
        true
    );

    let mut failed_again = false;
    for _ in 0..50 {
        let follow_up = send_control(&socket_path, "{\"type\":\"get_status\"}");
        if follow_up["status"]["runtime"]["phase"] == Value::String("failed".to_string())
            && follow_up["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
        {
            assert_eq!(
                follow_up["status"]["runtime"]["runtime_operator_action_needed"],
                Value::Bool(true)
            );
            failed_again = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        failed_again,
        "forced restart should fail and keep reporting failed phase"
    );
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "server must still remain alive after restart attempt"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn auto_host_failure_stays_failed_and_does_not_downgrade_to_winit() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-auto-host-failure-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut child = spawn_server(
        &socket_path,
        "auto",
        &[
            ("SURF_ACE_HOST_RUNTIME_FORCE_FAIL", "1"),
            ("SURF_ACE_COMPOSITOR_TEST_HOST_RUNTIME_CAPABLE", "1"),
        ],
    );
    wait_for_socket(&socket_path);

    let status = send_control(&socket_path, "{\"type\":\"get_status\"}");
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["backend"],
        Value::String("host_drm".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["runtime_selection_mode"],
        Value::String("automatic".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["runtime_operator_action_needed"],
        Value::Bool(true)
    );
    assert_eq!(
        status["status"]["runtime"]["runtime_operator_action_reason"]
            .as_str()
            .expect("operator action reason should be present")
            .contains("automatic host runtime failed"),
        true
    );
    assert_eq!(status["status"]["runtime"]["wayland_socket"], Value::Null);
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "server must remain alive so auto-selected host recovery stays available"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn live_socket_owner_blocks_second_host_start_on_same_socket() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-live-socket-owner-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut owner = spawn_server(&socket_path, "none", &[]);
    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "host",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be representable"),
        ])
        .env("SURF_ACE_HOST_RUNTIME_FORCE_FAIL", "1")
        .output()
        .expect("second host server should run");

    assert!(
        !output.status.success(),
        "second server unexpectedly succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("control socket already active"),
        "second server stderr should report live socket ownership: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = send_control(&socket_path, "{\"type\":\"get_status\"}");
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["phase"],
        Value::String("inactive".to_string())
    );
    assert!(
        owner
            .try_wait()
            .expect("owner wait should succeed")
            .is_none(),
        "existing server must remain alive after rejecting second start"
    );

    let _ = owner.kill();
    let _ = owner.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn queued_retry_transitions_into_starting_without_clearing_last_error() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-retry-starting-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut child = spawn_server(
        &socket_path,
        "host",
        &[
            (
                "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_FAILURE_PHASE",
                "starting",
            ),
            ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "250"),
        ],
    );
    wait_for_socket(&socket_path);

    let initial_failed =
        wait_for_status_where(&socket_path, "initial failed host state", |status| {
            status["status"]["runtime"]["phase"] == Value::String("failed".to_string())
                && status["status"]["runtime"]["host_start_attempt_count"]
                    == Value::Number(1u64.into())
        });
    assert_eq!(
        initial_failed["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present")
            .contains("scripted host runtime failure after starting"),
        true
    );

    let start_response = send_control(&socket_path, "{\"type\":\"start_host_runtime\"}");
    assert_eq!(start_response["ok"], Value::Bool(true));
    assert_eq!(
        start_response["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_start_request_pending"],
        Value::Bool(true)
    );

    let starting = wait_for_status_where(&socket_path, "retry entering starting", |status| {
        status["status"]["runtime"]["phase"] == Value::String("starting".to_string())
            && status["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
    });
    assert_eq!(
        starting["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should remain visible while restart is starting")
            .contains("scripted host runtime failure after starting"),
        true
    );
    assert_eq!(
        starting["status"]["runtime"]["runtime_operator_action_needed"],
        Value::Bool(false)
    );
    assert_eq!(
        starting["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );

    let failed_again = wait_for_status_where(
        &socket_path,
        "retry failed again after starting",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("failed".to_string())
                && status["status"]["runtime"]["host_start_attempt_count"]
                    == Value::Number(2u64.into())
                && status["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
        },
    );
    assert_eq!(
        failed_again["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present after restart failure")
            .contains("scripted host runtime failure after starting"),
        true
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn queued_retry_transitions_into_preflight_ready_without_clearing_last_error() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-retry-preflight-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut child = spawn_server(
        &socket_path,
        "host",
        &[
            (
                "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_FAILURE_PHASE",
                "preflight_ready",
            ),
            ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "250"),
        ],
    );
    wait_for_socket(&socket_path);

    let initial_failed = wait_for_status_where(
        &socket_path,
        "initial failed preflight host state",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("failed".to_string())
                && status["status"]["runtime"]["host_start_attempt_count"]
                    == Value::Number(1u64.into())
        },
    );
    assert_eq!(
        initial_failed["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present")
            .contains("scripted host runtime failure after preflight_ready"),
        true
    );

    let start_response = send_control(&socket_path, "{\"type\":\"start_host_runtime\"}");
    assert_eq!(start_response["ok"], Value::Bool(true));
    assert_eq!(
        start_response["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_start_request_pending"],
        Value::Bool(true)
    );

    let preflight_ready =
        wait_for_status_where(&socket_path, "retry entering preflight_ready", |status| {
            status["status"]["runtime"]["phase"] == Value::String("preflight_ready".to_string())
                && status["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
        });
    assert_eq!(
        preflight_ready["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should remain visible while restart is preflight-ready")
            .contains("scripted host runtime failure after preflight_ready"),
        true
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["wayland_socket"],
        Value::String("wayland-test-preflight".to_string())
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["runtime_operator_action_needed"],
        Value::Bool(false)
    );

    let failed_again = wait_for_status_where(
        &socket_path,
        "retry failed again after preflight_ready",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("failed".to_string())
                && status["status"]["runtime"]["host_start_attempt_count"]
                    == Value::Number(2u64.into())
                && status["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
        },
    );
    assert_eq!(
        failed_again["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present after restart failure")
            .contains("scripted host runtime failure after preflight_ready"),
        true
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn successful_restart_clears_last_error_only_when_running_and_owned() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-retry-running-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);

    let mut child = spawn_server(
        &socket_path,
        "host",
        &[
            (
                "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_PHASES",
                "starting,running",
            ),
            ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "200"),
        ],
    );
    wait_for_socket(&socket_path);

    let initial_failed = wait_for_status_where(
        &socket_path,
        "initial failed host state before successful retry",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("failed".to_string())
                && status["status"]["runtime"]["host_start_attempt_count"]
                    == Value::Number(1u64.into())
        },
    );
    assert_eq!(
        initial_failed["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should be present after initial failure")
            .contains("scripted host runtime failure after starting"),
        true
    );

    let start_response = send_control(&socket_path, "{\"type\":\"start_host_runtime\"}");
    assert_eq!(start_response["ok"], Value::Bool(true));
    assert_eq!(
        start_response["status"]["runtime"]["phase"],
        Value::String("failed".to_string())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_start_request_pending"],
        Value::Bool(true)
    );
    assert_eq!(
        start_response["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should remain visible while retry is queued")
            .contains("scripted host runtime failure after starting"),
        true
    );

    let preflight_ready = wait_for_status_where(
        &socket_path,
        "successful retry entering preflight_ready",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("preflight_ready".to_string())
                && status["status"]["runtime"]["host_start_request_pending"] == Value::Bool(false)
        },
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["last_error"]
            .as_str()
            .expect("last_error should remain visible while restart is preflight-ready")
            .contains("scripted host runtime failure after starting"),
        true
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["wayland_socket"],
        Value::String("wayland-test-running".to_string())
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["runtime_operator_action_needed"],
        Value::Bool(false)
    );

    let running = wait_for_status_where(
        &socket_path,
        "successful retry entering running",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("running".to_string())
                && status["status"]["runtime"]["host_output_ownership"] == Value::Bool(true)
        },
    );
    assert_eq!(
        running["status"]["runtime"]["last_error"],
        Value::Null,
        "last_error must clear only once running host ownership is live"
    );
    assert_eq!(
        running["status"]["runtime"]["host_start_attempt_count"],
        Value::Number(2u64.into())
    );
    assert_eq!(
        running["status"]["runtime"]["wayland_socket"],
        Value::String("wayland-test-running".to_string())
    );
    assert_eq!(
        running["status"]["runtime"]["host_primary_drm_path"],
        Value::String("/dev/dri/card-test".to_string())
    );
    assert_eq!(
        running["status"]["runtime"]["host_present_ownership"],
        Value::String("dumb".to_string())
    );
    assert_eq!(
        running["status"]["runtime"]["host_active_connector_name"],
        Value::String("TEST-1".to_string())
    );
    assert_eq!(
        running["status"]["runtime"]["host_last_selection_result"],
        Value::String("scripted host runtime claimed output ownership".to_string())
    );
    assert_eq!(
        running["status"]["runtime"]["runtime_operator_action_needed"],
        Value::Bool(false)
    );
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "server must remain alive after successful restart on the same control surface"
    );

    let follow_up = send_control(&socket_path, "{\"type\":\"get_status\"}");
    assert_eq!(follow_up["ok"], Value::Bool(true));
    assert_eq!(
        follow_up["status"]["runtime"]["phase"],
        Value::String("running".to_string())
    );
    assert_eq!(
        follow_up["status"]["runtime"]["host_output_ownership"],
        Value::Bool(true)
    );
    assert_eq!(follow_up["status"]["runtime"]["last_error"], Value::Null);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}
