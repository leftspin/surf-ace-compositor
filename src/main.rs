use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use surf_ace_compositor::control::{
    ControlRequest, RuntimeControlCommand, send_request, serve, serve_with_runtime_control,
};
use surf_ace_compositor::model::{HostRuntimeStartTrigger, OutputRotation};
use surf_ace_compositor::process_manager::LocalProcessController;
use surf_ace_compositor::runtime::{
    HostRuntimeOptions, RuntimeSelectionReport, run_host, run_winit,
};
use surf_ace_compositor::screen_capture::ScreenCaptureStore;
use surf_ace_compositor::state::CompositorState;

const RUNTIME_ENV: &str = "SURF_ACE_COMPOSITOR_RUNTIME";
const HOST_DRM_DEVICE_ENV: &str = "SURF_ACE_COMPOSITOR_HOST_DRM_DEVICE";
const HOST_OUTPUT_ENV: &str = "SURF_ACE_COMPOSITOR_HOST_OUTPUT";

#[derive(Debug, Parser)]
#[command(name = "surf-ace-compositor")]
#[command(about = "Surf Ace compositor control/runtime daemon (first implementation slice).")]
#[command(
    after_help = "Verified tty4 workflow on RACTER:\n  surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock\n  surf-ace-compositor rotate --socket-path /tmp/surf-ace-zsh-tty4.sock --rotation deg90\n  surf-ace-compositor capture --socket-path /tmp/surf-ace-zsh-tty4.sock --output-path /tmp/surf-ace-capture.png"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Run the compositor daemon and optional runtime backend.",
        after_help = "Verified host example:\n  surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock"
    )]
    Serve {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(
            long,
            env = RUNTIME_ENV,
            default_value = "auto",
            value_parser = ["auto", "none", "winit", "host"]
        )]
        runtime: String,
        #[arg(long, env = HOST_DRM_DEVICE_ENV)]
        host_drm_device: Option<PathBuf>,
        #[arg(long, env = HOST_OUTPUT_ENV)]
        host_output: Option<String>,
    },
    #[command(
        about = "Send a raw JSON control request over the local Unix socket.",
        after_help = "Verified control examples:\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"get_status\"}'\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"set_output_rotation\",\"rotation\":\"deg270\"}'\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"capture_screen\",\"output_path\":\"/tmp/surf-ace-capture.png\"}'"
    )]
    Ctl {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(
            long,
            help = "Raw control request JSON. Example: '{\"type\":\"get_status\"}'"
        )]
        request_json: String,
    },
    #[command(
        about = "Set compositor output rotation over the local Unix socket.",
        after_help = "Verified rotate example:\n  surf-ace-compositor rotate --socket-path /tmp/surf-ace-zsh-tty4.sock --rotation deg90"
    )]
    Rotate {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(long, value_parser = ["deg0", "deg90", "deg180", "deg270"])]
        rotation: String,
    },
    #[command(
        about = "Capture the current compositor output to a named path over the local Unix socket.",
        after_help = "Verified capture example:\n  surf-ace-compositor capture --socket-path /tmp/surf-ace-zsh-tty4.sock --output-path /tmp/surf-ace-capture.png"
    )]
    Capture {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(long)]
        output_path: String,
    },
}

#[derive(Debug, Clone)]
struct RuntimeLaunchPlan {
    selected_runtime: String,
    host_mode_active: bool,
    selection_report: RuntimeSelectionReport,
    selection_attempt: String,
    selection_result: String,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve {
            socket_path,
            runtime,
            host_drm_device,
            host_output,
        } => run_server(
            socket_path,
            &runtime,
            HostRuntimeOptions {
                forced_drm_path: host_drm_device,
                forced_output_name: host_output,
            },
        ),
        Command::Ctl {
            socket_path,
            request_json,
        } => run_ctl(socket_path, &request_json),
        Command::Rotate {
            socket_path,
            rotation,
        } => run_rotate(socket_path, &rotation),
        Command::Capture {
            socket_path,
            output_path,
        } => run_capture(socket_path, &output_path),
    }
}

fn run_server(socket_path: PathBuf, runtime: &str, host_options: HostRuntimeOptions) {
    let launch_plan = resolve_runtime_launch_plan(runtime, detect_host_runtime_capable());
    let state = CompositorState::new(
        launch_plan.host_mode_active,
        Box::new(LocalProcessController::default()),
    );
    let shared_state = Arc::new(Mutex::new(state));
    apply_runtime_selection_status(&shared_state, &launch_plan);
    let screen_capture = ScreenCaptureStore::default();

    match launch_plan.selected_runtime.as_str() {
        "none" => {
            if let Err(err) = serve(&socket_path, shared_state, screen_capture) {
                eprintln!("control server failed: {err}");
                std::process::exit(1);
            }
        }
        "winit" => {
            let control_state = shared_state.clone();
            let control_socket = socket_path.clone();
            let control_screen_capture = screen_capture.clone();
            thread::spawn(move || {
                if let Err(err) = serve(&control_socket, control_state, control_screen_capture) {
                    eprintln!("control server failed: {err}");
                }
            });

            if let Err(err) = run_winit(shared_state) {
                eprintln!("winit runtime failed: {err}");
                std::process::exit(1);
            }
        }
        "host" => {
            let auto_selected_host = runtime == "auto";
            let (runtime_control_tx, runtime_control_rx) = mpsc::channel::<RuntimeControlCommand>();
            let control_state = shared_state.clone();
            let control_socket = socket_path.clone();
            let control_runtime_tx = runtime_control_tx.clone();
            let control_screen_capture = screen_capture.clone();
            let control_thread = thread::spawn(move || {
                if let Err(err) = serve_with_runtime_control(
                    &control_socket,
                    control_state,
                    Some(control_runtime_tx),
                    control_screen_capture,
                ) {
                    eprintln!("control server failed: {err}");
                }
            });

            {
                let mut state = match shared_state.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                state.mark_host_runtime_start_requested(HostRuntimeStartTrigger::Bootstrap);
            }
            if runtime_control_tx
                .send(RuntimeControlCommand::StartHostRuntime)
                .is_err()
            {
                eprintln!("failed to queue initial host runtime start");
                std::process::exit(1);
            }

            drop(runtime_control_tx);

            while let Ok(command) = runtime_control_rx.recv() {
                match command {
                    RuntimeControlCommand::StartHostRuntime => {
                        if let Err(err) = run_host(
                            shared_state.clone(),
                            screen_capture.clone(),
                            host_options.clone(),
                        ) {
                            eprintln!("host runtime failed: {err}");
                            let mut state = match shared_state.lock() {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                            state.mark_runtime_failed(format!("host runtime failed: {err}"));
                            if auto_selected_host {
                                state.set_runtime_selection_status(
                                    surf_ace_compositor::model::RuntimeSelectionMode::FallbackAfterFailure,
                                    true,
                                    Some(format!(
                                        "automatic host runtime failed, fallback to winit: {err}"
                                    )),
                                    Some("auto runtime selection attempted backend=host".to_string()),
                                    Some("fallback backend=winit".to_string()),
                                );
                                drop(state);
                                if let Err(winit_err) = run_winit(shared_state.clone()) {
                                    eprintln!("winit fallback runtime failed: {winit_err}");
                                    std::process::exit(1);
                                }
                            }
                        }
                    }
                }
            }

            if control_thread.join().is_err() {
                eprintln!("control server thread terminated unexpectedly");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("unsupported runtime mode: {}", launch_plan.selected_runtime);
            std::process::exit(2);
        }
    };
}

fn apply_runtime_selection_status(
    shared_state: &Arc<Mutex<CompositorState>>,
    launch_plan: &RuntimeLaunchPlan,
) {
    let mut state = match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.set_runtime_selection_status(
        launch_plan.selection_report.mode,
        launch_plan.selection_report.operator_action_needed,
        launch_plan.selection_report.operator_action_reason.clone(),
        Some(launch_plan.selection_attempt.clone()),
        Some(launch_plan.selection_result.clone()),
    );
}

fn resolve_runtime_launch_plan(runtime: &str, host_runtime_capable: bool) -> RuntimeLaunchPlan {
    match runtime {
        "auto" if host_runtime_capable => RuntimeLaunchPlan {
            selected_runtime: "host".to_string(),
            host_mode_active: true,
            selection_report: RuntimeSelectionReport::automatic(),
            selection_attempt: "auto runtime selection attempted backend=host".to_string(),
            selection_result: "selected backend=host".to_string(),
        },
        "auto" => RuntimeLaunchPlan {
            selected_runtime: "winit".to_string(),
            host_mode_active: false,
            selection_report: RuntimeSelectionReport::automatic(),
            selection_attempt: "auto runtime selection attempted backend=host".to_string(),
            selection_result: "selected backend=winit".to_string(),
        },
        forced => RuntimeLaunchPlan {
            selected_runtime: forced.to_string(),
            host_mode_active: forced == "host",
            selection_report: RuntimeSelectionReport::forced(),
            selection_attempt: format!("forced runtime selection requested backend={forced}"),
            selection_result: format!("selected backend={forced}"),
        },
    }
}

fn detect_host_runtime_capable() -> bool {
    cfg!(target_os = "linux")
        && Path::new("/dev/dri").exists()
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

fn run_ctl(socket_path: PathBuf, request_json: &str) {
    let request: ControlRequest = match serde_json::from_str(request_json) {
        Ok(request) => request,
        Err(err) => {
            eprintln!("invalid request JSON: {err}");
            std::process::exit(2);
        }
    };

    run_control_request(socket_path, request);
}

fn run_rotate(socket_path: PathBuf, rotation: &str) {
    let rotation = match parse_output_rotation(rotation) {
        Ok(rotation) => rotation,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };

    run_control_request(socket_path, ControlRequest::SetOutputRotation { rotation });
}

fn run_capture(socket_path: PathBuf, output_path: &str) {
    run_control_request(
        socket_path,
        ControlRequest::CaptureScreen {
            output_path: output_path.to_string(),
        },
    );
}

fn run_control_request(socket_path: PathBuf, request: ControlRequest) {
    match send_request(&socket_path, &request) {
        Ok(response) => match serde_json::to_string_pretty(&response) {
            Ok(formatted) => println!("{formatted}"),
            Err(err) => {
                eprintln!("failed to format response: {err}");
                std::process::exit(3);
            }
        },
        Err(err) => {
            eprintln!("control request failed: {err}");
            std::process::exit(4);
        }
    }
}

fn parse_output_rotation(value: &str) -> Result<OutputRotation, String> {
    match value {
        "deg0" => Ok(OutputRotation::Deg0),
        "deg90" => Ok(OutputRotation::Deg90),
        "deg180" => Ok(OutputRotation::Deg180),
        "deg270" => Ok(OutputRotation::Deg270),
        _ => Err(format!("unsupported rotation: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_runtime_launch_plan;
    use surf_ace_compositor::model::RuntimeSelectionMode;

    #[test]
    fn auto_runtime_prefers_host_when_host_capable() {
        let plan = resolve_runtime_launch_plan("auto", true);
        assert_eq!(plan.selected_runtime, "host");
        assert!(plan.host_mode_active);
        assert_eq!(plan.selection_report.mode, RuntimeSelectionMode::Automatic);
        assert_eq!(plan.selection_result, "selected backend=host");
    }

    #[test]
    fn auto_runtime_falls_back_to_winit_when_host_not_capable() {
        let plan = resolve_runtime_launch_plan("auto", false);
        assert_eq!(plan.selected_runtime, "winit");
        assert!(!plan.host_mode_active);
        assert_eq!(plan.selection_report.mode, RuntimeSelectionMode::Automatic);
        assert_eq!(
            plan.selection_attempt,
            "auto runtime selection attempted backend=host"
        );
    }

    #[test]
    fn forced_runtime_reports_forced_selection() {
        let plan = resolve_runtime_launch_plan("host", false);
        assert_eq!(plan.selected_runtime, "host");
        assert!(plan.host_mode_active);
        assert_eq!(plan.selection_report.mode, RuntimeSelectionMode::Forced);
        assert_eq!(
            plan.selection_attempt,
            "forced runtime selection requested backend=host"
        );
    }
}
