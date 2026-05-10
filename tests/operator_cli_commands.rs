use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unique_temp_path(stem: &str, suffix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be usable")
        .as_nanos();
    PathBuf::from(format!(
        "/tmp/{}-{}-{}{}",
        stem,
        std::process::id(),
        unique,
        suffix
    ))
}

fn wait_for_socket(socket_path: &Path) {
    for _ in 0..100 {
        if socket_path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("socket not created at {}", socket_path.display());
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn send_control_request(socket_path: &Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(socket_path).expect("control socket should connect");
    stream
        .write_all(
            serde_json::to_string(&request)
                .expect("request should serialize")
                .as_bytes(),
        )
        .expect("request should write");
    stream.write_all(b"\n").expect("newline should write");
    let mut response_line = String::new();
    BufReader::new(stream)
        .read_line(&mut response_line)
        .expect("response should read");
    serde_json::from_str(&response_line).expect("response should parse")
}

fn serve_single_request(
    socket_path: &Path,
    handler: impl FnOnce(Value) -> Value + Send + 'static,
) -> thread::JoinHandle<()> {
    let socket_path = socket_path.to_path_buf();
    thread::spawn(move || {
        let _ = fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).expect("socket bind should succeed");
        let (mut stream, _) = listener.accept().expect("socket accept should succeed");
        let mut request_line = String::new();
        let mut reader = BufReader::new(stream.try_clone().expect("socket clone should succeed"));
        reader
            .read_line(&mut request_line)
            .expect("request should be readable");
        let request =
            serde_json::from_str::<Value>(&request_line).expect("request JSON should parse");
        let response = handler(request);
        let serialized = serde_json::to_string(&response).expect("response JSON should serialize");
        stream
            .write_all(serialized.as_bytes())
            .expect("response write should succeed");
        stream
            .write_all(b"\n")
            .expect("response newline should succeed");
        let _ = fs::remove_file(&socket_path);
    })
}

#[test]
fn rotate_command_sends_set_output_rotation_request() {
    let socket_path = unique_temp_path("surf-ace-rotate-command", ".sock");
    let server = serve_single_request(&socket_path, |request| {
        assert_eq!(
            request,
            json!({
                "type": "set_output_rotation",
                "rotation": "deg90",
            })
        );
        json!({ "ok": true })
    });

    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "rotate",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--rotation",
            "deg90",
        ])
        .output()
        .expect("rotate command should run");

    assert!(
        output.status.success(),
        "rotate command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be JSON");
    assert_eq!(stdout, json!({ "ok": true }));

    server.join().expect("server should finish");
}

#[test]
fn capture_command_sends_capture_request_and_reports_path() {
    let socket_path = unique_temp_path("surf-ace-capture-command", ".sock");
    let capture_path = unique_temp_path("surf-ace-cli-capture", ".png");
    let expected_capture_path = capture_path
        .to_str()
        .expect("capture path should be valid UTF-8")
        .to_string();
    let capture_path_for_server = capture_path.clone();

    let server = serve_single_request(&socket_path, {
        let expected_capture_path = expected_capture_path.clone();
        move |request| {
            assert_eq!(
                request,
                json!({
                    "type": "capture_screen",
                    "output_path": expected_capture_path,
                })
            );
            fs::write(&capture_path_for_server, b"png-proof")
                .expect("capture proof file should be written");
            json!({
                "ok": true,
                "capture_path": capture_path_for_server,
            })
        }
    });

    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "capture",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--output-path",
            &expected_capture_path,
        ])
        .output()
        .expect("capture command should run");

    assert!(
        output.status.success(),
        "capture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be JSON");
    assert_eq!(
        stdout,
        json!({
            "ok": true,
            "capture_path": expected_capture_path,
        })
    );
    assert!(capture_path.exists(), "capture proof file should exist");

    let _ = fs::remove_file(&capture_path);
    server.join().expect("server should finish");
}

#[test]
fn serve_restores_remembered_output_rotation_when_unspecified() {
    let state_path = unique_temp_path("surf-ace-output-rotation-state", ".json");
    let first_socket = unique_temp_path("surf-ace-output-rotation-first", ".sock");
    let first = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "none",
            "--socket-path",
            first_socket
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--output-rotation",
            "deg90",
            "--output-rotation-state-path",
            state_path
                .to_str()
                .expect("state path should be valid UTF-8"),
        ])
        .spawn()
        .expect("first serve should start");
    wait_for_socket(&first_socket);
    let first_status = send_control_request(&first_socket, json!({ "type": "get_status" }));
    assert_eq!(first_status["status"]["output_rotation"], json!("deg90"));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&state_path).expect("state should exist"))
            .expect("state should parse"),
        json!("deg90")
    );
    stop_child(first);
    let _ = fs::remove_file(&first_socket);

    let second_socket = unique_temp_path("surf-ace-output-rotation-second", ".sock");
    let second = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "none",
            "--socket-path",
            second_socket
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--output-rotation-state-path",
            state_path
                .to_str()
                .expect("state path should be valid UTF-8"),
        ])
        .spawn()
        .expect("second serve should start");
    wait_for_socket(&second_socket);
    let second_status = send_control_request(&second_socket, json!({ "type": "get_status" }));
    assert_eq!(second_status["status"]["output_rotation"], json!("deg90"));
    stop_child(second);

    let _ = fs::remove_file(&second_socket);
    let _ = fs::remove_file(&state_path);
}

#[test]
fn failed_explicit_start_does_not_overwrite_remembered_output_rotation() {
    let state_path = unique_temp_path("surf-ace-output-rotation-state-failed-start", ".json");
    let socket_path = unique_temp_path("surf-ace-output-rotation-owned", ".sock");
    fs::write(&state_path, b"\"deg90\"").expect("initial state should write");

    let owner = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "none",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--output-rotation-state-path",
            state_path
                .to_str()
                .expect("state path should be valid UTF-8"),
        ])
        .spawn()
        .expect("owner serve should start");
    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "none",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--output-rotation",
            "deg270",
            "--output-rotation-state-path",
            state_path
                .to_str()
                .expect("state path should be valid UTF-8"),
        ])
        .output()
        .expect("second serve should run");

    assert!(
        !output.status.success(),
        "second serve unexpectedly succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("control socket already active"),
        "second serve stderr should report live socket ownership: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&state_path).expect("state should remain"))
            .expect("state should parse"),
        json!("deg90")
    );

    stop_child(owner);
    let _ = fs::remove_file(&socket_path);
    let _ = fs::remove_file(&state_path);
}

#[test]
fn ctl_command_passes_exact_main_app_launch_intent_request_through() {
    let socket_path = unique_temp_path("surf-ace-main-app-ctl-command", ".sock");
    let server = serve_single_request(&socket_path, |request| {
        assert_eq!(
            request,
            json!({
                "type": "set_main_app_launch_intent",
                "intent": {
                    "process": {
                        "command": "foot",
                        "args": ["--app-id", "surf-ace-visible-verifier"]
                    },
                    "binding": {
                        "kind": "app_id",
                        "app_id": "surf-ace-visible-verifier"
                    }
                }
            })
        );
        json!({
            "ok": true,
            "status": {
                "host_mode_active": true,
                "output_rotation": "deg0",
                "panes": [],
                "overlay_role_policy": {},
                "runtime": {
                    "backend": "none",
                    "phase": "inactive",
                    "runtime_selection_mode": "automatic",
                    "runtime_operator_action_needed": false,
                    "main_app_launch_intent": {
                        "process": {
                            "command": "foot",
                            "args": ["--app-id", "surf-ace-visible-verifier"]
                        },
                        "binding": {
                            "kind": "app_id",
                            "app_id": "surf-ace-visible-verifier"
                        }
                    },
                    "main_app_launch_state": {
                        "state": "waiting_for_runtime"
                    },
                    "shell_overlay_toggle_shortcut": "Super+`",
                    "redraw_count": 0,
                    "input_event_count": 0,
                    "host_detected_drm_device_count": 0,
                    "host_opened_drm_device_count": 0,
                    "host_output_ownership": false,
                    "host_start_attempt_count": 0,
                    "host_start_request_pending": false,
                    "host_device_selection_state": "automatic",
                    "host_output_selection_state": "automatic",
                    "host_present_ownership": "none",
                    "host_atomic_commit_enabled": false,
                    "host_overlay_plane_capable": false,
                    "host_last_queued_present_source": "none",
                    "host_last_queued_atomic_commit": false,
                    "host_last_queued_overlay_plane": false,
                    "dmabuf_protocol_enabled": false,
                    "denied_toplevel_count": 0
                }
            }
        })
    });

    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "ctl",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--request-json",
            "{\"type\":\"set_main_app_launch_intent\",\"intent\":{\"process\":{\"command\":\"foot\",\"args\":[\"--app-id\",\"surf-ace-visible-verifier\"]},\"binding\":{\"kind\":\"app_id\",\"app_id\":\"surf-ace-visible-verifier\"}}}",
        ])
        .output()
        .expect("ctl command should run");

    assert!(
        output.status.success(),
        "ctl command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be JSON");
    assert_eq!(
        stdout["status"]["runtime"]["main_app_launch_state"]["state"],
        Value::String("waiting_for_runtime".to_string())
    );

    server.join().expect("server should finish");
}

#[test]
fn ctl_launch_shorthand_sends_main_app_launch_intent_request() {
    let socket_path = unique_temp_path("surf-ace-ctl-launch-command", ".sock");
    let server = serve_single_request(&socket_path, |request| {
        assert_eq!(
            request,
            json!({
                "type": "set_main_app_launch_intent",
                "intent": {
                    "process": {
                        "command": "ghostty",
                        "args": ["--class=surf-ace-main-app", "-e", "top"]
                    },
                    "binding": {
                        "kind": "app_id",
                        "app_id": "surf-ace-main-app"
                    }
                }
            })
        );
        json!({ "ok": true })
    });

    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "ctl",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--launch",
            "ghostty -e top",
        ])
        .output()
        .expect("ctl launch command should run");

    assert!(
        output.status.success(),
        "ctl launch command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be JSON");
    assert_eq!(stdout, json!({ "ok": true }));

    server.join().expect("server should finish");
}

#[test]
fn serve_launch_shorthand_reuses_active_control_socket_instead_of_rebinding() {
    let socket_path = unique_temp_path("surf-ace-serve-launch-active", ".sock");
    let server = serve_single_request(&socket_path, |request| {
        assert_eq!(
            request,
            json!({
                "type": "set_main_app_launch_intent",
                "intent": {
                    "process": {
                        "command": "ghostty",
                        "args": ["--class=surf-ace-main-app", "-e", "top"]
                    },
                    "binding": {
                        "kind": "app_id",
                        "app_id": "surf-ace-main-app"
                    }
                }
            })
        );
        json!({ "ok": true })
    });

    wait_for_socket(&socket_path);

    let output = Command::new(env!("CARGO_BIN_EXE_surf-ace-compositor"))
        .args([
            "serve",
            "--runtime",
            "none",
            "--socket-path",
            socket_path
                .to_str()
                .expect("socket path should be valid UTF-8"),
            "--launch",
            "ghostty -e top",
        ])
        .output()
        .expect("serve launch command should run");

    assert!(
        output.status.success(),
        "serve launch command should dispatch to active socket instead of failing bind: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should be JSON");
    assert_eq!(stdout, json!({ "ok": true }));

    server.join().expect("server should finish");
}
