use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use surf_ace_compositor::control::{
    ControlRequest, RuntimeControlCommand, send_request, serve, serve_with_runtime_control,
};
use surf_ace_compositor::model::HostRuntimeStartTrigger;
use surf_ace_compositor::process_manager::LocalProcessController;
use surf_ace_compositor::runtime::{run_host, run_winit};
use surf_ace_compositor::state::CompositorState;

#[derive(Debug, Parser)]
#[command(name = "surf-ace-compositor")]
#[command(about = "Surf Ace compositor control/runtime daemon (first implementation slice).")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(long, default_value = "none", value_parser = ["none", "winit", "host"])]
        runtime: String,
    },
    Ctl {
        #[arg(long, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(
            long,
            help = "Raw control request JSON. Example: '{\"type\":\"get_status\"}'"
        )]
        request_json: String,
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
    }
}

fn run_server(socket_path: PathBuf, runtime: &str) {
    let host_mode_active = matches!(runtime, "host");
    let state = CompositorState::new(
        host_mode_active,
        Box::new(LocalProcessController::default()),
    );
    let shared_state = Arc::new(Mutex::new(state));

    match runtime {
        "none" => {
            if let Err(err) = serve(&socket_path, shared_state) {
                eprintln!("control server failed: {err}");
                std::process::exit(1);
            }
        }
        "winit" => {
            let control_state = shared_state.clone();
            let control_socket = socket_path.clone();
            thread::spawn(move || {
                if let Err(err) = serve(&control_socket, control_state) {
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
            let control_thread = thread::spawn(move || {
                if let Err(err) = serve_with_runtime_control(
                    &control_socket,
                    control_state,
                    Some(control_runtime_tx),
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
                        if let Err(err) = run_host(shared_state.clone()) {
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
