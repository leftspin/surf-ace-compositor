use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
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
