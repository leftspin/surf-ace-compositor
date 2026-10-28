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

fn wait_for_stderr_contains(stderr_path: &Path, expected: &str) -> String {
    for _ in 0..100 {
        let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
        if stderr.contains(expected) {
            return stderr;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for stderr to contain {expected:?}");
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

fn spawn_server_with_args_and_stderr(
    socket_path: &Path,
    runtime: &str,
    extra_args: &[&str],
    extra_env: &[(&str, &str)],
    stderr_path: &Path,
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
        .stderr(Stdio::from(
            fs::File::create(stderr_path).expect("stderr capture file should be created"),
        ))
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
fn invalid_or_empty_main_app_configuration_keeps_host_compositor_running() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let cases = [
        ("empty", "", "launch intent JSON must not be empty"),
        (
            "invalid",
            r#"{"process":{"command":" ","args":[],"env":{}},"binding":{"kind":"app_id","app_id":"surf-ace-main-app"}}"#,
            "main app process command must not be empty",
        ),
    ];

    for (case, launch_intent, expected_error) in cases {
        let socket_path = PathBuf::from(format!(
            "/tmp/surf-ace-invalid-main-app-{}-{}-{}.sock",
            std::process::id(),
            unique_suffix,
            case
        ));
        let stderr_path = PathBuf::from(format!(
            "/tmp/surf-ace-invalid-main-app-{}-{}-{}.stderr",
            std::process::id(),
            unique_suffix,
            case
        ));
        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(&stderr_path);

        let mut child = spawn_server_with_args_and_stderr(
            &socket_path,
            "host",
            &["--main-app-launch-intent-json", launch_intent],
            &[
                (
                    "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_FAILURE_PHASE",
                    "running",
                ),
                ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "1000"),
            ],
            &stderr_path,
        );
        wait_for_socket(&socket_path);

        let status = wait_for_status_where(&socket_path, "running host runtime", |status| {
            status["status"]["runtime"]["phase"] == Value::String("running".to_string())
        });
        assert_eq!(
            status["status"]["runtime"]["main_app_launch_intent"],
            Value::Null
        );
        assert!(
            status["status"]["runtime"]["last_diagnostic"]
                .as_str()
                .unwrap_or_default()
                .contains(expected_error)
        );
        assert!(
            child
                .try_wait()
                .expect("child wait should succeed")
                .is_none(),
            "invalid main-app configuration must leave the compositor process running"
        );
        let stderr = fs::read_to_string(&stderr_path).expect("stderr should be captured");
        assert!(stderr.contains(expected_error));
        assert!(stderr.contains("continuing without a main app"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(&stderr_path);
    }
}

#[test]
fn host_failure_keeps_control_socket_reachable_while_retrying() {
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

    let status = wait_for_status_where(
        &socket_path,
        "host runtime retry with the failure retained",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("starting".to_string())
                && status["status"]["runtime"]["last_diagnostic"]
                    .as_str()
                    .is_some_and(|error| error.contains("forced host runtime failure"))
        },
    );
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["phase"],
        Value::String("starting".to_string())
    );
    assert!(
        status["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("retry warning should remain visible during retry")
            .contains("forced host runtime failure")
    );
    assert_eq!(status["status"]["runtime"]["wayland_socket"], Value::Null);
    assert_eq!(
        status["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );
    assert!(
        child
            .try_wait()
            .expect("child wait should succeed")
            .is_none(),
        "server must remain alive so control path is usable for recovery"
    );

    let follow_up = send_control(&socket_path, "{\"type\":\"get_status\"}");
    assert_eq!(follow_up["ok"], Value::Bool(true));
    assert!(
        follow_up["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("retry warning should remain visible during retry")
            .contains("forced host runtime failure")
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
fn auto_host_failure_keeps_retrying_without_downgrading_to_winit() {
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

    let status = wait_for_status_where(&socket_path, "automatic host selection retry", |status| {
        status["status"]["runtime"]["backend"] == Value::String("host_drm".to_string())
            && status["status"]["runtime"]["runtime_selection_mode"]
                == Value::String("automatic".to_string())
            && status["status"]["runtime"]["phase"] == Value::String("starting".to_string())
            && status["status"]["runtime"]["last_diagnostic"]
                .as_str()
                .is_some_and(|error| error.contains("forced host runtime failure"))
    });
    assert_eq!(status["ok"], Value::Bool(true));
    assert_eq!(
        status["status"]["runtime"]["backend"],
        Value::String("host_drm".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["phase"],
        Value::String("starting".to_string())
    );
    assert_eq!(
        status["status"]["runtime"]["runtime_selection_mode"],
        Value::String("automatic".to_string())
    );
    assert!(
        status["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("retry warning should report the unavailable host display")
            .contains("forced host runtime failure")
    );
    assert_eq!(status["status"]["runtime"]["wayland_socket"], Value::Null);
    assert_eq!(
        status["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );
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
fn second_server_survives_optional_control_socket_conflict() {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for test")
        .as_nanos();
    let socket_path = PathBuf::from(format!(
        "/tmp/surf-ace-live-socket-owner-{}-{}.sock",
        std::process::id(),
        unique_suffix
    ));
    let stderr_path = PathBuf::from(format!(
        "/tmp/surf-ace-live-socket-owner-{}-{}.stderr",
        std::process::id(),
        unique_suffix
    ));
    let _ = fs::remove_file(&socket_path);
    let _ = fs::remove_file(&stderr_path);

    let mut owner = spawn_server(&socket_path, "none", &[]);
    wait_for_socket(&socket_path);

    let mut second = spawn_server_with_args_and_stderr(
        &socket_path,
        "host",
        &[],
        &[("SURF_ACE_HOST_RUNTIME_FORCE_FAIL", "1")],
        &stderr_path,
    );
    let stderr = wait_for_stderr_contains(
        &stderr_path,
        "optional control service unavailable; continuing without control",
    );
    assert!(stderr.contains("control socket already active"));
    assert!(
        second
            .try_wait()
            .expect("second server wait should succeed")
            .is_none(),
        "optional control socket failure must not stop the compositor process"
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
        "existing server must remain alive after the second process loses optional control"
    );

    let _ = second.kill();
    let _ = second.wait();
    let _ = owner.kill();
    let _ = owner.wait();
    let _ = fs::remove_file(&socket_path);
    let _ = fs::remove_file(&stderr_path);
}

#[test]
fn host_startup_failure_retries_and_keeps_warning_visible() {
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

    let starting = wait_for_status_where(
        &socket_path,
        "automatic retry entering starting",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("starting".to_string())
                && status["status"]["runtime"]["last_diagnostic"]
                    .as_str()
                    .is_some_and(|error| {
                        error.contains("scripted host runtime failure after starting")
                    })
        },
    );
    assert_eq!(
        starting["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("retry warning should remain visible while startup retries")
            .contains("scripted host runtime failure after starting"),
        true
    );
    assert_eq!(
        starting["status"]["runtime"]["host_output_ownership"],
        Value::Bool(false)
    );
    assert_eq!(starting["status"]["runtime"]["wayland_socket"], Value::Null);

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn automatic_preflight_retry_keeps_warning_visible() {
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
                "SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_PHASES",
                "preflight_ready,preflight_ready",
            ),
            ("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS", "250"),
        ],
    );
    wait_for_socket(&socket_path);

    let preflight_ready = wait_for_status_where(
        &socket_path,
        "automatic retry entering preflight_ready",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("preflight_ready".to_string())
                && status["status"]["runtime"]["last_diagnostic"]
                    .as_str()
                    .is_some_and(|error| {
                        error.contains("scripted host runtime failure after preflight_ready")
                    })
        },
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("retry warning should remain visible while preflight is ready")
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

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&socket_path);
}

#[test]
fn automatic_recovery_reaches_running_after_startup_failure() {
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

    let preflight_ready = wait_for_status_where(
        &socket_path,
        "automatic retry entering preflight_ready",
        |status| {
            status["status"]["runtime"]["phase"] == Value::String("preflight_ready".to_string())
                && status["status"]["runtime"]["wayland_socket"]
                    == Value::String("wayland-test-running".to_string())
                && status["status"]["runtime"]["last_diagnostic"]
                    .as_str()
                    .is_some_and(|warning| {
                        warning.contains("scripted host runtime failure after starting")
                    })
        },
    );
    assert_eq!(
        preflight_ready["status"]["runtime"]["last_error"],
        Value::Null
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
        running["status"]["runtime"]["last_diagnostic"]
            .as_str()
            .expect("the failure warning should remain in status after recovery")
            .contains("scripted host runtime failure after starting"),
        true
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
