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

fn spawn_forced_host_failure_server(socket_path: &Path) -> Child {
    let bin = env!("CARGO_BIN_EXE_surf-ace-compositor");
    Command::new(bin)
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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("host server should start")
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
        start_response["status"]["runtime"]["host_start_attempt_count"],
        Value::Number(2u64.into())
    );
    assert_eq!(
        start_response["status"]["runtime"]["host_last_start_trigger"],
        Value::String("control_retry".to_string())
    );

    let mut failed_again = false;
    for _ in 0..50 {
        let follow_up = send_control(&socket_path, "{\"type\":\"get_status\"}");
        if follow_up["status"]["runtime"]["phase"] == Value::String("failed".to_string()) {
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
