use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use surf_ace_compositor::control::{
    ControlRequest, RuntimeControlCommand, send_request, serve, serve_with_runtime_control,
};
use surf_ace_compositor::model::{HostRuntimeStartTrigger, OutputRotation};
use surf_ace_compositor::process_manager::LocalProcessController;
use surf_ace_compositor::runtime::{run_host, run_winit};
use surf_ace_compositor::screen_capture::ScreenCaptureStore;
use surf_ace_compositor::state::CompositorState;

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
        #[arg(long, default_value = "none", value_parser = ["none", "winit", "host"])]
        runtime: String,
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

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve {
            socket_path,
            runtime,
        } => run_server(socket_path, &runtime),
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

fn run_server(socket_path: PathBuf, runtime: &str) {
    let host_mode_active = matches!(runtime, "host");
    let state = CompositorState::new(
        host_mode_active,
        Box::new(LocalProcessController::default()),
    );
    let shared_state = Arc::new(Mutex::new(state));
    let screen_capture = ScreenCaptureStore::default();

    match runtime {
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
                        if let Err(err) = run_host(shared_state.clone(), screen_capture.clone()) {
                            eprintln!("host runtime failed: {err}");
                            let mut state = match shared_state.lock() {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                            state.mark_runtime_failed(format!("host runtime failed: {err}"));
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
            eprintln!("unsupported runtime mode: {runtime}");
            std::process::exit(2);
        }
    };
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
