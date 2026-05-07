use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use surf_ace_compositor::control::{
    ControlRequest, RuntimeControlCommand, bind_control_listener, send_request,
    serve_listener_with_runtime_control,
};
use surf_ace_compositor::model::{
    HostRuntimeStartTrigger, MainAppLaunchIntent, OutputRotation, ProcessSpec,
};
use surf_ace_compositor::output_rotation_memory::{
    OUTPUT_ROTATION_STATE_PATH_ENV, OutputRotationMemory,
};
use surf_ace_compositor::process_manager::LocalProcessController;
use surf_ace_compositor::runtime::{
    HostRuntimeOptions, RuntimeSelectionReport, parse_shell_overlay_toggle_shortcut, run_host,
    run_winit,
};
use surf_ace_compositor::screen_capture::ScreenCaptureStore;
use surf_ace_compositor::state::CompositorState;

const RUNTIME_ENV: &str = "SURF_ACE_COMPOSITOR_RUNTIME";
const HOST_DRM_DEVICE_ENV: &str = "SURF_ACE_COMPOSITOR_HOST_DRM_DEVICE";
const HOST_OUTPUT_ENV: &str = "SURF_ACE_COMPOSITOR_HOST_OUTPUT";
const CONTROL_SOCKET_ENV: &str = "SURF_ACE_COMPOSITOR_SOCKET";
const OUTPUT_ROTATION_ENV: &str = "SURF_ACE_COMPOSITOR_OUTPUT_ROTATION";
const TEST_HOST_RUNTIME_CAPABLE_ENV: &str = "SURF_ACE_COMPOSITOR_TEST_HOST_RUNTIME_CAPABLE";
const SHELL_OVERLAY_TOGGLE_SHORTCUT_ENV: &str = "SURF_ACE_COMPOSITOR_SHELL_OVERLAY_TOGGLE_SHORTCUT";
const SHELL_OVERLAY_APP_ID: &str = "surf-ace-shell-overlay";
const MAIN_APP_LAUNCH_SHORTHAND_APP_ID: &str = "surf-ace-main-app";

#[derive(Debug, Parser)]
#[command(name = "surf-ace-compositor")]
#[command(about = "Surf Ace compositor control/runtime daemon (first implementation slice).")]
#[command(
    after_help = "Verified tty4 workflow on RACTER:\n  surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock\n  surf-ace-compositor rotate --socket-path /tmp/surf-ace-zsh-tty4.sock --rotation deg90\n  surf-ace-compositor capture --socket-path /tmp/surf-ace-zsh-tty4.sock --output-path /tmp/surf-ace-capture.png"
)]
struct Cli {
    #[arg(
        long,
        help = "Convenience main-app launch command. Compiles to the existing main-app launch-intent contract."
    )]
    launch: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Run the compositor daemon and optional runtime backend.",
        after_help = "Verified host example:\n  surf-ace-compositor serve --runtime host --socket-path /tmp/surf-ace-zsh-tty4.sock"
    )]
    Serve {
        #[arg(long, env = CONTROL_SOCKET_ENV, default_value = "/tmp/surf-ace-compositor.sock")]
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
        #[arg(
            long,
            env = OUTPUT_ROTATION_ENV,
            value_parser = ["deg0", "deg90", "deg180", "deg270"]
        )]
        output_rotation: Option<String>,
        #[arg(long, env = OUTPUT_ROTATION_STATE_PATH_ENV)]
        output_rotation_state_path: Option<PathBuf>,
        #[arg(long)]
        main_app_launch_intent_json: Option<String>,
        #[arg(
            long,
            help = "Convenience main-app launch command. Mutually exclusive with --main-app-launch-intent-json."
        )]
        launch: Option<String>,
        #[arg(
            long,
            env = SHELL_OVERLAY_TOGGLE_SHORTCUT_ENV,
            default_value = "Super+`"
        )]
        shell_overlay_toggle_shortcut: String,
        #[arg(long, env = "SURF_ACE_OVERLAY_REGION_DEBUG_BORDERS")]
        overlay_region_debug_borders: bool,
    },
    #[command(
        about = "Send a raw JSON control request over the local Unix socket.",
        after_help = "Verified control examples:\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"get_status\"}'\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"set_output_rotation\",\"rotation\":\"deg270\"}'\n  surf-ace-compositor ctl --socket-path /tmp/surf-ace-zsh-tty4.sock --request-json '{\"type\":\"capture_screen\",\"output_path\":\"/tmp/surf-ace-capture.png\"}'"
    )]
    Ctl {
        #[arg(long, env = CONTROL_SOCKET_ENV, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(
            long,
            help = "Raw control request JSON. Example: '{\"type\":\"get_status\"}'"
        )]
        request_json: Option<String>,
        #[arg(
            long,
            help = "Convenience main-app launch/relaunch command for an already-running compositor."
        )]
        launch: Option<String>,
    },
    #[command(
        about = "Set compositor output rotation over the local Unix socket.",
        after_help = "Verified rotate example:\n  surf-ace-compositor rotate --socket-path /tmp/surf-ace-zsh-tty4.sock --rotation deg90"
    )]
    Rotate {
        #[arg(long, env = CONTROL_SOCKET_ENV, default_value = "/tmp/surf-ace-compositor.sock")]
        socket_path: PathBuf,
        #[arg(long, value_parser = ["deg0", "deg90", "deg180", "deg270"])]
        rotation: String,
    },
    #[command(
        about = "Capture the current compositor output to a named path over the local Unix socket.",
        after_help = "Verified capture example:\n  surf-ace-compositor capture --socket-path /tmp/surf-ace-zsh-tty4.sock --output-path /tmp/surf-ace-capture.png"
    )]
    Capture {
        #[arg(long, env = CONTROL_SOCKET_ENV, default_value = "/tmp/surf-ace-compositor.sock")]
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
    let Cli { launch, command } = Cli::parse();
    match command {
        None => {
            let launch = launch.as_deref().unwrap_or_else(|| {
                eprintln!("missing subcommand; use 'serve' or pass --launch <command>");
                std::process::exit(2);
            });
            let socket_path = default_control_socket_path();
            if control_socket_is_active(&socket_path) {
                let request = match build_launch_control_request(launch) {
                    Ok(request) => request,
                    Err(err) => {
                        eprintln!("invalid --launch command: {err}");
                        std::process::exit(2);
                    }
                };
                match send_request(&socket_path, &request) {
                    Ok(response) => {
                        print_control_response(response);
                        return;
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                        ) =>
                    {
                        let _ = fs::remove_file(&socket_path);
                    }
                    Err(err) => {
                        eprintln!("control request failed: {err}");
                        std::process::exit(4);
                    }
                }
            }
            run_server(
                socket_path,
                "auto",
                HostRuntimeOptions {
                    forced_drm_path: None,
                    forced_output_name: None,
                },
                None,
                Some(launch),
                "Super+`",
                false,
                None,
                OutputRotationMemory::default_path(),
            )
        }
        Some(Command::Serve {
            socket_path,
            runtime,
            host_drm_device,
            host_output,
            output_rotation,
            output_rotation_state_path,
            main_app_launch_intent_json,
            launch: serve_launch,
            shell_overlay_toggle_shortcut,
            overlay_region_debug_borders,
        }) => run_server(
            socket_path,
            &runtime,
            HostRuntimeOptions {
                forced_drm_path: host_drm_device,
                forced_output_name: host_output,
            },
            main_app_launch_intent_json.as_deref(),
            serve_launch.as_deref().or(launch.as_deref()),
            &shell_overlay_toggle_shortcut,
            overlay_region_debug_borders,
            output_rotation.as_deref(),
            output_rotation_state_path,
        ),
        Some(Command::Ctl {
            socket_path,
            request_json,
            launch: ctl_launch,
        }) => {
            reject_launch_without_serve(launch.as_deref());
            match (request_json.as_deref(), ctl_launch.as_deref()) {
                (Some(request_json), None) => run_ctl(socket_path, request_json),
                (None, Some(launch)) => run_launch_control(socket_path, launch),
                (Some(_), Some(_)) => {
                    eprintln!("--request-json and --launch are mutually exclusive");
                    std::process::exit(2);
                }
                (None, None) => {
                    eprintln!("ctl requires --request-json or --launch");
                    std::process::exit(2);
                }
            }
        }
        Some(Command::Rotate {
            socket_path,
            rotation,
        }) => {
            reject_launch_without_serve(launch.as_deref());
            run_rotate(socket_path, &rotation)
        }
        Some(Command::Capture {
            socket_path,
            output_path,
        }) => {
            reject_launch_without_serve(launch.as_deref());
            run_capture(socket_path, &output_path)
        }
    }
}

fn reject_launch_without_serve(launch: Option<&str>) {
    if launch.is_some() {
        eprintln!("--launch is only valid at startup or with the serve command");
        std::process::exit(2);
    }
}

fn run_server(
    socket_path: PathBuf,
    runtime: &str,
    host_options: HostRuntimeOptions,
    main_app_launch_intent_json: Option<&str>,
    launch: Option<&str>,
    shell_overlay_toggle_shortcut: &str,
    overlay_region_debug_borders: bool,
    output_rotation: Option<&str>,
    output_rotation_state_path: Option<PathBuf>,
) {
    let launch_plan = resolve_runtime_launch_plan(runtime, detect_host_runtime_capable());
    let output_rotation_memory = output_rotation_state_path
        .or_else(OutputRotationMemory::default_path)
        .map(OutputRotationMemory::new);
    let startup_output_rotation =
        resolve_startup_output_rotation(output_rotation, output_rotation_memory.as_ref());
    let shell_overlay_toggle_shortcut =
        match parse_shell_overlay_toggle_shortcut(shell_overlay_toggle_shortcut) {
            Ok(shortcut) => shortcut,
            Err(err) => {
                eprintln!("invalid shell overlay toggle shortcut: {err}");
                std::process::exit(2);
            }
        };
    let main_app_launch_intent =
        match resolve_main_app_launch_intent(main_app_launch_intent_json, launch) {
            Ok(intent) => intent,
            Err(err) => {
                eprintln!("invalid main app launch intent: {err}");
                std::process::exit(2);
            }
        };
    let mut state = CompositorState::new_with_output_rotation(
        launch_plan.host_mode_active,
        Box::new(LocalProcessController::default()),
        startup_output_rotation,
    );
    if let Some(memory) = output_rotation_memory {
        state.remember_output_rotation_with(memory);
    }
    let shared_state = Arc::new(Mutex::new(state));
    {
        let mut state = match shared_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.set_shell_overlay_toggle_shortcut(shell_overlay_toggle_shortcut.display_string());
        state.set_overlay_region_debug_borders(overlay_region_debug_borders);
        state.configure_shell_overlay_process(resolve_default_shell_overlay_process());
        if let Some(intent) = main_app_launch_intent {
            if let Err(err) = state.select_main_app_launch_intent(intent) {
                eprintln!("invalid main app launch intent: {err}");
                std::process::exit(2);
            }
        }
    }
    apply_runtime_selection_status(&shared_state, &launch_plan);
    let screen_capture = ScreenCaptureStore::default();

    match launch_plan.selected_runtime.as_str() {
        "none" => {
            let listener = match bind_control_listener(&socket_path) {
                Ok(listener) => listener,
                Err(err) => {
                    eprintln!("control server failed: {err}");
                    std::process::exit(1);
                }
            };
            persist_startup_output_rotation_if_explicit(&shared_state, output_rotation);
            if let Err(err) =
                serve_listener_with_runtime_control(listener, shared_state, None, screen_capture)
            {
                eprintln!("control server failed: {err}");
                std::process::exit(1);
            }
        }
        "winit" => {
            let listener = match bind_control_listener(&socket_path) {
                Ok(listener) => listener,
                Err(err) => {
                    eprintln!("control server failed: {err}");
                    std::process::exit(1);
                }
            };
            persist_startup_output_rotation_if_explicit(&shared_state, output_rotation);
            let control_state = shared_state.clone();
            let control_screen_capture = screen_capture.clone();
            thread::spawn(move || {
                if let Err(err) = serve_listener_with_runtime_control(
                    listener,
                    control_state,
                    None,
                    control_screen_capture,
                ) {
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
            let listener = match bind_control_listener(&socket_path) {
                Ok(listener) => listener,
                Err(err) => {
                    eprintln!("control server failed: {err}");
                    std::process::exit(1);
                }
            };
            persist_startup_output_rotation_if_explicit(&shared_state, output_rotation);
            let (runtime_control_tx, runtime_control_rx) = mpsc::channel::<RuntimeControlCommand>();
            let control_state = shared_state.clone();
            let control_runtime_tx = runtime_control_tx.clone();
            let control_screen_capture = screen_capture.clone();
            let control_thread = thread::spawn(move || {
                if let Err(err) = serve_listener_with_runtime_control(
                    listener,
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
                                    surf_ace_compositor::model::RuntimeSelectionMode::Automatic,
                                    true,
                                    Some(format!(
                                        "automatic host runtime failed; operator recovery required: {err}"
                                    )),
                                    Some("auto runtime selection attempted backend=host".to_string()),
                                    Some(format!("selected backend=host; start failed: {err}")),
                                );
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

fn persist_startup_output_rotation_if_explicit(
    shared_state: &Arc<Mutex<CompositorState>>,
    output_rotation: Option<&str>,
) {
    if output_rotation.is_none() {
        return;
    }
    let state = match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.persist_current_output_rotation();
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
    if let Some(value) = std::env::var_os(TEST_HOST_RUNTIME_CAPABLE_ENV) {
        return matches!(
            value.to_str().map(|value| value.trim()),
            Some("1" | "true" | "TRUE" | "yes" | "YES")
        );
    }
    cfg!(target_os = "linux")
        && Path::new("/dev/dri").exists()
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

fn parse_main_app_launch_intent(
    value: Option<&str>,
) -> Result<Option<MainAppLaunchIntent>, String> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err("launch intent JSON must not be empty".to_string());
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|err| err.to_string())
}

fn resolve_main_app_launch_intent(
    intent_json: Option<&str>,
    launch: Option<&str>,
) -> Result<Option<MainAppLaunchIntent>, String> {
    if intent_json.is_some() && launch.is_some() {
        return Err(
            "--launch is mutually exclusive with --main-app-launch-intent-json".to_string(),
        );
    }
    if let Some(launch) = launch {
        return parse_main_app_launch_shorthand(launch).map(Some);
    }
    parse_main_app_launch_intent(intent_json)
}

fn parse_main_app_launch_shorthand(value: &str) -> Result<MainAppLaunchIntent, String> {
    let parts = split_launch_command(value)?;
    let Some((command, rest)) = parts.split_first() else {
        return Err("--launch command must not be empty".to_string());
    };
    if command.trim().is_empty() {
        return Err("--launch command must not be empty".to_string());
    }

    let executable_name = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
        .to_string();
    let mut args = rest.to_vec();
    let app_id = MAIN_APP_LAUNCH_SHORTHAND_APP_ID.to_string();
    let binding_app_id = match executable_name.as_str() {
        "foot" => {
            args.splice(0..0, ["--app-id".to_string(), app_id.clone()]);
            app_id
        }
        "ghostty" => {
            args.splice(0..0, [format!("--class={app_id}")]);
            app_id
        }
        "kitty" => {
            args.splice(0..0, ["--class".to_string(), app_id.clone()]);
            app_id
        }
        "wezterm" => {
            args.splice(
                0..0,
                [
                    "start".to_string(),
                    "--class".to_string(),
                    app_id.clone(),
                    "--".to_string(),
                ],
            );
            app_id
        }
        "alacritty" => {
            args.splice(0..0, ["--class".to_string(), format!("{app_id},{app_id}")]);
            app_id
        }
        _ if !command.contains('/') => {
            if let Some(terminal_process) = resolve_terminal_wrapped_main_app_process(value) {
                return Ok(MainAppLaunchIntent {
                    process: terminal_process,
                    binding: surf_ace_compositor::model::MainAppSurfaceBinding::AppId { app_id },
                });
            }
            executable_name
        }
        _ => executable_name,
    };

    Ok(MainAppLaunchIntent {
        process: ProcessSpec {
            command: command.to_string(),
            args,
            cwd: None,
            env: BTreeMap::new(),
        },
        binding: surf_ace_compositor::model::MainAppSurfaceBinding::AppId {
            app_id: binding_app_id,
        },
    })
}

fn resolve_terminal_wrapped_main_app_process(command_line: &str) -> Option<ProcessSpec> {
    let terminal = ["foot", "ghostty", "kitty", "wezterm", "alacritty"]
        .into_iter()
        .find(|candidate| command_is_available(candidate))?;
    let app_id = MAIN_APP_LAUNCH_SHORTHAND_APP_ID.to_string();
    let mut args = match terminal {
        "foot" => vec!["--app-id".to_string(), app_id],
        "ghostty" => vec![format!("--class={app_id}"), "-e".to_string()],
        "kitty" => vec!["--class".to_string(), app_id],
        "wezterm" => vec![
            "start".to_string(),
            "--class".to_string(),
            app_id,
            "--".to_string(),
        ],
        "alacritty" => vec![
            "--class".to_string(),
            format!("{app_id},{app_id}"),
            "-e".to_string(),
        ],
        _ => return None,
    };
    args.extend([
        "sh".to_string(),
        "-lc".to_string(),
        command_line.trim().to_string(),
    ]);

    Some(ProcessSpec {
        command: terminal.to_string(),
        args,
        cwd: None,
        env: BTreeMap::new(),
    })
}

fn split_launch_command(value: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quote = None;

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => quote = Some(ch),
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }

    if let Some(q) = quote {
        return Err(format!("unterminated quote {q:?} in --launch command"));
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

fn resolve_default_shell_overlay_process() -> Option<ProcessSpec> {
    let terminal = ["foot", "ghostty", "kitty", "wezterm", "alacritty"]
        .into_iter()
        .find(|candidate| command_is_available(candidate))?;
    let (shell, shell_args) = if command_is_available("zsh") {
        ("zsh".to_string(), vec!["-i".to_string()])
    } else {
        ("sh".to_string(), vec!["-i".to_string()])
    };

    let mut args = match terminal {
        "foot" => vec!["--app-id".to_string(), SHELL_OVERLAY_APP_ID.to_string()],
        "ghostty" => vec![format!("--class={SHELL_OVERLAY_APP_ID}"), "-e".to_string()],
        "kitty" => vec!["--class".to_string(), SHELL_OVERLAY_APP_ID.to_string()],
        "wezterm" => vec![
            "start".to_string(),
            "--class".to_string(),
            SHELL_OVERLAY_APP_ID.to_string(),
            "--".to_string(),
        ],
        "alacritty" => vec![
            "--class".to_string(),
            format!("{SHELL_OVERLAY_APP_ID},{SHELL_OVERLAY_APP_ID}"),
            "-e".to_string(),
        ],
        _ => return None,
    };
    args.push(shell);
    args.extend(shell_args);

    Some(ProcessSpec {
        command: terminal.to_string(),
        args,
        cwd: None,
        env: BTreeMap::new(),
    })
}

fn command_is_available(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return executable_path_exists(path);
    }
    let Some(search_path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search_path)
        .map(|entry| entry.join(command))
        .any(|candidate| executable_path_exists(&candidate))
}

fn executable_path_exists(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
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

fn run_launch_control(socket_path: PathBuf, launch: &str) {
    let request = match build_launch_control_request(launch) {
        Ok(request) => request,
        Err(err) => {
            eprintln!("invalid --launch command: {err}");
            std::process::exit(2);
        }
    };
    run_control_request(socket_path, request);
}

fn build_launch_control_request(launch: &str) -> Result<ControlRequest, String> {
    parse_main_app_launch_shorthand(launch)
        .map(|intent| ControlRequest::SetMainAppLaunchIntent { intent })
}

fn control_socket_is_active(socket_path: &Path) -> bool {
    if !socket_path.exists() {
        return false;
    }
    let Ok(file_type) = fs::symlink_metadata(socket_path).map(|metadata| metadata.file_type())
    else {
        return false;
    };
    file_type.is_socket()
}

fn default_control_socket_path() -> PathBuf {
    std::env::var_os(CONTROL_SOCKET_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/surf-ace-compositor.sock"))
}

fn resolve_startup_output_rotation(
    explicit_rotation: Option<&str>,
    memory: Option<&OutputRotationMemory>,
) -> OutputRotation {
    if let Some(rotation) = explicit_rotation {
        return match parse_output_rotation(rotation) {
            Ok(rotation) => rotation,
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(2);
            }
        };
    }
    let Some(memory) = memory else {
        return OutputRotation::Deg0;
    };
    match memory.load() {
        Ok(Some(rotation)) => rotation,
        Ok(None) => OutputRotation::Deg0,
        Err(err) => {
            eprintln!(
                "failed to load remembered output rotation from {}: {err}",
                memory.path().display()
            );
            OutputRotation::Deg0
        }
    }
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
        Ok(response) => print_control_response(response),
        Err(err) => {
            eprintln!("control request failed: {err}");
            std::process::exit(4);
        }
    }
}

fn print_control_response(response: surf_ace_compositor::control::ControlResponse) {
    match serde_json::to_string_pretty(&response) {
        Ok(formatted) => println!("{formatted}"),
        Err(err) => {
            eprintln!("failed to format response: {err}");
            std::process::exit(3);
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
    use super::{
        Cli, Command, parse_main_app_launch_intent, parse_main_app_launch_shorthand,
        resolve_main_app_launch_intent, resolve_runtime_launch_plan,
        resolve_startup_output_rotation,
    };
    use clap::Parser;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use surf_ace_compositor::model::{
        MainAppLaunchIntent, MainAppLaunchState, MainAppSurfaceBinding, OutputRotation,
        RuntimeSelectionMode,
    };
    use surf_ace_compositor::output_rotation_memory::OutputRotationMemory;

    fn temp_rotation_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        PathBuf::from(format!(
            "/tmp/surf-ace-main-output-rotation-{}-{unique}.json",
            std::process::id()
        ))
    }

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

    #[test]
    fn serve_cli_defaults_shell_overlay_toggle_shortcut_to_super_grave() {
        let cli = Cli::try_parse_from(["surf-ace-compositor", "serve"])
            .expect("serve command should parse");
        let Some(Command::Serve {
            main_app_launch_intent_json,
            launch,
            shell_overlay_toggle_shortcut,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(cli.launch.is_none());
        assert!(main_app_launch_intent_json.is_none());
        assert!(launch.is_none());
        assert_eq!(shell_overlay_toggle_shortcut, "Super+`");
    }

    #[test]
    fn serve_cli_accepts_shell_overlay_toggle_shortcut_override() {
        let cli = Cli::try_parse_from([
            "surf-ace-compositor",
            "serve",
            "--main-app-launch-intent-json",
            "{\"process\":{\"command\":\"foot\"},\"binding\":{\"kind\":\"app_id\",\"app_id\":\"surf-ace\"}}",
            "--shell-overlay-toggle-shortcut",
            "super+f12",
        ])
        .expect("serve command should parse");
        let Some(Command::Serve {
            main_app_launch_intent_json,
            launch,
            shell_overlay_toggle_shortcut,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(cli.launch.is_none());
        assert_eq!(
            main_app_launch_intent_json.as_deref(),
            Some(
                "{\"process\":{\"command\":\"foot\"},\"binding\":{\"kind\":\"app_id\",\"app_id\":\"surf-ace\"}}"
            )
        );
        assert!(launch.is_none());
        assert_eq!(shell_overlay_toggle_shortcut, "super+f12");
    }

    #[test]
    fn serve_cli_accepts_explicit_output_rotation_and_state_path() {
        let cli = Cli::try_parse_from([
            "surf-ace-compositor",
            "serve",
            "--output-rotation",
            "deg90",
            "--output-rotation-state-path",
            "/tmp/surf-ace-rotation-state.json",
        ])
        .expect("serve command should parse");
        let Some(Command::Serve {
            output_rotation,
            output_rotation_state_path,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert_eq!(output_rotation.as_deref(), Some("deg90"));
        assert_eq!(
            output_rotation_state_path.as_deref(),
            Some(std::path::Path::new("/tmp/surf-ace-rotation-state.json"))
        );
    }

    #[test]
    fn startup_output_rotation_restores_remembered_rotation_when_unspecified() {
        let memory = OutputRotationMemory::new(temp_rotation_path());
        memory
            .store(OutputRotation::Deg90)
            .expect("rotation should store");

        assert_eq!(
            resolve_startup_output_rotation(None, Some(&memory)),
            OutputRotation::Deg90
        );
        let _ = std::fs::remove_file(memory.path());
    }

    #[test]
    fn startup_output_rotation_explicit_override_wins_over_remembered_rotation() {
        let memory = OutputRotationMemory::new(temp_rotation_path());
        memory
            .store(OutputRotation::Deg90)
            .expect("rotation should store");

        assert_eq!(
            resolve_startup_output_rotation(Some("deg270"), Some(&memory)),
            OutputRotation::Deg270
        );
        let _ = std::fs::remove_file(memory.path());
    }

    #[test]
    fn launch_shorthand_at_top_level_defaults_to_serve_startup() {
        let cli = Cli::try_parse_from(["surf-ace-compositor", "--launch", "ghostty"])
            .expect("top-level launch shorthand should parse");
        assert_eq!(cli.launch.as_deref(), Some("ghostty"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn serve_cli_accepts_launch_shorthand() {
        let cli = Cli::try_parse_from(["surf-ace-compositor", "serve", "--launch", "foot -e top"])
            .expect("serve launch shorthand should parse");
        let Some(Command::Serve { launch, .. }) = cli.command else {
            panic!("expected serve command");
        };
        assert_eq!(launch.as_deref(), Some("foot -e top"));
    }

    #[test]
    fn main_app_launch_intent_parser_accepts_exact_process_and_binding_contract() {
        let parsed = parse_main_app_launch_intent(Some(
            "{\"process\":{\"command\":\"foot\",\"args\":[\"--app-id\",\"surf-ace\"]},\"binding\":{\"kind\":\"app_id\",\"app_id\":\"surf-ace\"}}",
        ))
        .expect("launch intent should parse");

        assert_eq!(
            parsed,
            Some(MainAppLaunchIntent {
                process: surf_ace_compositor::model::ProcessSpec {
                    command: "foot".to_string(),
                    args: vec!["--app-id".to_string(), "surf-ace".to_string()],
                    cwd: None,
                    env: std::collections::BTreeMap::new(),
                },
                binding: MainAppSurfaceBinding::AppId {
                    app_id: "surf-ace".to_string(),
                },
            })
        );
    }

    #[test]
    fn launch_shorthand_compiles_to_main_app_intent_contract_for_known_terminal() {
        let parsed = parse_main_app_launch_shorthand("/usr/bin/ghostty -e top")
            .expect("launch shorthand should parse");

        assert_eq!(
            parsed,
            MainAppLaunchIntent {
                process: surf_ace_compositor::model::ProcessSpec {
                    command: "/usr/bin/ghostty".to_string(),
                    args: vec![
                        "--class=surf-ace-main-app".to_string(),
                        "-e".to_string(),
                        "top".to_string()
                    ],
                    cwd: None,
                    env: std::collections::BTreeMap::new(),
                },
                binding: MainAppSurfaceBinding::AppId {
                    app_id: "surf-ace-main-app".to_string(),
                },
            }
        );
    }

    #[test]
    fn ghostty_launch_shorthand_keeps_generic_binding_contract() {
        let parsed =
            parse_main_app_launch_shorthand("ghostty").expect("ghostty shorthand should parse");

        assert_eq!(
            parsed.binding,
            MainAppSurfaceBinding::AppId {
                app_id: "surf-ace-main-app".to_string()
            }
        );
        assert_eq!(parsed.process.command, "ghostty");
        assert_eq!(
            parsed.process.args,
            vec!["--class=surf-ace-main-app".to_string()]
        );
    }

    #[test]
    fn launch_shorthand_is_mutually_exclusive_with_json_intent() {
        let err = resolve_main_app_launch_intent(
            Some("{\"process\":{\"command\":\"foot\"},\"binding\":{\"kind\":\"app_id\",\"app_id\":\"surf-ace\"}}"),
            Some("foot"),
        )
        .expect_err("json and shorthand should be rejected together");

        assert_eq!(
            err,
            "--launch is mutually exclusive with --main-app-launch-intent-json"
        );
    }

    #[test]
    fn main_app_launch_intent_parser_rejects_empty_string() {
        assert_eq!(
            parse_main_app_launch_intent(Some("   ")).unwrap_err(),
            "launch intent JSON must not be empty"
        );
    }

    #[test]
    fn runtime_status_default_uses_not_requested_main_app_state() {
        assert_eq!(
            surf_ace_compositor::model::RuntimeStatus::default().main_app_launch_state,
            MainAppLaunchState::NotRequested
        );
    }
}
