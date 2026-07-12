use crate::control::RuntimeControlCommand;
use crate::model::{
    LaunchTokenEvidence, OutputRotation, OverlayCaptureCapability, OverlayRegionStatus,
    PaneGeometry, PaneId, RuntimeBackend, RuntimeDmabufFormatStatus, RuntimeFocusTarget,
    RuntimeHostPresentOwnership, RuntimeHostQueuedPresentSource, RuntimeHostSelectionState,
    RuntimeSelectionMode, SurfaceBindingEvidence, SurfaceBindingEvidenceOutcome,
};
use crate::output_rotation_model::OutputRotationModel;
use crate::screen_capture::ScreenCaptureStore;
use crate::state::{CompositorState, LAUNCH_TOKEN_ENV};
use input::Libinput;
use rustix::fs::OFlags;
use rustix::io::dup;
use smithay::backend::allocator::gbm::{GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{
    Buffer, Format, Fourcc, Modifier,
    dmabuf::{AsDmabuf, Dmabuf},
};
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{DrmDeviceFd, DrmNode, NodeType};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, WaylandSurfaceTexture, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{
    Element, Id, Kind, RenderElement, UnderlyingStorage, render_elements,
    solid::SolidColorRenderElement, utils::CropRenderElement,
};
use smithay::backend::renderer::gles::{
    GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
};
use smithay::backend::renderer::utils::{
    CommitCounter, DamageSet, OpaqueRegions, RendererSurfaceStateUserData, draw_render_elements,
    import_surface_tree, on_commit_buffer_handler,
};
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, ImportDma, Offscreen, Renderer, Texture, TextureMapping,
};
use smithay::backend::session::Event as SessionSignal;
use smithay::backend::session::Session;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev::{UdevBackend, UdevEvent, primary_gpu};
use smithay::backend::winit::{self, WinitEvent};
use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_dmabuf;
use smithay::delegate_fractional_scale;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_viewporter;
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState, keysyms, xkb};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{
    Mode as OutputMode, Output, PhysicalProperties, Scale as OutputScale, Subpixel,
};
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, LoopSignal, Mode as CalloopMode, PostAction,
    RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};
use smithay::reexports::drm::{
    self as drm_api, ClientCapability, Device as DrmDevice,
    buffer::{Buffer as DrmBuffer, DrmFourcc},
    control::{
        AtomicCommitFlags, Device as DrmControlDevice, Mode as DrmMode, ModeTypeFlags,
        PageFlipFlags, atomic::AtomicModeReq, connector as drm_connector, crtc as drm_crtc,
        dumbbuffer as drm_dumbbuffer, encoder as drm_encoder, framebuffer as drm_framebuffer,
        plane as drm_plane, property as drm_property,
    },
};
use smithay::reexports::wayland_protocols::xdg::{
    decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode,
    shell::server::xdg_toplevel,
};
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, ObjectId,
};
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{
    Buffer as BufferCoords, DeviceFd, Logical, Physical, Point, Rectangle, SERIAL_COUNTER,
    Scale as SurfaceScale, Serial, Size, Transform,
};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState as SmithayCompositorState,
    SubsurfaceCachedState, SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier, get_dmabuf,
};
use smithay::wayland::fractional_scale::{
    self, FractionalScaleHandler, FractionalScaleManagerState,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};
use smithay::wayland::shell::xdg::{
    Configure, PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
    XdgShellState, XdgToplevelSurfaceData,
    decoration::{XdgDecorationHandler, XdgDecorationState},
};
use smithay::wayland::shm::{BufferAccessError, ShmHandler, ShmState, with_buffer_contents};
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::viewporter::ViewporterState;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Default)]
pub struct HostRuntimeOptions {
    pub forced_drm_path: Option<PathBuf>,
    pub forced_output_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSelectionReport {
    pub mode: RuntimeSelectionMode,
    pub operator_action_needed: bool,
    pub operator_action_reason: Option<String>,
}

impl RuntimeSelectionReport {
    pub fn automatic() -> Self {
        Self {
            mode: RuntimeSelectionMode::Automatic,
            operator_action_needed: false,
            operator_action_reason: None,
        }
    }

    pub fn forced() -> Self {
        Self {
            mode: RuntimeSelectionMode::Forced,
            operator_action_needed: false,
            operator_action_reason: None,
        }
    }

    pub fn fallback(reason: impl Into<String>) -> Self {
        Self {
            mode: RuntimeSelectionMode::FallbackAfterFailure,
            operator_action_needed: true,
            operator_action_reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOverlayToggleShortcut {
    normalized: String,
    keysym: Keysym,
}

impl ShellOverlayToggleShortcut {
    pub fn display_string(&self) -> String {
        self.normalized.clone()
    }

    fn matches(&self, modifiers: &ModifiersState, raw_syms: &[Keysym]) -> bool {
        modifiers.logo
            && !modifiers.ctrl
            && !modifiers.alt
            && raw_syms.iter().copied().any(|sym| sym == self.keysym)
    }
}

fn format_shell_overlay_raw_syms(raw_syms: &[Keysym]) -> String {
    if raw_syms.is_empty() {
        return "<none>".to_string();
    }

    raw_syms
        .iter()
        .map(|sym| {
            let name = xkb::keysym_get_name(*sym);
            if name.is_empty() {
                "NoSymbol".to_string()
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_shell_overlay_modifiers(modifiers: &ModifiersState) -> String {
    format!(
        "logo={} ctrl={} alt={} shift={} caps_lock={} num_lock={} iso_level3_shift={} iso_level5_shift={}",
        modifiers.logo,
        modifiers.ctrl,
        modifiers.alt,
        modifiers.shift,
        modifiers.caps_lock,
        modifiers.num_lock,
        modifiers.iso_level3_shift,
        modifiers.iso_level5_shift,
    )
}

pub fn parse_shell_overlay_toggle_shortcut(
    value: &str,
) -> Result<ShellOverlayToggleShortcut, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("shortcut must not be empty".to_string());
    }

    let parts: Vec<_> = trimmed.split('+').map(str::trim).collect();
    if parts.len() != 2 {
        return Err(format!(
            "shortcut must use the form Super+<keysym>; got '{trimmed}'"
        ));
    }
    if !matches!(
        parts[0].to_ascii_lowercase().as_str(),
        "super" | "logo" | "meta" | "win"
    ) {
        return Err(format!(
            "shortcut modifier must be Super; got '{}'",
            parts[0]
        ));
    }

    let key_name = match parts[1] {
        "`" => "grave".to_string(),
        other => other.to_string(),
    };
    let keysym = {
        let exact = xkb::keysym_from_name(&key_name, xkb::KEYSYM_NO_FLAGS);
        if exact != Keysym::NoSymbol {
            exact
        } else {
            xkb::keysym_from_name(&key_name, xkb::KEYSYM_CASE_INSENSITIVE)
        }
    };
    if keysym == Keysym::NoSymbol {
        return Err(format!("unknown shell overlay shortcut key '{}'", parts[1]));
    }

    let normalized_key = if keysym == Keysym::new(keysyms::KEY_grave) {
        "`".to_string()
    } else {
        xkb::keysym_get_name(keysym)
    };
    Ok(ShellOverlayToggleShortcut {
        normalized: format!("Super+{normalized_key}"),
        keysym,
    })
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("failed to create calloop event loop: {0}")]
    EventLoop(String),
    #[error("failed to initialize smithay winit backend: {0}")]
    WinitInit(String),
    #[error("failed to initialize libseat session: {0}")]
    HostSession(String),
    #[error("failed to initialize udev monitor: {0}")]
    HostUdev(String),
    #[error("no drm devices detected for seat: {0}")]
    HostNoDrmDevices(String),
    #[error("no drm devices could be opened for seat: {0}")]
    HostNoDrmDeviceOpened(String),
    #[error("failed to open drm device {path}: {error}")]
    HostDeviceOpen { path: String, error: String },
    #[error("failed to close drm device {path}: {error}")]
    HostDeviceClose { path: String, error: String },
    #[error("failed to inspect drm resources on {path}: {error}")]
    HostOutputInspect { path: String, error: String },
    #[error("failed to claim output ownership on {path}: {error}")]
    HostOutputClaim { path: String, error: String },
    #[error("no connected drm outputs with a valid connector/crtc/mode route were found")]
    HostNoConnectedOutputRoute,
    #[error("failed to create wayland display: {0}")]
    WaylandDisplay(String),
    #[error("failed to create wayland listening socket: {0}")]
    WaylandSocket(String),
    #[error("failed to register event source: {0}")]
    RegisterSource(String),
    #[error("runtime loop failed: {0}")]
    Loop(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostRuntimeTestScriptedFailurePhase {
    Starting,
    PreflightReady,
    Running,
}

fn host_runtime_test_scripted_failure_phase() -> Option<HostRuntimeTestScriptedFailurePhase> {
    let value = std::env::var("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_PHASE")
        .ok()
        .or_else(|| std::env::var("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_FAILURE_PHASE").ok())?;
    match value.trim() {
        "starting" => Some(HostRuntimeTestScriptedFailurePhase::Starting),
        "preflight_ready" => Some(HostRuntimeTestScriptedFailurePhase::PreflightReady),
        "running" => Some(HostRuntimeTestScriptedFailurePhase::Running),
        _ => None,
    }
}

fn host_runtime_test_scripted_phase(
    shared_state: &Arc<Mutex<CompositorState>>,
) -> Option<HostRuntimeTestScriptedFailurePhase> {
    if let Ok(phases) = std::env::var("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_PHASES") {
        let attempt = {
            let state = lock_state(shared_state);
            state.status_snapshot().runtime.host_start_attempt_count
        };
        let attempt_index = usize::try_from(attempt.saturating_sub(1)).ok()?;
        let phase = phases
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .nth(attempt_index)?;
        return match phase {
            "starting" => Some(HostRuntimeTestScriptedFailurePhase::Starting),
            "preflight_ready" => Some(HostRuntimeTestScriptedFailurePhase::PreflightReady),
            "running" => Some(HostRuntimeTestScriptedFailurePhase::Running),
            _ => None,
        };
    }

    host_runtime_test_scripted_failure_phase()
}

fn host_runtime_test_scripted_hold_duration() -> Duration {
    std::env::var("SURF_ACE_HOST_RUNTIME_TEST_SCRIPTED_HOLD_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default()
}

fn maybe_sleep_host_runtime_test_scripted_hold() {
    let hold_duration = host_runtime_test_scripted_hold_duration();
    if !hold_duration.is_zero() {
        std::thread::sleep(hold_duration);
    }
}

fn run_host_test_scripted_running(
    shared_state: Arc<Mutex<CompositorState>>,
) -> Result<(), RuntimeError> {
    const SCRIPTED_WAYLAND_SOCKET: &str = "wayland-test-running";
    const SCRIPTED_SEAT_NAME: &str = "seat-test";
    const SCRIPTED_DRM_PATH: &str = "/dev/dri/card-test";
    const SCRIPTED_CONNECTOR_NAME: &str = "TEST-1";
    const SCRIPTED_CONNECTOR_ID: u32 = 7;
    const SCRIPTED_WIDTH: i32 = 1280;
    const SCRIPTED_HEIGHT: i32 = 800;

    {
        let mut state = lock_state(&shared_state);
        state.set_runtime_host_backend_snapshot(
            Some(SCRIPTED_SEAT_NAME.to_string()),
            1,
            1,
            Some(SCRIPTED_DRM_PATH.to_string()),
        );
        state.mark_runtime_host_preflight_ready(Some(SCRIPTED_WAYLAND_SOCKET.to_string()));
    }
    maybe_sleep_host_runtime_test_scripted_hold();

    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_host_running(
            SCRIPTED_WAYLAND_SOCKET.to_string(),
            SCRIPTED_WIDTH,
            SCRIPTED_HEIGHT,
            Some(SCRIPTED_SEAT_NAME.to_string()),
            1,
            1,
            Some(SCRIPTED_DRM_PATH.to_string()),
            Some(SCRIPTED_CONNECTOR_NAME.to_string()),
            Some(SCRIPTED_CONNECTOR_ID),
            Some("scripted host runtime selected test connector".to_string()),
            Some("scripted host runtime claimed output ownership".to_string()),
            RuntimeHostPresentOwnership::Dumb,
            false,
            false,
        );
    }
    maybe_sleep_host_runtime_test_scripted_hold();
    Ok(())
}

pub fn run_winit(shared_state: Arc<Mutex<CompositorState>>) -> Result<(), RuntimeError> {
    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_starting(RuntimeBackend::Winit);
    }

    let mut event_loop: EventLoop<RuntimeLoopData> =
        EventLoop::try_new().map_err(|err| RuntimeError::EventLoop(err.to_string()))?;
    let loop_signal = event_loop.get_signal();
    let loop_handle = event_loop.handle();

    let display: Display<RuntimeWaylandState> =
        Display::new().map_err(|err| RuntimeError::WaylandDisplay(err.to_string()))?;
    let display_handle = display.handle();
    let mut wayland_state = RuntimeWaylandState::new(display_handle.clone(), shared_state.clone())
        .map_err(|err| RuntimeError::WaylandDisplay(err.code().to_string()))?;

    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|err| RuntimeError::WaylandSocket(err.to_string()))?;
    let socket_name = listening_socket
        .socket_name()
        .to_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| "wayland-unknown".to_string());

    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, CalloopMode::Level),
            |_, display, data| {
                // Safety: display is pinned in this event source for the runtime lifetime.
                let dispatch_result =
                    unsafe { display.get_mut().dispatch_clients(&mut data.wayland_state) };
                if dispatch_result.is_err() {
                    data.loop_signal.stop();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    let (mut backend, winit_source) =
        winit::init::<GlesRenderer>().map_err(|err| RuntimeError::WinitInit(err.to_string()))?;
    let size = backend.window_size();
    wayland_state.reconfigure_roles(size.w, size.h);
    wayland_state.sync_runtime_status_with_roles();

    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_running(
            RuntimeBackend::Winit,
            Some(socket_name.clone()),
            size.w,
            size.h,
        );
    }

    backend.window().request_redraw();

    loop_handle
        .insert_source(winit_source, move |event, _, data| match event {
            WinitEvent::Resized { size, .. } => {
                {
                    let mut state = lock_state(&data.shared_state);
                    state.mark_runtime_resize(size.w, size.h);
                }
                data.wayland_state.reconfigure_roles(size.w, size.h);
                backend.window().request_redraw();
            }
            WinitEvent::Input(event) => {
                data.wayland_state.forward_input_event(event);
                let mut state = lock_state(&data.shared_state);
                state.mark_runtime_input_event();
            }
            WinitEvent::Redraw => {
                data.wayland_state
                    .sync_output_rotation_reconfigure_if_needed();
                data.wayland_state.prune_dead_surfaces();
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                let rotation = data
                    .wayland_state
                    .root_geometry_snapshot()
                    .map(|snapshot| snapshot.rotation)
                    .unwrap_or_else(|| lock_state(&data.shared_state).output_rotation());
                let transform = transform_from_rotation(rotation);

                let render_result = (|| {
                    {
                        let (renderer, mut framebuffer) = backend
                            .bind()
                            .map_err(|err| format!("failed to bind winit frame: {err}"))?;

                        let logical_size = data.wayland_state.runtime_output_size();
                        let capture = data.wayland_state.collect_render_elements(
                            renderer,
                            logical_size.w,
                            logical_size.h,
                        );
                        if let Some(failure) = capture.failure.as_ref() {
                            return Err(failure.clone());
                        }
                        let mut frame = renderer
                            .render(&mut framebuffer, size, transform)
                            .map_err(|err| format!("failed to start render pass: {err}"))?;
                        frame
                            .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
                            .map_err(|err| format!("failed to clear frame: {err}"))?;
                        draw_render_elements(
                            &mut frame,
                            data.wayland_state.root_display_scale(),
                            &capture.elements,
                            &[damage],
                        )
                        .map_err(|err| format!("failed to draw surface elements: {err}"))?;
                        let _ = frame
                            .finish()
                            .map_err(|err| format!("failed to finish render pass: {err}"))?;
                        let mut cursor_frame = renderer
                            .render(&mut framebuffer, size, Transform::Normal)
                            .map_err(|err| format!("failed to start cursor render pass: {err}"))?;
                        draw_software_cursor_frame(
                            &mut cursor_frame,
                            data.wayland_state.cursor_render_location(),
                            Size::<i32, Physical>::from((logical_size.w, logical_size.h)),
                            size,
                            data.wayland_state.root_display_scale(),
                            rotation,
                        )
                        .map_err(|err| format!("failed to draw software cursor: {err}"))?;
                        let _ = cursor_frame
                            .finish()
                            .map_err(|err| format!("failed to finish cursor render pass: {err}"))?;
                    }
                    backend
                        .submit(Some(&[damage]))
                        .map_err(|err| format!("failed to submit winit frame: {err}"))?;
                    Ok::<(), String>(())
                })();

                match render_result {
                    Ok(()) => {
                        data.wayland_state.send_frame_callbacks();
                        let _ = data.display_handle.flush_clients();
                        let mut state = lock_state(&data.shared_state);
                        state.mark_runtime_redraw();
                        state.poll_processes();
                        backend.window().request_redraw();
                    }
                    Err(err) => {
                        let mut state = lock_state(&data.shared_state);
                        state.mark_runtime_failed(err);
                        data.loop_signal.stop();
                    }
                }
            }
            WinitEvent::CloseRequested => {
                {
                    let mut state = lock_state(&data.shared_state);
                    state.mark_runtime_stopped();
                }
                data.loop_signal.stop();
            }
            _ => {}
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    let mut runtime_data = RuntimeLoopData {
        shared_state,
        display_handle,
        loop_signal,
        wayland_state,
    };

    let run_result = event_loop.run(None, &mut runtime_data, |_| {});

    {
        let mut state = lock_state(&runtime_data.shared_state);
        if run_result.is_err() {
            state.mark_runtime_failed("calloop runtime loop failed");
        } else if !matches!(
            state.status_snapshot().runtime.phase,
            crate::model::RuntimePhase::Failed
        ) {
            state.mark_runtime_stopped();
        }
    }

    run_result.map_err(|err| RuntimeError::Loop(err.to_string()))?;
    Ok(())
}

pub fn run_host(
    shared_state: Arc<Mutex<CompositorState>>,
    screen_capture: ScreenCaptureStore,
    options: HostRuntimeOptions,
) -> Result<(), RuntimeError> {
    run_host_with_control(shared_state, screen_capture, options, None)
}

pub fn run_host_with_control(
    shared_state: Arc<Mutex<CompositorState>>,
    screen_capture: ScreenCaptureStore,
    options: HostRuntimeOptions,
    runtime_control: Option<Arc<Mutex<Receiver<RuntimeControlCommand>>>>,
) -> Result<(), RuntimeError> {
    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_starting(RuntimeBackend::HostDrm);
    }

    match host_runtime_test_scripted_phase(&shared_state) {
        Some(HostRuntimeTestScriptedFailurePhase::Starting) => {
            maybe_sleep_host_runtime_test_scripted_hold();
            return Err(RuntimeError::HostSession(
                "scripted host runtime failure after starting".to_string(),
            ));
        }
        Some(HostRuntimeTestScriptedFailurePhase::PreflightReady) => {
            {
                let mut state = lock_state(&shared_state);
                state.mark_runtime_host_preflight_ready(Some("wayland-test-preflight".to_string()));
            }
            maybe_sleep_host_runtime_test_scripted_hold();
            return Err(RuntimeError::HostOutputClaim {
                path: "<test-scripted-host-runtime>".to_string(),
                error: "scripted host runtime failure after preflight_ready".to_string(),
            });
        }
        Some(HostRuntimeTestScriptedFailurePhase::Running) => {
            return run_host_test_scripted_running(shared_state);
        }
        None => {}
    }

    if std::env::var_os("SURF_ACE_HOST_RUNTIME_FORCE_FAIL").is_some() {
        return Err(RuntimeError::HostSession(
            "forced host runtime failure".to_string(),
        ));
    }

    let mut event_loop: EventLoop<HostRuntimeLoopData> =
        EventLoop::try_new().map_err(|err| RuntimeError::EventLoop(err.to_string()))?;
    let loop_signal = event_loop.get_signal();
    let loop_handle = event_loop.handle();

    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|err| RuntimeError::WaylandSocket(err.to_string()))?;
    let socket_name = listening_socket
        .socket_name()
        .to_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| "wayland-unknown".to_string());

    let (session, session_notifier) =
        LibSeatSession::new().map_err(|err| RuntimeError::HostSession(err.to_string()))?;
    let seat_name = session.seat();
    let mut libinput_context =
        Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput_context.udev_assign_seat(&seat_name).map_err(|_| {
        RuntimeError::HostUdev(format!("failed to assign libinput seat {seat_name}"))
    })?;
    let libinput_backend = LibinputInputBackend::new(libinput_context);
    let udev =
        UdevBackend::new(&seat_name).map_err(|err| RuntimeError::HostUdev(err.to_string()))?;
    let preferred_primary_path = options
        .forced_drm_path
        .clone()
        .or_else(|| primary_gpu(&seat_name).ok().flatten());

    let mut host_backend = HostBackendState::new(
        session,
        seat_name.clone(),
        preferred_primary_path.clone(),
        options.forced_drm_path.clone(),
        options.forced_output_name.clone(),
        screen_capture,
    );
    let mut initial_devices: Vec<(u64, PathBuf)> = udev
        .device_list()
        .map(|(device_id, path)| (device_id as u64, path.to_path_buf()))
        .collect();
    initial_devices.sort_by(|(_, left_path), (_, right_path)| {
        host_device_sort_key(left_path, preferred_primary_path.as_deref()).cmp(
            &host_device_sort_key(right_path, preferred_primary_path.as_deref()),
        )
    });
    for (device_id, path) in initial_devices {
        if let Err(err) = host_backend.upsert_device(device_id, path.clone()) {
            eprintln!("host backend skipped drm device {}: {err}", path.display());
        }
    }

    {
        let mut state = lock_state(&shared_state);
        state.set_runtime_host_backend_snapshot(
            Some(host_backend.seat_name.clone()),
            host_backend.detected_count(),
            host_backend.opened_count(),
            host_backend.primary_opened_path(),
        );
    }
    sync_runtime_host_selection_status(&shared_state, &host_backend);

    if host_backend.detected_count() == 0 {
        return Err(RuntimeError::HostNoDrmDevices(seat_name));
    }
    if host_backend.opened_count() == 0 {
        return Err(RuntimeError::HostNoDrmDeviceOpened(
            host_backend.seat_name.clone(),
        ));
    }

    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_host_preflight_ready(Some(socket_name.clone()));
        state.set_runtime_host_backend_snapshot(
            Some(host_backend.seat_name.clone()),
            host_backend.detected_count(),
            host_backend.opened_count(),
            host_backend.primary_opened_path(),
        );
    }
    sync_runtime_host_selection_status(&shared_state, &host_backend);

    let display: Display<RuntimeWaylandState> =
        Display::new().map_err(|err| RuntimeError::WaylandDisplay(err.to_string()))?;
    let display_handle = display.handle();
    let mut startup_wayland_state = None;
    let claimed_output = {
        let mut stage_startup_geometry = |width: u16, height: u16| {
            let prepared = lock_state(&shared_state)
                .prepare_initial_root4_geometry(width as i32, height as i32)
                .map_err(|err| RuntimeError::HostOutputClaim {
                    path: "root4".to_string(),
                    error: err.code().to_string(),
                })?;
            startup_wayland_state = Some(
                RuntimeWaylandState::new_with_initial_root_geometry(
                    display_handle.clone(),
                    shared_state.clone(),
                    Some(prepared),
                )
                .map_err(|err| RuntimeError::HostOutputClaim {
                    path: "root4".to_string(),
                    error: err.code().to_string(),
                })?,
            );
            Ok(())
        };
        match host_backend.claim_output_ownership(None, Some(&mut stage_startup_geometry), false) {
            Ok(claimed_output) => claimed_output,
            Err(err) => {
                sync_runtime_host_selection_status(&shared_state, &host_backend);
                return Err(err);
            }
        }
    };
    let mut wayland_state =
        startup_wayland_state
            .take()
            .ok_or_else(|| RuntimeError::HostOutputClaim {
                path: "root4".to_string(),
                error: "display_scale_apply_failed".to_string(),
            })?;
    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, CalloopMode::Level),
            |_, display, data| {
                // Safety: display is pinned in this event source for the runtime lifetime.
                let dispatch_result =
                    unsafe { display.get_mut().dispatch_clients(&mut data.wayland_state) };
                if dispatch_result.is_err() {
                    data.loop_signal.stop();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;
    sync_runtime_host_selection_status(&shared_state, &host_backend);
    sync_runtime_host_present_capabilities(&shared_state, &host_backend);
    wayland_state
        .sync_dmabuf_protocol_formats(host_backend.claimed_dmabuf_protocol_advertisement());
    let (mode_w, mode_h) = claimed_output.mode.size();
    let reclaim_required_ownership = if matches!(
        claimed_output.startup_present_ownership,
        StartupPresentOwnership::DirectGbm
    ) {
        Some(StartupPresentOwnership::DirectGbm)
    } else {
        None
    };
    let drm_events_fd =
        host_backend
            .claimed_device_event_fd()
            .ok_or_else(|| RuntimeError::HostOutputClaim {
                path: host_backend
                    .primary_opened_path()
                    .unwrap_or_else(|| "<unknown-device>".to_string()),
                error: "claimed output device fd missing".to_string(),
            })?;
    loop_handle
        .insert_source(listening_socket, move |client_stream, _, data| {
            data.wayland_state.sync_output_state();
            let _ = data
                .display_handle
                .insert_client(client_stream, Arc::new(RuntimeClientState::default()));
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;
    wayland_state.reconfigure_roles(mode_w as i32, mode_h as i32);
    wayland_state.sync_runtime_status_with_roles();
    {
        let (active_connector_name, active_connector_id) = host_backend.active_connector_status();
        let (last_selection_attempt, last_selection_result) = host_backend.selection_logs();
        let rotation = { lock_state(&shared_state).output_rotation() };
        let (ownership, atomic_enabled, overlay_capable) =
            runtime_host_present_capabilities_for_status(&host_backend, rotation);
        let mut state = lock_state(&shared_state);
        state.mark_runtime_host_running(
            socket_name.clone(),
            mode_w as i32,
            mode_h as i32,
            Some(host_backend.seat_name.clone()),
            host_backend.detected_count(),
            host_backend.opened_count(),
            host_backend.primary_opened_path(),
            active_connector_name,
            active_connector_id,
            last_selection_attempt,
            last_selection_result,
            ownership,
            atomic_enabled,
            overlay_capable,
        );
    }

    let drm_events_source_token = Rc::new(RefCell::new(None));
    bind_claimed_drm_event_source(
        &loop_handle,
        drm_events_fd,
        Rc::clone(&drm_events_source_token),
    )?;
    loop_handle
        .insert_source(libinput_backend, |event, _, data| {
            data.wayland_state.forward_input_event(event);
            let mut state = lock_state(&data.shared_state);
            state.mark_runtime_input_event();
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    let loop_handle_for_timer = loop_handle.clone();
    let drm_events_source_token_for_timer = Rc::clone(&drm_events_source_token);
    loop_handle
        .insert_source(Timer::immediate(), move |_, _, data| {
            data.wayland_state.prune_dead_surfaces();
            if data.host_backend.claimed_output.is_none() {
                sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
                data.wayland_state.sync_dmabuf_protocol_formats(None);
                if let Err(err) = reclaim_host_output_in_process(
                    data,
                    &loop_handle_for_timer,
                    &drm_events_source_token_for_timer,
                    reclaim_required_ownership,
                ) {
                    mark_host_output_reclaim_pending(
                        data,
                        format!("host backend has no claimed output and reclaim failed: {err}"),
                    );
                    return TimeoutAction::ToDuration(Duration::from_millis(250));
                }
                return TimeoutAction::ToDuration(Duration::from_millis(16));
            }
            data.stage_next_root_geometry_mutation();
            if !data.wayland_state.staged_native_materializations_ready() {
                return TimeoutAction::ToDuration(Duration::from_millis(16));
            }
            let presentation = data.queue_pinned_presentation_tick();
            match presentation {
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(failure) => {
                    data.discard_staged_root_geometry(
                        crate::root_geometry::RootGeometryError::DisplayScaleApplyFailed,
                    );
                    if failure.is_transaction() {
                        eprintln!("root4 geometry transaction rejected: {}", failure.error_ref());
                        return TimeoutAction::ToDuration(Duration::from_millis(16));
                    }
                    if failure.is_reclaimable() {
                        data.host_backend.mark_claim_lost();
                        sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
                        data.wayland_state.sync_dmabuf_protocol_formats(None);
                        if let Err(reclaim_err) = reclaim_host_output_in_process(
                            data,
                            &loop_handle_for_timer,
                            &drm_events_source_token_for_timer,
                            reclaim_required_ownership,
                        ) {
                            mark_host_output_reclaim_pending(
                                data,
                                format!(
                                "host present/commit recovery failed after queue error ({}): {reclaim_err}",
                                failure.error_ref()
                                ),
                            );
                            return TimeoutAction::ToDuration(Duration::from_millis(250));
                        }
                        return TimeoutAction::ToDuration(Duration::from_millis(16));
                    }
                    let mut state = lock_state(&data.shared_state);
                    state.mark_runtime_failed(format!(
                        "failed while queuing host presentation frame: {}",
                        failure.into_error()
                    ));
                    data.loop_signal.stop();
                    return TimeoutAction::Drop;
                }
            }
            TimeoutAction::ToDuration(Duration::from_millis(16))
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    loop_handle
        .insert_source(session_notifier, |event, _, data| match event {
            SessionSignal::PauseSession => {
                let mut state = lock_state(&data.shared_state);
                state.mark_runtime_failed("host session paused");
                data.loop_signal.stop();
            }
            SessionSignal::ActivateSession => {
                let mut state = lock_state(&data.shared_state);
                state.set_runtime_host_backend_snapshot(
                    Some(data.host_backend.seat_name.clone()),
                    data.host_backend.detected_count(),
                    data.host_backend.opened_count(),
                    data.host_backend.primary_opened_path(),
                );
            }
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    let loop_handle_for_udev = loop_handle.clone();
    let drm_events_source_token_for_udev = Rc::clone(&drm_events_source_token);
    loop_handle
        .insert_source(udev, move |event, _, data| {
            match event {
                UdevEvent::Added { device_id, path } => {
                    if data
                        .host_backend
                        .forced_drm_path
                        .as_deref()
                        .is_some_and(|forced| forced != path.as_path())
                    {
                        return;
                    }
                    if let Err(err) = data.host_backend.upsert_device(device_id as u64, path) {
                        eprintln!("host backend failed to open added drm device: {err}");
                    }
                }
                UdevEvent::Changed { device_id } => {
                    if let Some(path) = data.host_backend.path_for(device_id as u64).cloned() {
                        if let Err(err) = data.host_backend.upsert_device(device_id as u64, path) {
                            eprintln!("host backend failed to refresh changed drm device: {err}");
                        }
                    }
                }
                UdevEvent::Removed { device_id } => {
                    if let Err(err) = data.host_backend.remove_device(device_id as u64) {
                        eprintln!("host backend failed to remove drm device: {err}");
                    }
                }
            }

            if data.host_backend.opened_count() == 0 {
                let mut state = lock_state(&data.shared_state);
                state.set_runtime_host_backend_snapshot(
                    Some(data.host_backend.seat_name.clone()),
                    data.host_backend.detected_count(),
                    data.host_backend.opened_count(),
                    data.host_backend.primary_opened_path(),
                );
                state.mark_runtime_failed("host backend has no opened drm devices");
                data.loop_signal.stop();
                return;
            }
            if data.host_backend.claimed_output.is_none() {
                sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
                data.wayland_state.sync_dmabuf_protocol_formats(None);
                if let Err(err) = reclaim_host_output_in_process(
                    data,
                    &loop_handle_for_udev,
                    &drm_events_source_token_for_udev,
                    reclaim_required_ownership,
                ) {
                    mark_host_output_reclaim_pending(
                        data,
                        format!("host backend lost claimed output and reclaim failed: {err}"),
                    );
                    return;
                }
            } else {
                let mut state = lock_state(&data.shared_state);
                state.set_runtime_host_backend_snapshot(
                    Some(data.host_backend.seat_name.clone()),
                    data.host_backend.detected_count(),
                    data.host_backend.opened_count(),
                    data.host_backend.primary_opened_path(),
                );
            }
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;

    let mut runtime_data = HostRuntimeLoopData {
        shared_state,
        display_handle,
        loop_signal,
        wayland_state,
        host_backend,
        runtime_control,
        pending_rotation_response: None,
        root_geometry_queue: RuntimeGeometryMutationQueue::default(),
        pending_geometry_mutation: None,
        root_geometry_flip_boundary: RootGeometryFlipBoundary::default(),
        pending_reclaim_publication: None,
        #[cfg(test)]
        root_geometry_stage_failure: None,
    };

    let run_result = event_loop.run(None, &mut runtime_data, |_| {});

    {
        let mut state = lock_state(&runtime_data.shared_state);
        if run_result.is_err() {
            state.mark_runtime_failed("calloop runtime loop failed");
        } else if !matches!(
            state.status_snapshot().runtime.phase,
            crate::model::RuntimePhase::Failed
        ) {
            state.mark_runtime_stopped();
        }
    }

    run_result.map_err(|err| RuntimeError::Loop(err.to_string()))?;
    Ok(())
}

struct HostRuntimeLoopData {
    shared_state: Arc<Mutex<CompositorState>>,
    display_handle: DisplayHandle,
    loop_signal: LoopSignal,
    wayland_state: RuntimeWaylandState,
    host_backend: HostBackendState,
    runtime_control: Option<Arc<Mutex<Receiver<RuntimeControlCommand>>>>,
    pending_rotation_response:
        Option<std::sync::mpsc::SyncSender<Result<(), crate::root_geometry::RootGeometryError>>>,
    root_geometry_queue: RuntimeGeometryMutationQueue,
    pending_geometry_mutation: Option<(u64, &'static str)>,
    root_geometry_flip_boundary: RootGeometryFlipBoundary,
    pending_reclaim_publication: Option<PendingReclaimPublication>,
    #[cfg(test)]
    root_geometry_stage_failure: Option<Root4ConsumerStage>,
}

struct PendingReclaimPublication {
    mode_width: i32,
    mode_height: i32,
    active_connector_name: Option<String>,
    active_connector_id: Option<u32>,
    last_selection_attempt: Option<String>,
    last_selection_result: Option<String>,
    ownership: RuntimeHostPresentOwnership,
    atomic_enabled: bool,
    overlay_capable: bool,
}

#[derive(Default)]
struct RootGeometryFlipBoundary {
    queued: Option<PresentationToken>,
}

impl RootGeometryFlipBoundary {
    fn mark_queued(&mut self, token: PresentationToken) {
        self.queued = Some(token);
    }

    fn take_completed(&mut self, completed: &[PresentationToken]) -> bool {
        self.queued
            .take_if(|queued| completed.contains(queued))
            .is_some()
    }

    fn discard(&mut self) {
        self.queued = None;
    }
}

/// The narrow side-effect boundary used by the root4 transaction engine.
/// Production delegates this to the host capture store; tests can drive the
/// same staging/activation code without opening a seat or a DRM device.
trait RootGeometryStageStore {
    fn begin_stage(&mut self, generation: u64);
    fn commit_stage(&mut self, generation: u64);
    fn discard_stage(&mut self, generation: u64);
}

impl RootGeometryStageStore for HostBackendState {
    fn begin_stage(&mut self, generation: u64) {
        self.screen_capture.begin_root4_stage(generation);
    }

    fn commit_stage(&mut self, generation: u64) {
        self.screen_capture.commit_root4_stage(generation);
    }

    fn discard_stage(&mut self, generation: u64) {
        self.screen_capture.discard_root4_stage(generation);
    }
}

fn install_root_geometry_stage(
    wayland_state: &mut RuntimeWaylandState,
    stage_store: &mut impl RootGeometryStageStore,
    staged: StagedRuntimeRootGeometry,
) {
    stage_store.begin_stage(staged.committed.snapshot.generation);
    wayland_state.staged_root_geometry = Some(staged.clone());
    wayland_state.presentation_root_geometry = Some(staged);
    wayland_state.stage_role_configures();
    wayland_state.presentation_root_geometry = None;
}

fn activate_root_geometry_stage(
    shared_state: &Arc<Mutex<CompositorState>>,
    wayland_state: &mut RuntimeWaylandState,
    stage_store: &mut impl RootGeometryStageStore,
    publish: impl FnOnce(&mut CompositorState),
) -> Option<StagedRuntimeRootGeometry> {
    let prepared = wayland_state.staged_root_geometry.take()?;
    let mut state = lock_state(shared_state);
    state.mark_runtime_root4_dimensions_committed(
        prepared.committed.snapshot.physical_size_px.width,
        prepared.committed.snapshot.physical_size_px.height,
    );
    state.commit_root4_geometry_consumers(
        prepared.committed,
        prepared.root_layout.clone(),
        &prepared.viewports,
    );
    publish(&mut state);
    stage_store.commit_stage(prepared.committed.snapshot.generation);
    wayland_state.backend_output_size = Size::<i32, Physical>::from((
        prepared.committed.snapshot.physical_size_px.width,
        prepared.committed.snapshot.physical_size_px.height,
    ));
    wayland_state.activate_output_global(prepared.output_global.clone());
    wayland_state.active_root_geometry_consumers = Some(prepared.clone());
    wayland_state.applied_output_rotation = prepared.committed.snapshot.rotation;
    wayland_state.applied_root_geometry_generation = prepared.committed.snapshot.generation;
    Some(prepared)
}

struct QueuedRuntimeGeometryMutation {
    sequence: u64,
    mutation: RuntimeGeometryMutation,
}

#[derive(Default)]
struct RuntimeGeometryMutationQueue {
    next_sequence: u64,
    pending: VecDeque<QueuedRuntimeGeometryMutation>,
}

impl RuntimeGeometryMutationQueue {
    fn push(&mut self, mutation: RuntimeGeometryMutation) -> u64 {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        self.pending
            .push_back(QueuedRuntimeGeometryMutation { sequence, mutation });
        sequence
    }

    fn pop(&mut self) -> Option<QueuedRuntimeGeometryMutation> {
        self.pending.pop_front()
    }
}

enum RuntimeGeometryMutation {
    Scale {
        factor: f64,
        source: crate::root_geometry::DisplayScaleSource,
    },
    Rotation {
        rotation: OutputRotation,
        response: std::sync::mpsc::SyncSender<Result<(), crate::root_geometry::RootGeometryError>>,
    },
    Mode {
        width: i32,
        height: i32,
    },
}

impl HostRuntimeLoopData {
    fn complete_root_geometry_presentations(&mut self, completed: &[PresentationToken]) -> bool {
        if self.root_geometry_flip_boundary.take_completed(completed) {
            self.commit_staged_root_geometry();
            true
        } else {
            false
        }
    }

    fn queue_pinned_presentation_tick(
        &mut self,
    ) -> Result<Option<PresentationToken>, HostPresentFailure> {
        let material_identity = self.wayland_state.accepted_material_identity();
        let identity_accepted =
            self.wayland_state
                .staged_root_geometry
                .as_mut()
                .is_none_or(|staged| {
                    staged.accept_or_validate_material_identity(material_identity.clone())
                });
        if !identity_accepted {
            return Err(self.material_identity_changed_failure());
        }
        self.wayland_state.presentation_root_geometry =
            self.wayland_state.staged_root_geometry.clone();
        let queued = self
            .host_backend
            .queue_claimed_presentation_tick(&mut self.wayland_state);
        let materials_unchanged = self
            .wayland_state
            .presentation_root_geometry
            .as_ref()
            .is_none_or(|presentation| {
                presentation.accepted_material_identity.as_ref()
                    == Some(&self.wayland_state.accepted_material_identity())
            });
        self.wayland_state.presentation_root_geometry = None;
        if !materials_unchanged {
            return Err(self.material_identity_changed_failure());
        }
        if let Ok(Some(token)) = queued
            && self.wayland_state.staged_root_geometry.is_some()
        {
            self.root_geometry_flip_boundary.mark_queued(token);
        }
        queued
    }

    fn material_identity_changed_failure(&self) -> HostPresentFailure {
        HostPresentFailure::transaction(RuntimeError::HostOutputClaim {
            path: self
                .host_backend
                .primary_opened_path()
                .unwrap_or_else(|| "<selected-root4-device>".to_string()),
            error: "root4 surface material changed during the pinned presentation".to_string(),
        })
    }

    fn install_staged_root_geometry(&mut self, staged: StagedRuntimeRootGeometry) {
        install_root_geometry_stage(&mut self.wayland_state, &mut self.host_backend, staged);
    }

    fn stage_root4_mode_if_changed(
        &mut self,
        width: i32,
        height: i32,
    ) -> Result<(), crate::root_geometry::RootGeometryError> {
        let staged = {
            let state = lock_state(&self.shared_state);
            state
                .prepare_root4_mode(width, height)
                .and_then(|prepared| {
                    #[cfg(test)]
                    {
                        stage_runtime_root_geometry_consumers(
                            &state,
                            prepared,
                            self.root_geometry_stage_failure,
                        )
                    }
                    #[cfg(not(test))]
                    {
                        stage_runtime_root_geometry_consumers(&state, prepared)
                    }
                })?
        };
        self.install_staged_root_geometry(staged);
        Ok(())
    }

    fn ingest_root_geometry_commands(&mut self) {
        let commands = self.runtime_control.as_ref().map(|receiver| {
            let receiver = match receiver.lock() {
                Ok(receiver) => receiver,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut commands = Vec::new();
            while let Ok(command) = receiver.try_recv() {
                commands.push(command);
            }
            commands
        });
        for command in commands.into_iter().flatten() {
            match command {
                RuntimeControlCommand::Root4ConfigScaleSet { factor, source } => {
                    self.root_geometry_queue
                        .push(RuntimeGeometryMutation::Scale { factor, source });
                }
                RuntimeControlCommand::SetOutputRotation { rotation, response } => {
                    self.root_geometry_queue
                        .push(RuntimeGeometryMutation::Rotation { rotation, response });
                }
                RuntimeControlCommand::StartHostRuntime => {}
            }
        }
    }

    fn stage_next_root_geometry_mutation(&mut self) {
        if self.wayland_state.staged_root_geometry.is_some() {
            return;
        }
        self.ingest_root_geometry_commands();
        let command = self.root_geometry_queue.pop();
        match command {
            Some(QueuedRuntimeGeometryMutation {
                sequence,
                mutation: RuntimeGeometryMutation::Scale { factor, source },
            }) => {
                let staged = {
                    let state = lock_state(&self.shared_state);
                    state
                        .prepare_root4_display_scale_from_config(factor, source)
                        .and_then(|prepared| {
                            #[cfg(test)]
                            {
                                stage_runtime_root_geometry_consumers(
                                    &state,
                                    prepared,
                                    self.root_geometry_stage_failure,
                                )
                            }
                            #[cfg(not(test))]
                            {
                                stage_runtime_root_geometry_consumers(&state, prepared)
                            }
                        })
                };
                match staged {
                    Ok(prepared) => {
                        self.install_staged_root_geometry(prepared);
                        self.pending_geometry_mutation = Some((sequence, "scale"));
                    }
                    Err(err) => {
                        eprintln!(
                            "{{\"ok\":false,\"error\":\"{}\",\"mutation\":\"scale\",\"sequence\":{sequence}}}",
                            err.code()
                        );
                    }
                }
            }
            Some(QueuedRuntimeGeometryMutation {
                sequence,
                mutation: RuntimeGeometryMutation::Rotation { rotation, response },
            }) => {
                let staged = {
                    let state = lock_state(&self.shared_state);
                    state.prepare_root4_rotation(rotation).and_then(|prepared| {
                        #[cfg(test)]
                        {
                            stage_runtime_root_geometry_consumers(
                                &state,
                                prepared,
                                self.root_geometry_stage_failure,
                            )
                        }
                        #[cfg(not(test))]
                        {
                            stage_runtime_root_geometry_consumers(&state, prepared)
                        }
                    })
                };
                match staged {
                    Ok(prepared) => {
                        self.install_staged_root_geometry(prepared);
                        self.pending_rotation_response = Some(response);
                        self.pending_geometry_mutation = Some((sequence, "rotation"));
                    }
                    Err(err) => {
                        eprintln!(
                            "{{\"ok\":false,\"error\":\"{}\",\"mutation\":\"rotation\",\"sequence\":{sequence}}}",
                            err.code()
                        );
                        let _ = response.send(Err(err));
                    }
                }
            }
            Some(QueuedRuntimeGeometryMutation {
                sequence,
                mutation: RuntimeGeometryMutation::Mode { width, height },
            }) => match self
                .host_backend
                .arm_prepared_reclaim_for_presentation()
                .and_then(|()| {
                    self.stage_root4_mode_if_changed(width, height)
                        .map_err(|err| RuntimeError::HostOutputClaim {
                            path: "root4".to_string(),
                            error: err.code().to_string(),
                        })
                }) {
                Ok(()) => self.pending_geometry_mutation = Some((sequence, "mode")),
                Err(_err) => {
                    self.host_backend.discard_unactivated_reclaim();
                    eprintln!(
                        "{{\"ok\":false,\"error\":\"display_scale_apply_failed\",\"mutation\":\"mode\",\"sequence\":{sequence}}}"
                    );
                }
            },
            None => {}
        }
    }

    fn commit_staged_root_geometry(&mut self) {
        self.root_geometry_flip_boundary.discard();
        let activated_reclaim = self
            .pending_geometry_mutation
            .is_some_and(|(_, mutation)| mutation == "mode");
        let reclaim_publication = if activated_reclaim {
            self.pending_reclaim_publication.take().map(|publication| {
                (
                    publication,
                    self.host_backend.seat_name.clone(),
                    self.host_backend.detected_count(),
                    self.host_backend.opened_count(),
                    self.host_backend.primary_opened_path(),
                )
            })
        } else {
            None
        };
        let Some(prepared) = activate_root_geometry_stage(
            &self.shared_state,
            &mut self.wayland_state,
            &mut self.host_backend,
            |state| {
                if let Some((publication, seat_name, detected, opened, path)) = reclaim_publication
                {
                    state.set_runtime_host_backend_snapshot(
                        Some(seat_name),
                        detected,
                        opened,
                        path,
                    );
                    state.mark_runtime_host_output_reclaimed(
                        publication.mode_width,
                        publication.mode_height,
                        publication.active_connector_name,
                        publication.active_connector_id,
                        publication.last_selection_attempt,
                        publication.last_selection_result,
                        publication.ownership,
                        publication.atomic_enabled,
                        publication.overlay_capable,
                    );
                }
            },
        ) else {
            return;
        };
        if activated_reclaim {
            self.host_backend.finish_reclaim_activation();
            self.wayland_state.sync_dmabuf_protocol_formats(
                self.host_backend.claimed_dmabuf_protocol_advertisement(),
            );
        }
        if let Some(response) = self.pending_rotation_response.take() {
            let _ = response.send(Ok(()));
        }
        if let Some((sequence, mutation)) = self.pending_geometry_mutation.take() {
            eprintln!(
                "{{\"ok\":true,\"mutation\":\"{mutation}\",\"sequence\":{sequence},\"rootGeometryGeneration\":{}}}",
                prepared.committed.snapshot.generation
            );
        }
    }

    fn discard_staged_root_geometry(&mut self, error: crate::root_geometry::RootGeometryError) {
        self.root_geometry_flip_boundary.discard();
        if let Some(prepared) = self.wayland_state.staged_root_geometry.take() {
            self.host_backend
                .discard_stage(prepared.committed.snapshot.generation);
        }
        self.wayland_state
            .sync_output_rotation_reconfigure_if_needed();
        if let Some(response) = self.pending_rotation_response.take() {
            let _ = response.send(Err(error));
        }
        if self
            .pending_geometry_mutation
            .is_some_and(|(_, mutation)| mutation == "mode")
        {
            self.pending_reclaim_publication = None;
            self.host_backend.discard_unactivated_reclaim();
        }
        if let Some((sequence, mutation)) = self.pending_geometry_mutation.take() {
            eprintln!(
                "{{\"ok\":false,\"error\":\"{}\",\"mutation\":\"{mutation}\",\"sequence\":{sequence}}}",
                error.code()
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPresentFailureClass {
    Reclaimable,
    Transaction,
    Fatal,
}

struct HostPresentFailure {
    class: HostPresentFailureClass,
    error: RuntimeError,
}

impl HostPresentFailure {
    fn reclaimable(error: RuntimeError) -> Self {
        Self {
            class: HostPresentFailureClass::Reclaimable,
            error,
        }
    }

    fn fatal(error: RuntimeError) -> Self {
        Self {
            class: HostPresentFailureClass::Fatal,
            error,
        }
    }

    fn transaction(error: RuntimeError) -> Self {
        Self {
            class: HostPresentFailureClass::Transaction,
            error,
        }
    }

    fn is_reclaimable(&self) -> bool {
        matches!(self.class, HostPresentFailureClass::Reclaimable)
    }

    fn is_transaction(&self) -> bool {
        matches!(self.class, HostPresentFailureClass::Transaction)
    }

    fn error_ref(&self) -> &RuntimeError {
        &self.error
    }

    fn into_error(self) -> RuntimeError {
        self.error
    }
}

impl From<RuntimeError> for HostPresentFailure {
    fn from(error: RuntimeError) -> Self {
        Self::fatal(error)
    }
}

fn process_claimed_drm_event_source(
    data: &mut HostRuntimeLoopData,
) -> Result<(), HostPresentFailure> {
    let completed = data.host_backend.process_claimed_presentation_events()?;
    if !completed.is_empty() {
        data.complete_root_geometry_presentations(&completed);
        data.wayland_state.prune_dead_surfaces();
        data.wayland_state.send_frame_callbacks();
        let _ = data.display_handle.flush_clients();
        let mut state = lock_state(&data.shared_state);
        for _ in &completed {
            state.mark_runtime_redraw();
        }
        state.poll_processes();
        drop(state);
        data.queue_pinned_presentation_tick()?;
    }
    Ok(())
}

fn sync_runtime_host_present_capabilities(
    shared_state: &Arc<Mutex<CompositorState>>,
    host_backend: &HostBackendState,
) {
    let rotation = { lock_state(shared_state).output_rotation() };
    let (ownership, atomic_enabled, overlay_capable) =
        runtime_host_present_capabilities_for_status(host_backend, rotation);
    let mut state = lock_state(shared_state);
    state.set_runtime_host_present_capabilities(ownership, atomic_enabled, overlay_capable);
}

fn runtime_host_present_capabilities_for_status(
    host_backend: &HostBackendState,
    rotation: OutputRotation,
) -> (RuntimeHostPresentOwnership, bool, bool) {
    let (mut ownership, atomic_enabled, overlay_capable) = host_backend
        .claimed_present_capabilities()
        .unwrap_or((RuntimeHostPresentOwnership::None, false, false));
    if matches!(ownership, RuntimeHostPresentOwnership::DirectGbm)
        && !direct_present_supported_for_rotation(rotation)
    {
        ownership = RuntimeHostPresentOwnership::Dumb;
    }
    (ownership, atomic_enabled, overlay_capable)
}

fn sync_runtime_host_selection_status(
    shared_state: &Arc<Mutex<CompositorState>>,
    host_backend: &HostBackendState,
) {
    let (device_selection_state, output_selection_state) = host_backend.selection_states();
    let (active_connector_name, active_connector_id) = host_backend.active_connector_status();
    let (last_selection_attempt, last_selection_result) = host_backend.selection_logs();
    let mut state = lock_state(shared_state);
    state.set_runtime_host_selection_overrides(
        host_backend.forced_drm_path_str(),
        host_backend.forced_output_name(),
        device_selection_state,
        output_selection_state,
    );
    state.set_runtime_host_route_selection_status(
        active_connector_name,
        active_connector_id,
        last_selection_attempt,
        last_selection_result,
    );
}

fn reclaim_host_output_in_process(
    data: &mut HostRuntimeLoopData,
    loop_handle: &LoopHandle<'_, HostRuntimeLoopData>,
    drm_events_source_token: &Rc<RefCell<Option<RegistrationToken>>>,
    reclaim_required_ownership: Option<StartupPresentOwnership>,
) -> Result<(), RuntimeError> {
    let root_geometry_queue = &mut data.root_geometry_queue;
    let mut observe_mode_before_activation = |width: u16, height: u16| {
        root_geometry_queue.push(RuntimeGeometryMutation::Mode {
            width: width as i32,
            height: height as i32,
        });
        Ok(())
    };
    let claimed_output = match data.host_backend.claim_output_ownership(
        reclaim_required_ownership,
        Some(&mut observe_mode_before_activation),
        true,
    ) {
        Ok(claimed_output) => claimed_output,
        Err(err) => {
            sync_runtime_host_selection_status(&data.shared_state, &data.host_backend);
            return Err(err);
        }
    };
    if let Some(old_token) = drm_events_source_token.borrow_mut().take() {
        loop_handle.remove(old_token);
    }
    let drm_events_fd = data.host_backend.claimed_device_event_fd().ok_or_else(|| {
        RuntimeError::HostOutputClaim {
            path: data
                .host_backend
                .primary_opened_path()
                .unwrap_or_else(|| "<unknown-device>".to_string()),
            error: "host backend reclaimed output but no drm event fd was available".to_string(),
        }
    })?;
    bind_claimed_drm_event_source(
        loop_handle,
        drm_events_fd,
        Rc::clone(drm_events_source_token),
    )?;
    let (mode_w, mode_h) = claimed_output.mode.size();
    let active_connector_name = Some(claimed_output.identity.connector_name.clone());
    let active_connector_id = Some(claimed_output.identity.connector_id);
    let (last_selection_attempt, last_selection_result) = data.host_backend.selection_logs();
    let rotation = { lock_state(&data.shared_state).output_rotation() };
    let (mut ownership, atomic_enabled, overlay_capable) = data
        .host_backend
        .present_capabilities_for(&claimed_output)
        .unwrap_or((RuntimeHostPresentOwnership::None, false, false));
    if matches!(ownership, RuntimeHostPresentOwnership::DirectGbm)
        && !direct_present_supported_for_rotation(rotation)
    {
        ownership = RuntimeHostPresentOwnership::Dumb;
    }
    data.pending_reclaim_publication = Some(PendingReclaimPublication {
        mode_width: mode_w as i32,
        mode_height: mode_h as i32,
        active_connector_name,
        active_connector_id,
        last_selection_attempt,
        last_selection_result,
        ownership,
        atomic_enabled,
        overlay_capable,
    });
    Ok(())
}

fn mark_host_output_reclaim_pending(data: &mut HostRuntimeLoopData, error: String) {
    sync_runtime_host_selection_status(&data.shared_state, &data.host_backend);
    sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
    let mut state = lock_state(&data.shared_state);
    state.set_runtime_host_backend_snapshot(
        Some(data.host_backend.seat_name.clone()),
        data.host_backend.detected_count(),
        data.host_backend.opened_count(),
        data.host_backend.primary_opened_path(),
    );
    state.mark_runtime_host_output_reclaim_pending(error);
}

fn bind_claimed_drm_event_source(
    loop_handle: &LoopHandle<'_, HostRuntimeLoopData>,
    drm_events_fd: OwnedFd,
    drm_events_source_token: Rc<RefCell<Option<RegistrationToken>>>,
) -> Result<(), RuntimeError> {
    let token_state_for_cb = Rc::clone(&drm_events_source_token);
    let token = loop_handle
        .insert_source(
            Generic::new(drm_events_fd, Interest::READ, CalloopMode::Level),
            move |_, _fd, data| {
                if let Err(failure) = process_claimed_drm_event_source(data) {
                    if failure.is_reclaimable() {
                        eprintln!(
                            "host backend lost present/event stream after commit/present error: {}; scheduling in-process reclaim",
                            failure.error_ref()
                        );
                        data.host_backend.mark_claim_lost();
                        sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
                        data.wayland_state.sync_dmabuf_protocol_formats(None);
                        *token_state_for_cb.borrow_mut() = None;
                        return Ok(PostAction::Remove);
                    }
                    let mut state = lock_state(&data.shared_state);
                    state.mark_runtime_failed(format!(
                        "failed while processing host presentation events: {}",
                        failure.into_error()
                    ));
                    data.loop_signal.stop();
                    *token_state_for_cb.borrow_mut() = None;
                    return Ok(PostAction::Remove);
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;
    *drm_events_source_token.borrow_mut() = Some(token);
    Ok(())
}

struct HostBackendState {
    session: Option<LibSeatSession>,
    seat_name: String,
    preferred_primary_path: Option<PathBuf>,
    forced_drm_path: Option<PathBuf>,
    forced_output_name: Option<String>,
    screen_capture: ScreenCaptureStore,
    detected_devices: HashMap<u64, PathBuf>,
    opened_devices: HashMap<u64, OpenedHostDevice>,
    claimed_output: Option<ClaimedHostOutput>,
    retired_claim: Option<RetiredHostClaim>,
    prepared_reclaim_output: Option<ClaimedHostOutput>,
    last_good_output_identity: Option<OutputIdentity>,
    device_selection_state: RuntimeHostSelectionState,
    output_selection_state: RuntimeHostSelectionState,
    last_selection_attempt: Option<String>,
    last_selection_result: Option<String>,
    next_presentation_token: u64,
}

struct OpenedHostDevice {
    path: PathBuf,
    node: DrmNode,
    fd: OwnedFd,
    claimed_pipeline: Option<ClaimedPresentationPipeline>,
    prepared_pipeline: Option<ClaimedPresentationPipeline>,
}

struct RetiredHostClaim {
    output: ClaimedHostOutput,
    pipeline: ClaimedPresentationPipeline,
}

struct ClaimedOutputBuffer {
    fb: drm_framebuffer::Handle,
    dumb: drm_dumbbuffer::DumbBuffer,
}

struct ClaimedPresentationPipeline {
    crtc: drm_crtc::Handle,
    dumb_buffers: Option<[ClaimedOutputBuffer; 2]>,
    dumb_front_buffer: usize,
    dumb_back_buffer: usize,
    atomic_commit_state: Option<AtomicCommitState>,
    pending_atomic_modeset: bool,
    flip_pending: bool,
    pending_flip_source: Option<QueuedFlipSource>,
    pending_presentation_token: Option<PresentationToken>,
    gles_renderer: Option<HostGlesRendererState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentationToken(u64);

struct HostGlesRendererState {
    render_node: DrmNode,
    _render_gbm_device: GbmDevice<DeviceFd>,
    _scanout_gbm_device: GbmDevice<DeviceFd>,
    _drm_device_fd: DrmDeviceFd,
    _egl_display: EGLDisplay,
    renderer: GlesRenderer,
    target_texture: GlesTexture,
    scanout_texture: GlesTexture,
    primary_scanout_format: DrmFourcc,
    overlay_scanout_format: Option<DrmFourcc>,
    direct_scanout: Option<HostDirectScanoutState>,
    overlay_scanout: Option<HostOverlayScanoutState>,
}

const GLES_INTERMEDIATE_RENDER_FORMAT: DrmFourcc = DrmFourcc::Xrgb8888;

struct HostDirectScanoutState {
    buffers: [HostDirectScanoutBuffer; 2],
    front_buffer: usize,
    back_buffer: usize,
}

struct HostDirectScanoutBuffer {
    _gbm_buffer: GbmBuffer,
    dmabuf: Dmabuf,
    framebuffer: GbmFramebuffer,
}

struct HostOverlayScanoutState {
    buffer: HostOverlayScanoutBuffer,
    size: Size<i32, BufferCoords>,
}

struct HostOverlayScanoutBuffer {
    _gbm_buffer: GbmBuffer,
    dmabuf: Dmabuf,
    framebuffer: GbmFramebuffer,
}

struct DirectRenderTargets {
    main: Option<drm_framebuffer::Handle>,
    overlay: Option<drm_framebuffer::Handle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueuedFlipSource {
    Dumb,
    DirectGbm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupPresentOwnership {
    DirectGbm,
    Dumb,
}

#[derive(Clone)]
struct ClaimedHostOutput {
    device_id: u64,
    mode: DrmMode,
    startup_present_ownership: StartupPresentOwnership,
    identity: OutputIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutputIdentity {
    device_path: PathBuf,
    connector_name: String,
    connector_id: u32,
}

#[derive(Clone)]
struct OutputClaimPlan {
    connector: drm_connector::Handle,
    connector_name: String,
    crtc: drm_crtc::Handle,
    mode: DrmMode,
    atomic: Option<AtomicClaimPlan>,
}

#[derive(Clone)]
struct OutputClaimCandidate {
    device_id: u64,
    device_path: PathBuf,
    plan: OutputClaimPlan,
}

#[derive(Clone)]
#[allow(dead_code)]
struct AtomicClaimPlan {
    connector: drm_connector::Handle,
    crtc: drm_crtc::Handle,
    mode: DrmMode,
    connector_crtc_id: drm_property::Handle,
    crtc_active: drm_property::Handle,
    crtc_mode_id: drm_property::Handle,
    primary_plane: AtomicPlaneState,
    overlay_plane: Option<AtomicPlaneState>,
}

#[derive(Clone)]
struct AtomicPlaneState {
    role: AtomicPlaneRole,
    handle: drm_plane::Handle,
    props: AtomicPlanePropertyHandles,
    scanout_format: DrmFourcc,
    zpos: Option<u64>,
    alpha: Option<u64>,
    pixel_blend_mode: Option<u64>,
    supports_alpha_blending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicPlaneRole {
    Primary,
    Overlay,
}

#[derive(Clone)]
struct AtomicPlanePropertyHandles {
    crtc_id: drm_property::Handle,
    fb_id: drm_property::Handle,
    src_x: drm_property::Handle,
    src_y: drm_property::Handle,
    src_w: drm_property::Handle,
    src_h: drm_property::Handle,
    crtc_x: drm_property::Handle,
    crtc_y: drm_property::Handle,
    crtc_w: drm_property::Handle,
    crtc_h: drm_property::Handle,
    zpos: Option<AtomicPlaneZposProperty>,
    alpha: Option<AtomicPlaneAlphaProperty>,
    pixel_blend_mode: Option<AtomicPlanePixelBlendModeProperty>,
}

#[derive(Clone)]
struct AtomicPlaneZposProperty {
    handle: drm_property::Handle,
    min: u64,
    max: u64,
}

#[derive(Clone)]
struct AtomicPlaneAlphaProperty {
    handle: drm_property::Handle,
    min: u64,
    max: u64,
}

#[derive(Clone)]
struct AtomicPlanePixelBlendModeProperty {
    handle: drm_property::Handle,
    premultiplied: Option<u64>,
    coverage: Option<u64>,
    none: Option<u64>,
}

#[derive(Clone)]
struct AtomicPlaneLayout {
    crtc_x: i32,
    crtc_y: i32,
    crtc_w: u32,
    crtc_h: u32,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
}

impl AtomicPlaneLayout {
    fn fullscreen(mode_size: (u16, u16)) -> Self {
        Self {
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: u32::from(mode_size.0),
            crtc_h: u32::from(mode_size.1),
            src_x: 0,
            src_y: 0,
            src_w: u32::from(mode_size.0),
            src_h: u32::from(mode_size.1),
        }
    }

    fn from_overlay_rect(rect: Rectangle<i32, Logical>) -> Option<Self> {
        let width = rect.size.w.max(0) as u32;
        let height = rect.size.h.max(0) as u32;
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            crtc_x: rect.loc.x.max(0),
            crtc_y: rect.loc.y.max(0),
            crtc_w: width,
            crtc_h: height,
            src_x: 0,
            src_y: 0,
            src_w: width,
            src_h: height,
        })
    }
}

struct AtomicCommitState {
    connector: drm_connector::Handle,
    crtc: drm_crtc::Handle,
    mode: DrmMode,
    mode_size: (u16, u16),
    connector_crtc_id: drm_property::Handle,
    crtc_active: drm_property::Handle,
    crtc_mode_id: drm_property::Handle,
    plane_states: Vec<AtomicPlaneState>,
    primary_scanout_format: DrmFourcc,
    overlay_scanout_format: Option<DrmFourcc>,
    overlay_alpha_blending_supported: bool,
}

const PRIMARY_SCANOUT_FORMAT_PREFERENCE: [DrmFourcc; 2] =
    [DrmFourcc::Xrgb8888, DrmFourcc::Argb8888];
const OVERLAY_SCANOUT_FORMAT_PREFERENCE: [DrmFourcc; 2] =
    [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

#[derive(Clone, Copy)]
struct AtomicPlaneCandidate {
    handle: drm_plane::Handle,
    scanout_format: DrmFourcc,
}

impl AtomicCommitState {
    fn from_plan(plan: &OutputClaimPlan) -> Option<Self> {
        let atomic = plan.atomic.as_ref()?;
        let mut states = Vec::new();
        states.push(atomic.primary_plane.clone());
        if let Some(overlay) = atomic.overlay_plane.as_ref() {
            states.push(overlay.clone());
        }
        Some(Self {
            connector: plan.connector,
            crtc: plan.crtc,
            mode: plan.mode,
            mode_size: plan.mode.size(),
            connector_crtc_id: atomic.connector_crtc_id,
            crtc_active: atomic.crtc_active,
            crtc_mode_id: atomic.crtc_mode_id,
            plane_states: states,
            primary_scanout_format: atomic.primary_plane.scanout_format,
            overlay_scanout_format: atomic.overlay_plane.as_ref().map(|p| p.scanout_format),
            overlay_alpha_blending_supported: atomic
                .overlay_plane
                .as_ref()
                .map(|p| p.supports_alpha_blending)
                .unwrap_or(false),
        })
    }
}

struct HostKmsCard<'a> {
    fd: &'a OwnedFd,
}

impl<'a> HostKmsCard<'a> {
    fn new(fd: &'a OwnedFd) -> Self {
        Self { fd }
    }
}

impl AsFd for HostKmsCard<'_> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl drm_api::Device for HostKmsCard<'_> {}

impl DrmControlDevice for HostKmsCard<'_> {}

impl HostBackendState {
    #[cfg(test)]
    fn for_root_geometry_test(screen_capture: ScreenCaptureStore) -> Self {
        Self {
            session: None,
            seat_name: "test-seat".to_string(),
            preferred_primary_path: None,
            forced_drm_path: None,
            forced_output_name: None,
            screen_capture,
            detected_devices: HashMap::new(),
            opened_devices: HashMap::new(),
            claimed_output: None,
            retired_claim: None,
            prepared_reclaim_output: None,
            last_good_output_identity: None,
            device_selection_state: RuntimeHostSelectionState::Automatic,
            output_selection_state: RuntimeHostSelectionState::Automatic,
            last_selection_attempt: None,
            last_selection_result: None,
            next_presentation_token: 0,
        }
    }

    fn mark_claim_lost(&mut self) {
        let Some(output) = self.claimed_output.take() else {
            return;
        };
        let pipeline = self
            .opened_devices
            .get_mut(&output.device_id)
            .and_then(|opened| opened.claimed_pipeline.take());
        self.retired_claim = pipeline.map(|pipeline| RetiredHostClaim { output, pipeline });
    }

    fn new(
        session: LibSeatSession,
        seat_name: String,
        preferred_primary_path: Option<PathBuf>,
        forced_drm_path: Option<PathBuf>,
        forced_output_name: Option<String>,
        screen_capture: ScreenCaptureStore,
    ) -> Self {
        let device_selection_state = if forced_drm_path.is_some() {
            RuntimeHostSelectionState::Forced
        } else {
            RuntimeHostSelectionState::Automatic
        };
        let output_selection_state = if forced_output_name.is_some() {
            RuntimeHostSelectionState::Forced
        } else {
            RuntimeHostSelectionState::Automatic
        };
        Self {
            session: Some(session),
            seat_name,
            preferred_primary_path,
            forced_drm_path,
            forced_output_name,
            screen_capture,
            detected_devices: HashMap::new(),
            opened_devices: HashMap::new(),
            claimed_output: None,
            retired_claim: None,
            prepared_reclaim_output: None,
            last_good_output_identity: None,
            device_selection_state,
            output_selection_state,
            last_selection_attempt: None,
            last_selection_result: None,
            next_presentation_token: 0,
        }
    }

    fn detected_count(&self) -> usize {
        self.detected_devices.len()
    }

    fn opened_count(&self) -> usize {
        self.opened_devices.len()
    }

    fn primary_opened_path(&self) -> Option<String> {
        select_primary_path(
            self.opened_devices.values().map(|device| &device.path),
            self.preferred_primary_path.as_deref(),
        )
    }

    fn path_for(&self, device_id: u64) -> Option<&PathBuf> {
        self.detected_devices.get(&device_id)
    }
    fn forced_drm_path_str(&self) -> Option<String> {
        self.forced_drm_path
            .as_ref()
            .map(|path| path.display().to_string())
    }

    fn forced_output_name(&self) -> Option<String> {
        self.forced_output_name.clone()
    }

    fn selection_states(&self) -> (RuntimeHostSelectionState, RuntimeHostSelectionState) {
        (self.device_selection_state, self.output_selection_state)
    }

    fn active_connector_status(&self) -> (Option<String>, Option<u32>) {
        match self.claimed_output.as_ref() {
            Some(claimed) => (
                Some(claimed.identity.connector_name.clone()),
                Some(claimed.identity.connector_id),
            ),
            None => (None, None),
        }
    }

    fn selection_logs(&self) -> (Option<String>, Option<String>) {
        (
            self.last_selection_attempt.clone(),
            self.last_selection_result.clone(),
        )
    }

    fn upsert_device(&mut self, device_id: u64, path: PathBuf) -> Result<(), RuntimeError> {
        self.detected_devices.insert(device_id, path.clone());
        if self.opened_devices.contains_key(&device_id) {
            self.close_device(device_id)?;
        }
        self.open_device(device_id, &path)
    }

    fn open_device(&mut self, device_id: u64, path: &Path) -> Result<(), RuntimeError> {
        let node = DrmNode::from_path(path).map_err(|err| RuntimeError::HostDeviceOpen {
            path: path.display().to_string(),
            error: err.to_string(),
        })?;
        let fd = self
            .session
            .as_mut()
            .expect("production host backend must own a libseat session")
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY)
            .map_err(|err| RuntimeError::HostDeviceOpen {
                path: path.display().to_string(),
                error: err.to_string(),
            })?;
        self.opened_devices.insert(
            device_id,
            OpenedHostDevice {
                path: path.to_path_buf(),
                node,
                fd,
                claimed_pipeline: None,
                prepared_pipeline: None,
            },
        );
        Ok(())
    }

    fn claim_output_ownership(
        &mut self,
        required_startup_ownership: Option<StartupPresentOwnership>,
        mut before_activation: Option<&mut dyn FnMut(u16, u16) -> Result<(), RuntimeError>>,
        defer_activation: bool,
    ) -> Result<ClaimedHostOutput, RuntimeError> {
        let recovering = self.last_good_output_identity.is_some();
        let forced_drm_path = self.forced_drm_path.clone();
        let forced_output_name = self.forced_output_name.clone();
        self.device_selection_state = if forced_drm_path.is_some() {
            RuntimeHostSelectionState::Forced
        } else {
            RuntimeHostSelectionState::Automatic
        };
        self.output_selection_state = if forced_output_name.is_some() {
            RuntimeHostSelectionState::Forced
        } else {
            RuntimeHostSelectionState::Automatic
        };
        self.last_selection_attempt = Some(describe_output_selection_attempt(
            forced_drm_path.as_deref(),
            forced_output_name.as_deref(),
            self.last_good_output_identity.as_ref(),
            recovering,
        ));
        self.last_selection_result = None;

        let mut device_ids: Vec<u64> = self.opened_devices.keys().copied().collect();
        let mut last_error: Option<RuntimeError> = None;
        device_ids.sort_by(|left, right| {
            let left_path = &self.opened_devices.get(left).expect("device exists").path;
            let right_path = &self.opened_devices.get(right).expect("device exists").path;
            host_device_sort_key(left_path, self.preferred_primary_path.as_deref()).cmp(
                &host_device_sort_key(right_path, self.preferred_primary_path.as_deref()),
            )
        });

        let mut candidates = Vec::new();
        for device_id in device_ids {
            let Some(opened) = self.opened_devices.get(&device_id) else {
                continue;
            };
            if forced_drm_path
                .as_ref()
                .is_some_and(|forced_path| forced_path != &opened.path)
            {
                continue;
            }
            match build_output_claim_plans(opened, forced_output_name.as_deref()) {
                Ok(plans) => {
                    for plan in plans {
                        candidates.push(OutputClaimCandidate {
                            device_id,
                            device_path: opened.path.clone(),
                            plan,
                        });
                    }
                }
                Err(err) => {
                    eprintln!(
                        "host backend failed to inspect output claim plan on {}: {err}",
                        opened.path.display()
                    );
                    last_error = Some(err);
                }
            }
        }

        if let Some(forced_path) = forced_drm_path.as_ref() {
            if candidates.is_empty() {
                self.device_selection_state = RuntimeHostSelectionState::ForcedFailed;
                let error = RuntimeError::HostOutputClaim {
                    path: forced_path.display().to_string(),
                    error: "forced device override rejected: no connected output route".to_string(),
                };
                self.last_selection_result = Some(format!(
                    "forced device override {} rejected: no connected output route",
                    forced_path.display()
                ));
                return Err(error);
            }
        }

        if let Some(forced_output) = forced_output_name.as_ref() {
            candidates.retain(|candidate| candidate.plan.connector_name == *forced_output);
            if candidates.is_empty() {
                self.output_selection_state = RuntimeHostSelectionState::ForcedFailed;
                let error = RuntimeError::HostOutputClaim {
                    path: forced_drm_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .or_else(|| self.primary_opened_path())
                        .unwrap_or_else(|| "<unknown-device>".to_string()),
                    error: format!(
                        "forced output override {} rejected: connector not present",
                        forced_output
                    ),
                };
                self.last_selection_result = Some(format!(
                    "forced output override {} rejected: connector not present",
                    forced_output
                ));
                return Err(error);
            }
        }

        let chosen = if let Some(last_good) = self.last_good_output_identity.as_ref() {
            if let Some(index) = candidates.iter().position(|candidate| {
                candidate.device_path == last_good.device_path
                    && candidate.plan.connector_name == last_good.connector_name
            }) {
                candidates.remove(index)
            } else if candidates.len() == 1 {
                candidates.remove(0)
            } else {
                let error = RuntimeError::HostOutputClaim {
                    path: last_good.device_path.display().to_string(),
                    error: "previous output disappeared and no single safe replacement was found"
                        .to_string(),
                };
                self.last_selection_result = Some(
                    "recovery required: previous output disappeared and no single safe replacement was found"
                        .to_string(),
                );
                return Err(error);
            }
        } else {
            let Some(first) = candidates.into_iter().next() else {
                return match last_error {
                    Some(err) => Err(err),
                    None => Err(RuntimeError::HostNoConnectedOutputRoute),
                };
            };
            first
        };

        let Some(opened) = self.opened_devices.get_mut(&chosen.device_id) else {
            return Err(RuntimeError::HostNoConnectedOutputRoute);
        };
        if !defer_activation && let Some(before_activation) = before_activation.as_mut() {
            let (width, height) = chosen.plan.mode.size();
            before_activation(width, height)?;
        }
        let previous_identity = self.last_good_output_identity.clone();
        match claim_output_on_device(
            opened,
            chosen.plan.clone(),
            required_startup_ownership,
            defer_activation,
        ) {
            Ok(claimed) => {
                if defer_activation && let Err(err) = drain_prior_pipeline_before_reclaim(opened) {
                    if let Some(pipeline) = opened.prepared_pipeline.take() {
                        destroy_claimed_pipeline_resources(&opened.fd, pipeline);
                    }
                    return Err(err);
                }
                if defer_activation && let Some(before_activation) = before_activation.as_mut() {
                    let (width, height) = claimed.mode.size();
                    if let Err(err) = before_activation(width, height) {
                        if let Some(pipeline) = opened.prepared_pipeline.take() {
                            destroy_claimed_pipeline_resources(&opened.fd, pipeline);
                        }
                        return Err(err);
                    }
                }
                let claimed_output = ClaimedHostOutput {
                    device_id: chosen.device_id,
                    mode: claimed.mode,
                    startup_present_ownership: claimed.startup_present_ownership,
                    identity: claimed.identity.clone(),
                };
                self.last_good_output_identity = Some(claimed.identity.clone());
                self.last_selection_result = Some(describe_output_selection_result(
                    recovering,
                    previous_identity.as_ref(),
                    &claimed.identity,
                    forced_drm_path.as_deref(),
                    forced_output_name.as_deref(),
                ));
                if defer_activation {
                    self.prepared_reclaim_output = Some(claimed_output.clone());
                } else {
                    self.claimed_output = Some(claimed_output.clone());
                }
                Ok(claimed_output)
            }
            Err(err) => {
                eprintln!(
                    "host backend failed to claim output ownership on {}: {err}",
                    opened.path.display()
                );
                self.last_selection_result = Some(format!(
                    "failed to claim output on {}:{}: {err}",
                    opened.path.display(),
                    chosen.plan.connector_name
                ));
                Err(err)
            }
        }
    }

    fn claimed_device_event_fd(&self) -> Option<OwnedFd> {
        let claimed = self
            .claimed_output
            .as_ref()
            .or(self.prepared_reclaim_output.as_ref())?;
        let opened = self.opened_devices.get(&claimed.device_id)?;
        dup(opened.fd.as_fd()).ok()
    }

    fn arm_prepared_reclaim_for_presentation(&mut self) -> Result<(), RuntimeError> {
        let prepared =
            self.prepared_reclaim_output
                .take()
                .ok_or_else(|| RuntimeError::HostOutputClaim {
                    path: self
                        .primary_opened_path()
                        .unwrap_or_else(|| "<selected-root4-device>".to_string()),
                    error: "mode mutation reached the FIFO head without a prepared reclaim"
                        .to_string(),
                })?;
        let opened = self
            .opened_devices
            .get_mut(&prepared.device_id)
            .ok_or_else(|| RuntimeError::HostOutputClaim {
                path: prepared.identity.device_path.display().to_string(),
                error: "prepared reclaim device disappeared before FIFO activation".to_string(),
            })?;
        let pipeline =
            opened
                .prepared_pipeline
                .take()
                .ok_or_else(|| RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error: "prepared reclaim pipeline disappeared before FIFO activation"
                        .to_string(),
                })?;
        opened.claimed_pipeline = Some(pipeline);
        self.claimed_output = Some(prepared);
        Ok(())
    }

    fn discard_unactivated_reclaim(&mut self) {
        if let Some(prepared) = self.prepared_reclaim_output.take() {
            if let Some(opened) = self.opened_devices.get_mut(&prepared.device_id) {
                if let Some(pipeline) = opened.prepared_pipeline.take() {
                    destroy_claimed_pipeline_resources(&opened.fd, pipeline);
                }
            }
            return;
        }
        let Some(failed) = self.claimed_output.take() else {
            return;
        };
        let Some(opened) = self.opened_devices.get_mut(&failed.device_id) else {
            if let Some(retired) = self.retired_claim.take() {
                if let Some(retired_opened) = self.opened_devices.get_mut(&retired.output.device_id)
                {
                    retired_opened.claimed_pipeline = Some(retired.pipeline);
                }
                self.claimed_output = Some(retired.output);
            }
            return;
        };
        let can_restore = opened
            .claimed_pipeline
            .as_ref()
            .is_some_and(|pipeline| pipeline.pending_atomic_modeset);
        if can_restore {
            let failed_pipeline = opened.claimed_pipeline.take();
            if let Some(failed_pipeline) = failed_pipeline {
                destroy_claimed_pipeline_resources(&opened.fd, failed_pipeline);
            }
            if let Some(retired) = self.retired_claim.take() {
                if let Some(retired_opened) = self.opened_devices.get_mut(&retired.output.device_id)
                {
                    retired_opened.claimed_pipeline = Some(retired.pipeline);
                }
                self.claimed_output = Some(retired.output);
            }
        } else {
            self.claimed_output = Some(failed);
        }
    }

    fn finish_reclaim_activation(&mut self) {
        let Some(retired) = self.retired_claim.take() else {
            return;
        };
        let Some(opened) = self.opened_devices.get_mut(&retired.output.device_id) else {
            return;
        };
        destroy_claimed_pipeline_resources(&opened.fd, retired.pipeline);
    }

    fn claimed_dmabuf_protocol_advertisement(&self) -> Option<(DrmNode, Vec<Format>)> {
        let claimed = self.claimed_output.as_ref()?;
        let opened = self.opened_devices.get(&claimed.device_id)?;
        let pipeline = opened.claimed_pipeline.as_ref()?;
        let gles = pipeline.gles_renderer.as_ref()?;
        Some((
            gles.render_node,
            gles.renderer.dmabuf_formats().iter().copied().collect(),
        ))
    }

    fn claimed_present_capabilities(&self) -> Option<(RuntimeHostPresentOwnership, bool, bool)> {
        let claimed = self.claimed_output.as_ref()?;
        self.present_capabilities_for(claimed)
    }

    fn present_capabilities_for(
        &self,
        claimed: &ClaimedHostOutput,
    ) -> Option<(RuntimeHostPresentOwnership, bool, bool)> {
        let opened = self.opened_devices.get(&claimed.device_id)?;
        let pipeline = opened.claimed_pipeline.as_ref()?;
        let ownership = match claimed.startup_present_ownership {
            StartupPresentOwnership::DirectGbm => RuntimeHostPresentOwnership::DirectGbm,
            StartupPresentOwnership::Dumb => RuntimeHostPresentOwnership::Dumb,
        };
        let atomic_enabled = pipeline.atomic_commit_state.is_some();
        let overlay_capable = pipeline
            .atomic_commit_state
            .as_ref()
            .and_then(|atomic| atomic.overlay_scanout_format)
            .map(overlay_scanout_format_supports_alpha)
            .unwrap_or(false)
            && pipeline
                .atomic_commit_state
                .as_ref()
                .map(|atomic| atomic.overlay_alpha_blending_supported)
                .unwrap_or(false);
        Some((ownership, atomic_enabled, overlay_capable))
    }

    fn queue_claimed_presentation_tick(
        &mut self,
        wayland_state: &mut RuntimeWaylandState,
    ) -> Result<Option<PresentationToken>, HostPresentFailure> {
        wayland_state.sync_output_rotation_reconfigure_if_needed();
        sync_runtime_host_present_capabilities(&wayland_state.shared_state, self);
        let Some(claimed) = self.claimed_output.as_ref().cloned() else {
            if wayland_state.staged_root_geometry.is_some() {
                return Err(HostPresentFailure::transaction(
                    RuntimeError::HostOutputClaim {
                        path: self
                            .primary_opened_path()
                            .unwrap_or_else(|| "<selected-root4-device>".to_string()),
                        error:
                            "root4 geometry head cannot present before the observed mode mutation"
                                .to_string(),
                    },
                ));
            }
            return Ok(None);
        };
        let Some(opened) = self.opened_devices.get_mut(&claimed.device_id) else {
            return Ok(None);
        };
        let Some(pipeline) = opened.claimed_pipeline.as_mut() else {
            return Ok(None);
        };
        if pipeline.flip_pending {
            return Ok(None);
        }
        let rotation = wayland_state
            .root_geometry_snapshot()
            .map(|snapshot| snapshot.rotation)
            .unwrap_or_else(|| lock_state(&wayland_state.shared_state).output_rotation());
        let direct_present_supported = direct_present_supported_for_rotation(rotation);
        let requires_direct_present = matches!(
            claimed.startup_present_ownership,
            StartupPresentOwnership::DirectGbm
        ) && direct_present_supported;

        let card = HostKmsCard::new(&opened.fd);
        let (mode_w, mode_h) = claimed.mode.size();
        let mut queued_framebuffer = pipeline
            .dumb_buffers
            .as_ref()
            .map(|buffers| buffers[pipeline.dumb_back_buffer].fb);
        let mut queued_source = if queued_framebuffer.is_some() {
            QueuedFlipSource::Dumb
        } else {
            QueuedFlipSource::DirectGbm
        };
        let mut rendered_with_gles_direct = false;
        let mut rendered_with_gles_readback = false;
        let mut disable_gles_renderer = false;
        let mut overlay_framebuffer: Option<drm_framebuffer::Handle> = None;
        let force_readback_present =
            std::env::var_os("SURF_ACE_HOST_RUNTIME_FORCE_READBACK").is_some();
        let overlay_plane_rotation_supported = matches!(
            wayland_state
                .root_geometry_snapshot()
                .map(|snapshot| snapshot.rotation)
                .unwrap_or_else(|| lock_state(&wayland_state.shared_state).output_rotation()),
            OutputRotation::Deg0
        );
        let overlay_plane_alpha_format_supported = pipeline
            .atomic_commit_state
            .as_ref()
            .and_then(|atomic| atomic.overlay_scanout_format)
            .map(overlay_scanout_format_supports_alpha)
            .unwrap_or(false);
        let overlay_plane_alpha_blending_supported = pipeline
            .atomic_commit_state
            .as_ref()
            .map(|atomic| atomic.overlay_alpha_blending_supported)
            .unwrap_or(false);
        let prefer_overlay_plane_split = overlay_plane_rotation_supported
            && wayland_state.root_display_scale() == 1.0
            && overlay_plane_alpha_format_supported
            && overlay_plane_alpha_blending_supported;
        if !force_readback_present && direct_present_supported {
            if let Some(gles_renderer) = pipeline.gles_renderer.as_mut() {
                match render_host_scene_with_gles_direct(
                    gles_renderer,
                    wayland_state,
                    &opened.path,
                    mode_w as i32,
                    mode_h as i32,
                    prefer_overlay_plane_split,
                    &self.screen_capture,
                ) {
                    Ok(targets) => {
                        if let Some(framebuffer) = targets.main {
                            queued_framebuffer = Some(framebuffer);
                            queued_source = QueuedFlipSource::DirectGbm;
                            rendered_with_gles_direct = true;
                        } else if requires_direct_present {
                            return Err(RuntimeError::HostOutputClaim {
                                path: opened.path.display().to_string(),
                                error: "direct-present ownership was established at startup but direct framebuffer is unavailable"
                                    .to_string(),
                            }
                            .into());
                        }
                        if overlay_framebuffer.is_none() {
                            overlay_framebuffer = targets.overlay;
                        }
                    }
                    Err(err) => {
                        if requires_direct_present {
                            return Err(RuntimeError::HostOutputClaim {
                                path: opened.path.display().to_string(),
                                error: format!(
                                    "direct-present ownership is active but direct scanout render failed: {err}"
                                ),
                            }
                            .into());
                        } else {
                            eprintln!(
                                "host gles direct scanout render failed on {}: {err}; falling back to readback/software composition",
                                opened.path.display()
                            );
                            if let Some(gles) = pipeline.gles_renderer.as_mut() {
                                gles.direct_scanout = None;
                            }
                        }
                    }
                }
            }
        }

        if !rendered_with_gles_direct {
            if requires_direct_present {
                return Err(RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error:
                        "direct-present startup ownership is active but no direct frame was queued"
                            .to_string(),
                }
                .into());
            }
            ensure_dumb_fallback_buffers(pipeline, &card, &opened.path, claimed.mode.size())?;
            let dumb_back_index = pipeline.dumb_back_buffer;
            let dumb_fb = pipeline
                .dumb_buffers
                .as_ref()
                .map(|buffers| buffers[dumb_back_index].fb)
                .ok_or_else(|| RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error: "dumb fallback buffers unavailable after allocation".to_string(),
                })?;
            queued_framebuffer = Some(dumb_fb);
            queued_source = QueuedFlipSource::Dumb;

            let (dumb_buffers_opt, gles_renderer_opt) =
                (&mut pipeline.dumb_buffers, &mut pipeline.gles_renderer);
            let dumb_buffers =
                dumb_buffers_opt
                    .as_mut()
                    .ok_or_else(|| RuntimeError::HostOutputClaim {
                        path: opened.path.display().to_string(),
                        error: "dumb fallback buffers missing".to_string(),
                    })?;
            let back_buffer = &mut dumb_buffers[dumb_back_index].dumb;
            let stride = back_buffer.pitch() as usize;
            let mut mapping =
                card.map_dumb_buffer(back_buffer)
                    .map_err(|err| RuntimeError::HostOutputClaim {
                        path: opened.path.display().to_string(),
                        error: format!("failed to map dumb buffer for scene render: {err}"),
                    })?;

            if let Some(gles_renderer) = gles_renderer_opt.as_mut() {
                match render_host_scene_with_gles_readback(
                    gles_renderer,
                    wayland_state,
                    &opened.path,
                    &self.screen_capture,
                    &mut mapping,
                    stride,
                    mode_w as i32,
                    mode_h as i32,
                ) {
                    Ok(()) => {
                        rendered_with_gles_readback = true;
                    }
                    Err(err) => {
                        eprintln!(
                            "host gles scene composition failed on {}: {err}; falling back to wl_shm software composition",
                            opened.path.display()
                        );
                        disable_gles_renderer = true;
                    }
                }
            }
            if disable_gles_renderer {
                if wayland_state.has_dmabuf_surface_material() {
                    return Err(HostPresentFailure::transaction(
                        RuntimeError::HostOutputClaim {
                            path: opened.path.display().to_string(),
                            error: "dmabuf scene cannot enter the wl_shm software fallback"
                                .to_string(),
                        },
                    ));
                }
                pipeline.gles_renderer = None;
            }
            if !rendered_with_gles_readback {
                if wayland_state.has_dmabuf_surface_material() {
                    return Err(HostPresentFailure::transaction(
                        RuntimeError::HostOutputClaim {
                            path: opened.path.display().to_string(),
                            error: "dmabuf scene cannot enter the wl_shm software fallback"
                                .to_string(),
                        },
                    ));
                }
                let _ = wayland_state.compose_host_scene(
                    &mut mapping,
                    stride,
                    mode_w as i32,
                    mode_h as i32,
                );
            }
            let quarter_turn_gles_capture_already_recorded =
                rendered_with_gles_readback && OutputRotationModel::new(rotation).swaps_axes();
            if !quarter_turn_gles_capture_already_recorded
                && let Some(snapshot) = wayland_state.root_capture_snapshot()
            {
                self.screen_capture.update_root4_scanout_xrgb8888(
                    &mapping[..],
                    stride,
                    mode_w.max(1) as usize,
                    mode_h.max(1) as usize,
                    false,
                    snapshot,
                    &wayland_state.root_pane_capture_geometries(),
                );
            }
        }

        if let Some(staged) = wayland_state.staged_root_geometry.as_ref()
            && !self
                .screen_capture
                .root4_stage_has_frame(staged.committed.snapshot.generation)
        {
            return Err(HostPresentFailure::transaction(
                RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error: "root4 capture consumer did not stage the presentation frame"
                        .to_string(),
                },
            ));
        }

        let queued_framebuffer =
            queued_framebuffer.ok_or_else(|| RuntimeError::HostOutputClaim {
                path: opened.path.display().to_string(),
                error: "no framebuffer available for page flip".to_string(),
            })?;
        let uses_atomic_commit = pipeline.atomic_commit_state.is_some();
        let queued_overlay_plane = if uses_atomic_commit {
            overlay_plane_layout_for_frame(wayland_state, overlay_framebuffer).is_some()
        } else {
            false
        };
        let queued_primary_dmabuf_format = match queued_source {
            QueuedFlipSource::Dumb => None,
            QueuedFlipSource::DirectGbm => pipeline.gles_renderer.as_ref().and_then(|gles| {
                let direct_scanout = gles.direct_scanout.as_ref()?;
                Some(runtime_dmabuf_format_status(
                    direct_scanout.buffers[direct_scanout.back_buffer]
                        .dmabuf
                        .format(),
                ))
            }),
        };
        let queued_overlay_dmabuf_format = if queued_overlay_plane {
            pipeline.gles_renderer.as_ref().and_then(|gles| {
                let overlay_scanout = gles.overlay_scanout.as_ref()?;
                Some(runtime_dmabuf_format_status(
                    overlay_scanout.buffer.dmabuf.format(),
                ))
            })
        } else {
            None
        };
        if let Some(atomic) = pipeline.atomic_commit_state.as_ref() {
            let commit = if pipeline.pending_atomic_modeset {
                queue_atomic_modeset_frame_commit(
                    &card,
                    &opened.path,
                    atomic,
                    queued_framebuffer,
                    overlay_framebuffer,
                    wayland_state,
                )
            } else {
                queue_atomic_frame_commit(
                    &card,
                    &opened.path,
                    atomic,
                    Some(queued_framebuffer),
                    overlay_framebuffer,
                    wayland_state,
                )
            };
            if let Err(err) = commit {
                return Err(HostPresentFailure::reclaimable(err));
            }
            pipeline.pending_atomic_modeset = false;
        } else {
            if let Err(err) = card.page_flip(
                pipeline.crtc,
                queued_framebuffer,
                PageFlipFlags::EVENT,
                None,
            ) {
                let failure = RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error: format!("failed to queue page flip: {err}"),
                };
                return Err(HostPresentFailure::reclaimable(failure));
            }
        }
        let present_source = match queued_source {
            QueuedFlipSource::Dumb => RuntimeHostQueuedPresentSource::Dumb,
            QueuedFlipSource::DirectGbm => RuntimeHostQueuedPresentSource::DirectGbm,
        };
        {
            let mut state = lock_state(&wayland_state.shared_state);
            state.set_runtime_last_queued_present(
                present_source,
                uses_atomic_commit,
                queued_overlay_plane,
                queued_primary_dmabuf_format,
                queued_overlay_dmabuf_format,
            );
        }
        self.next_presentation_token = self.next_presentation_token.saturating_add(1);
        let token = PresentationToken(self.next_presentation_token);
        pipeline.flip_pending = true;
        pipeline.pending_flip_source = Some(queued_source);
        pipeline.pending_presentation_token = Some(token);
        Ok(Some(token))
    }

    fn process_claimed_presentation_events(
        &mut self,
    ) -> Result<Vec<PresentationToken>, HostPresentFailure> {
        let Some(claimed) = self.claimed_output.as_ref().cloned() else {
            return Ok(Vec::new());
        };
        let Some(opened) = self.opened_devices.get_mut(&claimed.device_id) else {
            return Ok(Vec::new());
        };
        let Some(pipeline) = opened.claimed_pipeline.as_mut() else {
            return Ok(Vec::new());
        };

        let card = HostKmsCard::new(&opened.fd);
        let events = card.receive_events().map_err(|err| {
            HostPresentFailure::reclaimable(RuntimeError::HostOutputClaim {
                path: opened.path.display().to_string(),
                error: format!("failed to receive drm events: {err}"),
            })
        })?;

        let mut completed = Vec::new();
        for event in events {
            if let drm_api::control::Event::PageFlip(flip) = event {
                if flip.crtc == pipeline.crtc && pipeline.flip_pending {
                    if let Some(token) = complete_pipeline_flip(pipeline) {
                        completed.push(token);
                    }
                }
            }
        }

        Ok(completed)
    }

    fn close_device(&mut self, device_id: u64) -> Result<(), RuntimeError> {
        if self
            .claimed_output
            .as_ref()
            .map(|claimed| claimed.device_id == device_id)
            .unwrap_or(false)
        {
            self.claimed_output = None;
        }
        if self
            .prepared_reclaim_output
            .as_ref()
            .is_some_and(|prepared| prepared.device_id == device_id)
        {
            self.prepared_reclaim_output = None;
        }
        let retired_pipeline = if self
            .retired_claim
            .as_ref()
            .is_some_and(|retired| retired.output.device_id == device_id)
        {
            self.retired_claim.take().map(|retired| retired.pipeline)
        } else {
            None
        };
        let Some(opened) = self.opened_devices.remove(&device_id) else {
            return Ok(());
        };
        for pipeline in [
            opened.claimed_pipeline,
            opened.prepared_pipeline,
            retired_pipeline,
        ]
        .into_iter()
        .flatten()
        {
            destroy_claimed_pipeline_resources(&opened.fd, pipeline);
        }
        if let Some(session) = self.session.as_mut() {
            session
                .close(opened.fd)
                .map_err(|err| RuntimeError::HostDeviceClose {
                    path: opened.path.display().to_string(),
                    error: err.to_string(),
                })?;
        }
        Ok(())
    }

    fn remove_device(&mut self, device_id: u64) -> Result<(), RuntimeError> {
        self.detected_devices.remove(&device_id);
        self.close_device(device_id)
    }
}

fn destroy_claimed_pipeline_resources(fd: &OwnedFd, pipeline: ClaimedPresentationPipeline) {
    let card = HostKmsCard::new(fd);
    if let Some(dumb_buffers) = pipeline.dumb_buffers {
        for buffer in dumb_buffers {
            let _ = card.destroy_framebuffer(buffer.fb);
            let _ = card.destroy_dumb_buffer(buffer.dumb);
        }
    }
}

fn complete_pipeline_flip(pipeline: &mut ClaimedPresentationPipeline) -> Option<PresentationToken> {
    match pipeline.pending_flip_source {
        Some(QueuedFlipSource::Dumb) => {
            std::mem::swap(
                &mut pipeline.dumb_front_buffer,
                &mut pipeline.dumb_back_buffer,
            );
        }
        Some(QueuedFlipSource::DirectGbm) => {
            if let Some(gles_renderer) = pipeline.gles_renderer.as_mut()
                && let Some(direct_scanout) = gles_renderer.direct_scanout.as_mut()
            {
                std::mem::swap(
                    &mut direct_scanout.front_buffer,
                    &mut direct_scanout.back_buffer,
                );
            }
        }
        None => {}
    }
    pipeline.flip_pending = false;
    pipeline.pending_flip_source = None;
    pipeline.pending_presentation_token.take()
}

fn drain_prior_pipeline_before_reclaim(opened: &mut OpenedHostDevice) -> Result<(), RuntimeError> {
    let Some(pipeline) = opened.claimed_pipeline.as_mut() else {
        return Ok(());
    };
    if !pipeline.flip_pending {
        return Ok(());
    }
    let card = HostKmsCard::new(&opened.fd);
    let events = card
        .receive_events()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: opened.path.display().to_string(),
            error: format!("failed to drain prior presentation before reclaim: {err}"),
        })?;
    if events.into_iter().any(|event| {
        matches!(event, drm_api::control::Event::PageFlip(flip) if flip.crtc == pipeline.crtc)
    }) {
        complete_pipeline_flip(pipeline);
    }
    if pipeline.flip_pending {
        return Err(RuntimeError::HostOutputClaim {
            path: opened.path.display().to_string(),
            error: "prior presentation is still pending before reclaim activation".to_string(),
        });
    }
    Ok(())
}

impl Drop for HostBackendState {
    fn drop(&mut self) {
        let device_ids: Vec<u64> = self.opened_devices.keys().copied().collect();
        for device_id in device_ids {
            let _ = self.close_device(device_id);
        }
    }
}

struct ClaimedOutput {
    mode: DrmMode,
    startup_present_ownership: StartupPresentOwnership,
    identity: OutputIdentity,
}

fn connector_name(connector_info: &drm_api::control::connector::Info) -> String {
    format!(
        "{}-{}",
        connector_info.interface().as_str(),
        connector_info.interface_id()
    )
}

fn build_output_claim_plans(
    opened: &OpenedHostDevice,
    selected_output_name: Option<&str>,
) -> Result<Vec<OutputClaimPlan>, RuntimeError> {
    let card = HostKmsCard::new(&opened.fd);
    let _ = card.set_client_capability(ClientCapability::UniversalPlanes, true);
    let atomic_client_enabled = card
        .set_client_capability(ClientCapability::Atomic, true)
        .is_ok();
    let resources = card
        .resource_handles()
        .map_err(|err| RuntimeError::HostOutputInspect {
            path: opened.path.display().to_string(),
            error: err.to_string(),
        })?;

    let mut connector_infos = Vec::new();
    for connector in resources.connectors() {
        let info = card.get_connector(*connector, true).map_err(|err| {
            RuntimeError::HostOutputInspect {
                path: opened.path.display().to_string(),
                error: err.to_string(),
            }
        })?;
        if selected_output_name.is_some_and(|selected| connector_name(&info) != selected) {
            continue;
        }
        if info.state() == drm_connector::State::Connected && !info.modes().is_empty() {
            connector_infos.push(info);
        }
    }

    connector_infos.sort_by_key(|info| {
        (
            info.interface().as_str().to_string(),
            info.interface_id(),
            u32::from(info.handle()),
        )
    });

    let mut plans = Vec::new();
    for connector_info in connector_infos {
        let Some(mode) = select_connector_mode(connector_info.modes()) else {
            continue;
        };
        let mut encoder_candidates: Vec<drm_encoder::Handle> = Vec::new();
        if let Some(current) = connector_info.current_encoder() {
            encoder_candidates.push(current);
        }
        let mut additional_encoders: Vec<drm_encoder::Handle> =
            connector_info.encoders().iter().copied().collect();
        additional_encoders.sort_by_key(|encoder| u32::from(*encoder));
        for encoder in additional_encoders {
            if !encoder_candidates.iter().any(|item| *item == encoder) {
                encoder_candidates.push(encoder);
            }
        }

        for encoder in encoder_candidates {
            let encoder_info =
                card.get_encoder(encoder)
                    .map_err(|err| RuntimeError::HostOutputInspect {
                        path: opened.path.display().to_string(),
                        error: err.to_string(),
                    })?;
            let mut crtcs = resources.filter_crtcs(encoder_info.possible_crtcs());
            crtcs.sort_by_key(|crtc| u32::from(*crtc));
            let selected_crtc = if let Some(current_crtc) = encoder_info.crtc() {
                if crtcs.contains(&current_crtc) {
                    Some(current_crtc)
                } else {
                    crtcs.first().copied()
                }
            } else {
                crtcs.first().copied()
            };
            let Some(crtc) = selected_crtc else {
                continue;
            };
            let atomic = if atomic_client_enabled {
                match build_atomic_claim_plan(
                    &card,
                    &opened.path,
                    &resources,
                    connector_info.handle(),
                    crtc,
                    mode,
                ) {
                    Ok(plan) => plan,
                    Err(err) => {
                        eprintln!(
                            "host backend atomic claim plan probe failed on {}: {err}; continuing with legacy claim flow",
                            opened.path.display()
                        );
                        None
                    }
                }
            } else {
                None
            };
            plans.push(OutputClaimPlan {
                connector: connector_info.handle(),
                connector_name: connector_name(&connector_info),
                crtc,
                mode,
                atomic,
            });
            break;
        }
    }

    Ok(plans)
}

fn describe_output_selection_attempt(
    forced_drm_path: Option<&Path>,
    forced_output_name: Option<&str>,
    previous_identity: Option<&OutputIdentity>,
    recovering: bool,
) -> String {
    let mut parts = Vec::new();
    parts.push(if recovering {
        "recovery output selection".to_string()
    } else {
        "startup output selection".to_string()
    });
    parts.push(match forced_drm_path {
        Some(path) => format!("device=forced:{}", path.display()),
        None => "device=automatic".to_string(),
    });
    parts.push(match forced_output_name {
        Some(name) => format!("output=forced:{name}"),
        None => "output=automatic".to_string(),
    });
    if let Some(previous) = previous_identity {
        parts.push(format!(
            "previous={}:{}",
            previous.device_path.display(),
            previous.connector_name
        ));
    }
    parts.join(" ")
}

fn describe_output_selection_result(
    recovering: bool,
    previous_identity: Option<&OutputIdentity>,
    selected_identity: &OutputIdentity,
    forced_drm_path: Option<&Path>,
    forced_output_name: Option<&str>,
) -> String {
    if recovering {
        if let Some(previous) = previous_identity {
            if previous.device_path != selected_identity.device_path
                || previous.connector_name != selected_identity.connector_name
            {
                return format!(
                    "active connector {} disappeared, rebound to {} using matching single-output policy",
                    previous.connector_name, selected_identity.connector_name
                );
            }
        }
    }
    let selection_kind = if forced_drm_path.is_some() || forced_output_name.is_some() {
        "forced"
    } else {
        "auto-selected"
    };
    format!(
        "{} device={} output={}",
        selection_kind,
        selected_identity.device_path.display(),
        selected_identity.connector_name
    )
}

fn build_atomic_claim_plan(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    resources: &drm_api::control::ResourceHandles,
    connector: drm_connector::Handle,
    crtc: drm_crtc::Handle,
    mode: DrmMode,
) -> Result<Option<AtomicClaimPlan>, RuntimeError> {
    let connector_props = property_handle_map_for_resource(card, device_path, connector)?;
    let crtc_props = property_handle_map_for_resource(card, device_path, crtc)?;
    let Some(connector_crtc_id) = connector_props.get("CRTC_ID").copied() else {
        return Ok(None);
    };
    let Some(crtc_active) = crtc_props.get("ACTIVE").copied() else {
        return Ok(None);
    };
    let Some(crtc_mode_id) = crtc_props.get("MODE_ID").copied() else {
        return Ok(None);
    };

    let primary_plane =
        select_atomic_plane(card, device_path, resources, crtc, PlaneSelection::Primary)?;
    let primary_plane = match primary_plane {
        Some(handle) => handle,
        None => return Ok(None),
    };
    let primary_props = match plane_property_handles(card, device_path, primary_plane.handle)? {
        Some(props) => props,
        None => return Ok(None),
    };
    let mut primary_state = AtomicPlaneState {
        role: AtomicPlaneRole::Primary,
        handle: primary_plane.handle,
        props: primary_props,
        scanout_format: primary_plane.scanout_format,
        zpos: None,
        alpha: None,
        pixel_blend_mode: None,
        supports_alpha_blending: true,
    };
    configure_atomic_plane_composition_controls(device_path, &mut primary_state);

    let mut overlay_state = if let Some(overlay_plane) =
        select_atomic_plane(card, device_path, resources, crtc, PlaneSelection::Overlay)?
    {
        plane_property_handles(card, device_path, overlay_plane.handle)?.map(|props| {
            let mut state = AtomicPlaneState {
                role: AtomicPlaneRole::Overlay,
                handle: overlay_plane.handle,
                props,
                scanout_format: overlay_plane.scanout_format,
                zpos: None,
                alpha: None,
                pixel_blend_mode: None,
                supports_alpha_blending: false,
            };
            configure_atomic_plane_composition_controls(device_path, &mut state);
            state
        })
    } else {
        None
    };

    if overlay_state
        .as_ref()
        .map(|overlay| !overlay.supports_alpha_blending)
        .unwrap_or(false)
    {
        eprintln!(
            "host backend overlay plane on {} lacks alpha-safe blending controls; disabling overlay plane routing for this output",
            device_path.display()
        );
        overlay_state = None;
    }

    if let Some(overlay) = overlay_state.as_mut() {
        assign_atomic_plane_zpos(device_path, &mut primary_state, overlay);
    }

    Ok(Some(AtomicClaimPlan {
        connector,
        crtc,
        mode,
        connector_crtc_id,
        crtc_active,
        crtc_mode_id,
        primary_plane: primary_state,
        overlay_plane: overlay_state,
    }))
}

fn property_handle_map_for_resource<T: drm_api::control::ResourceHandle>(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    handle: T,
) -> Result<HashMap<String, drm_property::Handle>, RuntimeError> {
    let values = card
        .get_properties(handle)
        .map_err(|err| RuntimeError::HostOutputInspect {
            path: device_path.display().to_string(),
            error: format!("failed to fetch drm object properties: {err}"),
        })?;
    let mut map = HashMap::new();
    for (property, _) in values.iter() {
        let info = card
            .get_property(*property)
            .map_err(|err| RuntimeError::HostOutputInspect {
                path: device_path.display().to_string(),
                error: format!(
                    "failed to inspect drm property {}: {err}",
                    u32::from(*property)
                ),
            })?;
        if let Ok(name) = info.name().to_str() {
            map.insert(name.to_string(), *property);
        }
    }
    Ok(map)
}

#[derive(Clone, Copy)]
enum PlaneSelection {
    Primary,
    Overlay,
}

fn select_atomic_plane(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    resources: &drm_api::control::ResourceHandles,
    crtc: drm_crtc::Handle,
    selection: PlaneSelection,
) -> Result<Option<AtomicPlaneCandidate>, RuntimeError> {
    let mut planes = card
        .plane_handles()
        .map_err(|err| RuntimeError::HostOutputInspect {
            path: device_path.display().to_string(),
            error: format!("failed to enumerate drm planes: {err}"),
        })?;
    planes.sort_by_key(|plane| u32::from(*plane));

    for plane in planes {
        let info = card
            .get_plane(plane)
            .map_err(|err| RuntimeError::HostOutputInspect {
                path: device_path.display().to_string(),
                error: format!("failed to inspect drm plane {}: {err}", u32::from(plane)),
            })?;
        if !resources
            .filter_crtcs(info.possible_crtcs())
            .contains(&crtc)
        {
            continue;
        }
        let Some(scanout_format) = select_preferred_scanout_format(info.formats(), selection)
        else {
            continue;
        };
        let matches = match selection {
            PlaneSelection::Primary => plane_has_type(card, device_path, plane, "Primary")?,
            PlaneSelection::Overlay => plane_has_type(card, device_path, plane, "Overlay")?,
        };
        if matches {
            return Ok(Some(AtomicPlaneCandidate {
                handle: plane,
                scanout_format,
            }));
        }
    }

    Ok(None)
}

fn select_preferred_scanout_format(
    formats: &[u32],
    selection: PlaneSelection,
) -> Option<DrmFourcc> {
    let preference = match selection {
        PlaneSelection::Primary => &PRIMARY_SCANOUT_FORMAT_PREFERENCE,
        PlaneSelection::Overlay => &OVERLAY_SCANOUT_FORMAT_PREFERENCE,
    };
    preference
        .iter()
        .copied()
        .find(|candidate| formats.iter().any(|format| *format == *candidate as u32))
}

fn overlay_scanout_format_supports_alpha(format: DrmFourcc) -> bool {
    matches!(format, DrmFourcc::Argb8888)
}

fn plane_has_type(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    plane: drm_plane::Handle,
    desired: &str,
) -> Result<bool, RuntimeError> {
    let values = card
        .get_properties(plane)
        .map_err(|err| RuntimeError::HostOutputInspect {
            path: device_path.display().to_string(),
            error: format!(
                "failed to inspect plane properties for {}: {err}",
                u32::from(plane)
            ),
        })?;
    for (property, raw_value) in values.iter() {
        let info = card
            .get_property(*property)
            .map_err(|err| RuntimeError::HostOutputInspect {
                path: device_path.display().to_string(),
                error: format!(
                    "failed to inspect plane property {}: {err}",
                    u32::from(*property)
                ),
            })?;
        let Ok(name) = info.name().to_str() else {
            continue;
        };
        if name != "type" {
            continue;
        }
        let drm_property::ValueType::Enum(values) = info.value_type() else {
            return Ok(false);
        };
        let Some(enum_value) = values.get_value_from_raw_value(*raw_value) else {
            return Ok(false);
        };
        return Ok(enum_value.name().to_str().ok() == Some(desired));
    }
    Ok(false)
}

fn plane_property_handles(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    plane: drm_plane::Handle,
) -> Result<Option<AtomicPlanePropertyHandles>, RuntimeError> {
    let props = property_handle_map_for_resource(card, device_path, plane)?;
    let Some(crtc_id) = props.get("CRTC_ID").copied() else {
        return Ok(None);
    };
    let Some(fb_id) = props.get("FB_ID").copied() else {
        return Ok(None);
    };
    let Some(src_x) = props.get("SRC_X").copied() else {
        return Ok(None);
    };
    let Some(src_y) = props.get("SRC_Y").copied() else {
        return Ok(None);
    };
    let Some(src_w) = props.get("SRC_W").copied() else {
        return Ok(None);
    };
    let Some(src_h) = props.get("SRC_H").copied() else {
        return Ok(None);
    };
    let Some(crtc_x) = props.get("CRTC_X").copied() else {
        return Ok(None);
    };
    let Some(crtc_y) = props.get("CRTC_Y").copied() else {
        return Ok(None);
    };
    let Some(crtc_w) = props.get("CRTC_W").copied() else {
        return Ok(None);
    };
    let Some(crtc_h) = props.get("CRTC_H").copied() else {
        return Ok(None);
    };
    let zpos = if let Some(zpos_handle) = props
        .get("zpos")
        .copied()
        .or_else(|| props.get("ZPOS").copied())
    {
        let info =
            card.get_property(zpos_handle)
                .map_err(|err| RuntimeError::HostOutputInspect {
                    path: device_path.display().to_string(),
                    error: format!(
                        "failed to inspect plane zpos property {}: {err}",
                        u32::from(zpos_handle)
                    ),
                })?;
        if !info.mutable() || !info.atomic() {
            None
        } else {
            match info.value_type() {
                drm_property::ValueType::UnsignedRange(min, max) => Some(AtomicPlaneZposProperty {
                    handle: zpos_handle,
                    min,
                    max,
                }),
                _ => None,
            }
        }
    } else {
        None
    };
    let alpha = if let Some(alpha_handle) = props
        .get("alpha")
        .copied()
        .or_else(|| props.get("ALPHA").copied())
    {
        let info =
            card.get_property(alpha_handle)
                .map_err(|err| RuntimeError::HostOutputInspect {
                    path: device_path.display().to_string(),
                    error: format!(
                        "failed to inspect plane alpha property {}: {err}",
                        u32::from(alpha_handle)
                    ),
                })?;
        if !info.mutable() || !info.atomic() {
            None
        } else {
            match info.value_type() {
                drm_property::ValueType::UnsignedRange(min, max) => {
                    Some(AtomicPlaneAlphaProperty {
                        handle: alpha_handle,
                        min,
                        max,
                    })
                }
                _ => None,
            }
        }
    } else {
        None
    };
    let pixel_blend_mode = if let Some(pixel_blend_mode_handle) = props
        .get("pixel blend mode")
        .copied()
        .or_else(|| props.get("PIXEL_BLEND_MODE").copied())
    {
        let info = card.get_property(pixel_blend_mode_handle).map_err(|err| {
            RuntimeError::HostOutputInspect {
                path: device_path.display().to_string(),
                error: format!(
                    "failed to inspect plane pixel blend mode property {}: {err}",
                    u32::from(pixel_blend_mode_handle)
                ),
            }
        })?;
        if !info.mutable() || !info.atomic() {
            None
        } else {
            match info.value_type() {
                drm_property::ValueType::Enum(values) => {
                    let mut premultiplied = None;
                    let mut coverage = None;
                    let mut none = None;
                    for enum_value in values.values().1 {
                        let normalized =
                            normalize_drm_enum_name(enum_value.name().to_string_lossy().as_ref());
                        match normalized.as_str() {
                            "premultiplied" => premultiplied = Some(enum_value.value()),
                            "coverage" => coverage = Some(enum_value.value()),
                            "none" => none = Some(enum_value.value()),
                            _ => {}
                        }
                    }
                    Some(AtomicPlanePixelBlendModeProperty {
                        handle: pixel_blend_mode_handle,
                        premultiplied,
                        coverage,
                        none,
                    })
                }
                _ => None,
            }
        }
    } else {
        None
    };
    Ok(Some(AtomicPlanePropertyHandles {
        crtc_id,
        fb_id,
        src_x,
        src_y,
        src_w,
        src_h,
        crtc_x,
        crtc_y,
        crtc_w,
        crtc_h,
        zpos,
        alpha,
        pixel_blend_mode,
    }))
}

fn normalize_drm_enum_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn assign_atomic_plane_zpos(
    device_path: &Path,
    primary: &mut AtomicPlaneState,
    overlay: &mut AtomicPlaneState,
) {
    let Some(primary_zpos) = primary.props.zpos.as_ref() else {
        return;
    };
    let Some(overlay_zpos) = overlay.props.zpos.as_ref() else {
        return;
    };
    let Some((primary_value, overlay_value)) = select_atomic_plane_zpos_values(
        primary_zpos.min,
        primary_zpos.max,
        overlay_zpos.min,
        overlay_zpos.max,
    ) else {
        eprintln!(
            "host backend found atomic zpos properties on {} but could not establish primary<overlay ordering; leaving driver defaults",
            device_path.display()
        );
        return;
    };
    primary.zpos = Some(primary_value);
    overlay.zpos = Some(overlay_value);
}

fn select_atomic_plane_zpos_values(
    primary_min: u64,
    primary_max: u64,
    overlay_min: u64,
    overlay_max: u64,
) -> Option<(u64, u64)> {
    let primary_value = primary_min;
    if primary_value > primary_max {
        return None;
    }
    let overlay_value = overlay_min.max(primary_value.saturating_add(1));
    if overlay_value > overlay_max {
        return None;
    }
    Some((primary_value, overlay_value))
}

fn configure_atomic_plane_composition_controls(device_path: &Path, plane: &mut AtomicPlaneState) {
    if let Some(alpha) = plane.props.alpha.as_ref() {
        plane.alpha = Some(select_plane_alpha_value(alpha.min, alpha.max));
    }
    if !matches!(plane.role, AtomicPlaneRole::Overlay) {
        return;
    }
    let Some(pixel_blend_mode) = plane.props.pixel_blend_mode.as_ref() else {
        plane.supports_alpha_blending = false;
        eprintln!(
            "host backend overlay plane on {} is missing pixel blend mode property; forcing fail-closed overlay-plane disable",
            device_path.display()
        );
        return;
    };
    if let Some(value) = pixel_blend_mode.premultiplied.or(pixel_blend_mode.coverage) {
        plane.pixel_blend_mode = Some(value);
        plane.supports_alpha_blending = true;
        return;
    }
    plane.supports_alpha_blending = false;
    plane.pixel_blend_mode = pixel_blend_mode.none;
    eprintln!(
        "host backend overlay plane on {} lacks alpha-capable pixel blend mode enum; forcing fail-closed overlay-plane disable",
        device_path.display()
    );
}

fn select_plane_alpha_value(min: u64, max: u64) -> u64 {
    if max < min {
        return min;
    }
    let full = u64::from(u16::MAX);
    full.clamp(min, max)
}

fn populate_atomic_plane_properties(
    request: &mut AtomicModeReq,
    atomic: &AtomicCommitState,
    framebuffer: Option<drm_framebuffer::Handle>,
    layout: Option<&AtomicPlaneLayout>,
    plane: &AtomicPlaneState,
) {
    if let Some(layout) = layout {
        request.add_property(
            plane.handle,
            plane.props.crtc_id,
            drm_property::Value::CRTC(Some(atomic.crtc)),
        );
        request.add_property(
            plane.handle,
            plane.props.fb_id,
            drm_property::Value::Framebuffer(framebuffer),
        );
        request.add_property(
            plane.handle,
            plane.props.src_x,
            drm_property::Value::UnsignedRange((u64::from(layout.src_x)) << 16),
        );
        request.add_property(
            plane.handle,
            plane.props.src_y,
            drm_property::Value::UnsignedRange((u64::from(layout.src_y)) << 16),
        );
        request.add_property(
            plane.handle,
            plane.props.src_w,
            drm_property::Value::UnsignedRange((u64::from(layout.src_w)) << 16),
        );
        request.add_property(
            plane.handle,
            plane.props.src_h,
            drm_property::Value::UnsignedRange((u64::from(layout.src_h)) << 16),
        );
        request.add_property(
            plane.handle,
            plane.props.crtc_x,
            drm_property::Value::SignedRange(i64::from(layout.crtc_x)),
        );
        request.add_property(
            plane.handle,
            plane.props.crtc_y,
            drm_property::Value::SignedRange(i64::from(layout.crtc_y)),
        );
        request.add_property(
            plane.handle,
            plane.props.crtc_w,
            drm_property::Value::UnsignedRange(u64::from(layout.crtc_w)),
        );
        request.add_property(
            plane.handle,
            plane.props.crtc_h,
            drm_property::Value::UnsignedRange(u64::from(layout.crtc_h)),
        );
    } else {
        request.add_property(
            plane.handle,
            plane.props.crtc_id,
            drm_property::Value::CRTC(None),
        );
        request.add_property(
            plane.handle,
            plane.props.fb_id,
            drm_property::Value::Framebuffer(None),
        );
    }
    if let (Some(zpos), Some(value)) = (plane.props.zpos.as_ref(), plane.zpos) {
        request.add_property(
            plane.handle,
            zpos.handle,
            drm_property::Value::UnsignedRange(value),
        );
    }
    if let (Some(alpha), Some(value)) = (plane.props.alpha.as_ref(), plane.alpha) {
        request.add_property(
            plane.handle,
            alpha.handle,
            drm_property::Value::UnsignedRange(value),
        );
    }
    if let (Some(pixel_blend_mode), Some(value)) = (
        plane.props.pixel_blend_mode.as_ref(),
        plane.pixel_blend_mode,
    ) {
        request.add_property(
            plane.handle,
            pixel_blend_mode.handle,
            drm_property::Value::Unknown(value),
        );
    }
}

fn claim_output_with_atomic_modeset(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    atomic: &AtomicCommitState,
    framebuffer: drm_framebuffer::Handle,
) -> Result<(), RuntimeError> {
    let mode_blob =
        card.create_property_blob(&atomic.mode)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to create atomic mode blob: {err}"),
            })?;
    let mode_blob_id = mode_blob
        .as_blob()
        .ok_or_else(|| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: "invalid mode blob value while creating atomic request".to_string(),
        })?;
    let mut request = AtomicModeReq::new();
    request.add_property(
        atomic.connector,
        atomic.connector_crtc_id,
        drm_property::Value::CRTC(Some(atomic.crtc)),
    );
    request.add_property(
        atomic.crtc,
        atomic.crtc_active,
        drm_property::Value::Boolean(true),
    );
    request.add_property(atomic.crtc, atomic.crtc_mode_id, mode_blob);
    let fullscreen_layout = AtomicPlaneLayout::fullscreen(atomic.mode_size);
    for plane in &atomic.plane_states {
        let (fb, layout) = match plane.role {
            AtomicPlaneRole::Primary => (Some(framebuffer), Some(&fullscreen_layout)),
            AtomicPlaneRole::Overlay => (None, None),
        };
        populate_atomic_plane_properties(&mut request, atomic, fb, layout, plane);
    }
    let commit = card.atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request);
    let _ = card.destroy_property_blob(mode_blob_id);
    commit.map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed atomic modeset claim commit: {err}"),
    })
}

fn overlay_plane_layout_for_frame(
    wayland_state: &RuntimeWaylandState,
    overlay_framebuffer: Option<drm_framebuffer::Handle>,
) -> Option<AtomicPlaneLayout> {
    let overlay_plane_rotation_supported = matches!(
        wayland_state
            .root_geometry_snapshot()
            .map(|snapshot| snapshot.rotation)
            .unwrap_or_else(|| lock_state(&wayland_state.shared_state).output_rotation()),
        OutputRotation::Deg0
    );
    if overlay_plane_rotation_supported
        && wayland_state.overlay_toplevel.is_some()
        && overlay_framebuffer.is_some()
    {
        AtomicPlaneLayout::from_overlay_rect(wayland_state.overlay_rect())
    } else {
        None
    }
}

fn runtime_dmabuf_format_status(format: Format) -> RuntimeDmabufFormatStatus {
    RuntimeDmabufFormatStatus {
        code: format.code as u32,
        modifier: format.modifier.into(),
    }
}

fn queue_atomic_frame_commit(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    atomic: &AtomicCommitState,
    framebuffer: Option<drm_framebuffer::Handle>,
    overlay_framebuffer: Option<drm_framebuffer::Handle>,
    wayland_state: &RuntimeWaylandState,
) -> Result<(), RuntimeError> {
    let mut request = AtomicModeReq::new();
    let fullscreen_layout = AtomicPlaneLayout::fullscreen(atomic.mode_size);
    let overlay_layout = overlay_plane_layout_for_frame(wayland_state, overlay_framebuffer);
    for plane in &atomic.plane_states {
        let (fb, layout) = match plane.role {
            AtomicPlaneRole::Primary => (framebuffer, Some(&fullscreen_layout)),
            AtomicPlaneRole::Overlay => (overlay_framebuffer, overlay_layout.as_ref()),
        };
        populate_atomic_plane_properties(&mut request, atomic, fb, layout, plane);
    }
    card.atomic_commit(
        AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
        request,
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to queue atomic frame commit: {err}"),
    })
}

fn queue_atomic_modeset_frame_commit(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    atomic: &AtomicCommitState,
    framebuffer: drm_framebuffer::Handle,
    overlay_framebuffer: Option<drm_framebuffer::Handle>,
    wayland_state: &RuntimeWaylandState,
) -> Result<(), RuntimeError> {
    let mode_blob =
        card.create_property_blob(&atomic.mode)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to create atomic mode blob: {err}"),
            })?;
    let mode_blob_id = mode_blob
        .as_blob()
        .ok_or_else(|| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: "invalid mode blob value while creating atomic request".to_string(),
        })?;
    let mut request = AtomicModeReq::new();
    request.add_property(
        atomic.connector,
        atomic.connector_crtc_id,
        drm_property::Value::CRTC(Some(atomic.crtc)),
    );
    request.add_property(
        atomic.crtc,
        atomic.crtc_active,
        drm_property::Value::Boolean(true),
    );
    request.add_property(atomic.crtc, atomic.crtc_mode_id, mode_blob);
    let fullscreen_layout = AtomicPlaneLayout::fullscreen(atomic.mode_size);
    let overlay_layout = overlay_plane_layout_for_frame(wayland_state, overlay_framebuffer);
    for plane in &atomic.plane_states {
        let (fb, layout) = match plane.role {
            AtomicPlaneRole::Primary => (Some(framebuffer), Some(&fullscreen_layout)),
            AtomicPlaneRole::Overlay => (overlay_framebuffer, overlay_layout.as_ref()),
        };
        populate_atomic_plane_properties(&mut request, atomic, fb, layout, plane);
    }
    let commit = card.atomic_commit(
        AtomicCommitFlags::ALLOW_MODESET
            | AtomicCommitFlags::PAGE_FLIP_EVENT
            | AtomicCommitFlags::NONBLOCK,
        request,
    );
    let _ = card.destroy_property_blob(mode_blob_id);
    commit.map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to queue atomic modeset frame commit: {err}"),
    })
}

fn claim_output_on_device(
    opened: &mut OpenedHostDevice,
    plan: OutputClaimPlan,
    required_startup_ownership: Option<StartupPresentOwnership>,
    defer_activation: bool,
) -> Result<ClaimedOutput, RuntimeError> {
    let card = HostKmsCard::new(&opened.fd);
    let mut dumb_buffers: Option<[ClaimedOutputBuffer; 2]> = None;
    let mut used_direct_startup = false;
    let mut atomic_commit_state: Option<AtomicCommitState> = None;
    let mut atomic_candidate = AtomicCommitState::from_plan(&plan);
    let primary_scanout_format = atomic_candidate
        .as_ref()
        .map(|atomic| atomic.primary_scanout_format)
        .unwrap_or(DrmFourcc::Xrgb8888);
    let overlay_scanout_format = atomic_candidate
        .as_ref()
        .and_then(|atomic| {
            if atomic.overlay_alpha_blending_supported {
                atomic.overlay_scanout_format
            } else {
                None
            }
        })
        .filter(|format| overlay_scanout_format_supports_alpha(*format));
    let force_readback_present = std::env::var_os("SURF_ACE_HOST_RUNTIME_FORCE_READBACK").is_some();
    let requires_direct_startup = matches!(
        required_startup_ownership,
        Some(StartupPresentOwnership::DirectGbm)
    ) && !force_readback_present;

    let gles_renderer = match build_host_gles_renderer_state(
        &opened.fd,
        opened.node,
        &opened.path,
        plan.mode.size(),
        primary_scanout_format,
        overlay_scanout_format,
    ) {
        Ok(mut renderer) => {
            if !force_readback_present {
                match prime_direct_startup_frame(&mut renderer, &opened.path, plan.mode.size()) {
                    Ok(Some(framebuffer)) => {
                        if defer_activation && atomic_candidate.is_some() {
                            used_direct_startup = true;
                            atomic_commit_state = atomic_candidate.take();
                        } else if let Some(atomic) = atomic_candidate.as_ref() {
                            match claim_output_with_atomic_modeset(
                                &card,
                                &opened.path,
                                &atomic,
                                framebuffer,
                            ) {
                                Ok(()) => {
                                    used_direct_startup = true;
                                    atomic_commit_state = atomic_candidate.take();
                                }
                                Err(err) => {
                                    eprintln!(
                                        "host backend could not use atomic direct startup modeset on {}: {err}; trying legacy set_crtc",
                                        opened.path.display()
                                    );
                                }
                            }
                        }
                        if !used_direct_startup && !defer_activation {
                            if let Err(err) = card.set_crtc(
                                plan.crtc,
                                Some(framebuffer),
                                (0, 0),
                                &[plan.connector],
                                Some(plan.mode),
                            ) {
                                eprintln!(
                                    "host backend could not use direct gbm framebuffer for startup modeset on {}: {err}",
                                    opened.path.display()
                                );
                                renderer.direct_scanout = None;
                            } else {
                                used_direct_startup = true;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!(
                            "host backend could not prime direct gbm startup frame on {}: {err}",
                            opened.path.display()
                        );
                        renderer.direct_scanout = None;
                    }
                }
            }
            Some(renderer)
        }
        Err(err) => {
            eprintln!(
                "host backend could not initialize gbm/egl/gles renderer on {}: {err}",
                opened.path.display()
            );
            None
        }
    };

    if requires_direct_startup && !used_direct_startup {
        return Err(RuntimeError::HostOutputClaim {
            path: opened.path.display().to_string(),
            error:
                "direct-present ownership is required for this claim/reclaim, but direct startup modeset could not be established"
                    .to_string(),
        });
    }

    if !used_direct_startup {
        let first = create_claim_buffer(
            &card,
            &opened.path,
            plan.mode.size(),
            [0x10, 0x10, 0x10, 0x00],
        )?;
        let second = create_claim_buffer(
            &card,
            &opened.path,
            plan.mode.size(),
            [0x38, 0x18, 0x18, 0x00],
        )?;
        let mut claimed_with_atomic = false;
        if defer_activation {
            atomic_commit_state = atomic_candidate.take();
        } else if let Some(atomic) = atomic_candidate.as_ref() {
            match claim_output_with_atomic_modeset(&card, &opened.path, &atomic, first.fb) {
                Ok(()) => {
                    claimed_with_atomic = true;
                    atomic_commit_state = atomic_candidate.take();
                }
                Err(err) => {
                    eprintln!(
                        "host backend atomic dumb startup claim failed on {}: {err}; falling back to legacy set_crtc",
                        opened.path.display()
                    );
                }
            }
        }
        if defer_activation && atomic_commit_state.is_none() {
            let _ = card.destroy_framebuffer(first.fb);
            let _ = card.destroy_dumb_buffer(first.dumb);
            let _ = card.destroy_framebuffer(second.fb);
            let _ = card.destroy_dumb_buffer(second.dumb);
            return Err(RuntimeError::HostOutputClaim {
                path: opened.path.display().to_string(),
                error: "root4 reclaim requires an evented atomic modeset; legacy set_crtc cannot satisfy the geometry transaction".to_string(),
            });
        }
        if !claimed_with_atomic && !defer_activation {
            if let Err(err) = card.set_crtc(
                plan.crtc,
                Some(first.fb),
                (0, 0),
                &[plan.connector],
                Some(plan.mode),
            ) {
                let _ = card.destroy_framebuffer(first.fb);
                let _ = card.destroy_dumb_buffer(first.dumb);
                let _ = card.destroy_framebuffer(second.fb);
                let _ = card.destroy_dumb_buffer(second.dumb);
                return Err(RuntimeError::HostOutputClaim {
                    path: opened.path.display().to_string(),
                    error: format!(
                        "failed to modeset connector {} with crtc {}: {err}",
                        u32::from(plan.connector),
                        u32::from(plan.crtc)
                    ),
                });
            }
        }
        dumb_buffers = Some([first, second]);
    }

    // Keep direct scanout as primary when available, while retaining lazy dumb fallback buffers.
    let pipeline = ClaimedPresentationPipeline {
        crtc: plan.crtc,
        dumb_buffers,
        dumb_front_buffer: 0,
        dumb_back_buffer: 1,
        atomic_commit_state,
        pending_atomic_modeset: defer_activation,
        flip_pending: false,
        pending_flip_source: None,
        pending_presentation_token: None,
        gles_renderer,
    };
    if defer_activation {
        opened.prepared_pipeline = Some(pipeline);
    } else {
        opened.claimed_pipeline = Some(pipeline);
    }
    Ok(ClaimedOutput {
        mode: plan.mode,
        startup_present_ownership: if used_direct_startup {
            StartupPresentOwnership::DirectGbm
        } else {
            StartupPresentOwnership::Dumb
        },
        identity: OutputIdentity {
            device_path: opened.path.clone(),
            connector_name: plan.connector_name,
            connector_id: u32::from(plan.connector),
        },
    })
}

fn create_claim_buffer(
    card: &HostKmsCard<'_>,
    device_path: &Path,
    mode_size: (u16, u16),
    color: [u8; 4],
) -> Result<ClaimedOutputBuffer, RuntimeError> {
    let (width, height) = mode_size;
    let mut dumb = card
        .create_dumb_buffer((width as u32, height as u32), DrmFourcc::Xrgb8888, 32)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create dumb buffer: {err}"),
        })?;
    fill_dumb_buffer(card, &mut dumb, color).map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to initialize dumb buffer: {err}"),
    })?;
    let fb = card
        .add_framebuffer(&dumb, 24, 32)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to add dumb framebuffer: {err}"),
        })?;
    Ok(ClaimedOutputBuffer { fb, dumb })
}

fn fill_dumb_buffer(
    card: &HostKmsCard<'_>,
    dumb: &mut drm_dumbbuffer::DumbBuffer,
    color: [u8; 4],
) -> Result<(), std::io::Error> {
    let mut mapping = card.map_dumb_buffer(dumb)?;
    for chunk in mapping.chunks_exact_mut(4) {
        chunk.copy_from_slice(&color);
    }
    Ok(())
}

fn ensure_dumb_fallback_buffers(
    pipeline: &mut ClaimedPresentationPipeline,
    card: &HostKmsCard<'_>,
    device_path: &Path,
    mode_size: (u16, u16),
) -> Result<(), RuntimeError> {
    if pipeline.dumb_buffers.is_some() {
        return Ok(());
    }
    let first = create_claim_buffer(card, device_path, mode_size, [0x10, 0x10, 0x10, 0x00])?;
    let second = create_claim_buffer(card, device_path, mode_size, [0x38, 0x18, 0x18, 0x00])?;
    pipeline.dumb_buffers = Some([first, second]);
    pipeline.dumb_front_buffer = 0;
    pipeline.dumb_back_buffer = 1;
    Ok(())
}

fn build_host_gles_renderer_state(
    fd: &OwnedFd,
    scanout_node: DrmNode,
    device_path: &Path,
    mode_size: (u16, u16),
    primary_scanout_format: DrmFourcc,
    overlay_scanout_format: Option<DrmFourcc>,
) -> Result<HostGlesRendererState, RuntimeError> {
    let (mode_w, mode_h) = mode_size;
    let size = Size::<i32, BufferCoords>::from((mode_w as i32, mode_h as i32));
    let drm_fd = dup(fd.as_fd()).map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to duplicate drm fd for framebuffer export: {err}"),
    })?;
    let drm_device_fd = DrmDeviceFd::new(DeviceFd::from(drm_fd));
    let scanout_fd = dup(fd.as_fd()).map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to duplicate drm fd for gbm scanout allocation: {err}"),
    })?;
    let scanout_gbm_device = GbmDevice::new(DeviceFd::from(scanout_fd)).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gbm scanout device: {err}"),
        }
    })?;

    let (render_fd, render_node, render_path) = match scanout_node.node_with_type(NodeType::Render)
    {
        Some(render_node_result) => {
            let render_node = render_node_result.map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to resolve render node for {}: {err}", scanout_node),
            })?;
            let render_path =
                render_node
                    .dev_path()
                    .ok_or_else(|| RuntimeError::HostOutputClaim {
                        path: device_path.display().to_string(),
                        error: format!("resolved render node {} has no device path", render_node),
                    })?;
            let render_file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&render_path)
                .map_err(|err| {
                    let hint = if err.kind() == std::io::ErrorKind::PermissionDenied {
                        "; grant the compositor user access to the render node (for example via the render group or a logind ACL)"
                    } else {
                        ""
                    };
                    RuntimeError::HostOutputClaim {
                        path: device_path.display().to_string(),
                        error: format!(
                            "failed to open render node {} for gbm/egl renderer: {err}{}",
                            render_path.display(),
                            hint
                        ),
                    }
                })?;
            (render_file.into(), render_node, render_path)
        }
        None => {
            let render_fd = dup(fd.as_fd()).map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to duplicate drm fd for gbm/egl renderer: {err}"),
            })?;
            let render_path = scanout_node
                .dev_path()
                .unwrap_or_else(|| device_path.to_path_buf());
            (render_fd, scanout_node, render_path)
        }
    };

    let render_gbm_device =
        GbmDevice::new(DeviceFd::from(render_fd)).map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!(
                "failed to create gbm device on {}: {err}",
                render_path.display()
            ),
        })?;
    let egl_display = unsafe { EGLDisplay::new(render_gbm_device.clone()) }.map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!(
                "failed to create egl display for gbm device on {}: {err}",
                render_path.display()
            ),
        }
    })?;
    let egl_context =
        EGLContext::new(&egl_display).map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create egl context: {err}"),
        })?;
    let mut renderer =
        unsafe { GlesRenderer::new(egl_context) }.map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gles renderer: {err}"),
        })?;
    let target_texture = renderer
        .create_buffer(GLES_INTERMEDIATE_RENDER_FORMAT, size)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gles offscreen render target: {err}"),
        })?;
    let scanout_texture = renderer
        .create_buffer(GLES_INTERMEDIATE_RENDER_FORMAT, size)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gles scanout composite target: {err}"),
        })?;
    let direct_scanout = match build_host_direct_scanout_state(
        &drm_device_fd,
        &scanout_gbm_device,
        device_path,
        size,
        primary_scanout_format,
    ) {
        Ok(state) => Some(state),
        Err(err) => {
            eprintln!(
                "host backend could not initialize direct gbm scanout buffers on {}: {err}",
                device_path.display()
            );
            None
        }
    };

    Ok(HostGlesRendererState {
        render_node,
        _render_gbm_device: render_gbm_device,
        _scanout_gbm_device: scanout_gbm_device,
        _drm_device_fd: drm_device_fd,
        _egl_display: egl_display,
        renderer,
        target_texture,
        scanout_texture,
        primary_scanout_format,
        overlay_scanout_format,
        direct_scanout,
        overlay_scanout: None,
    })
}

const GBM_BUFFER_FROM_BO_PRESERVE_EXPLICIT_MODIFIER: bool = false;

macro_rules! gbm_buffer_from_allocated_bo_preserving_modifier {
    ($bo:expr) => {{
        // Smithay's `implicit` flag discards the BO's real modifier by forcing
        // Modifier::Invalid. These scanout allocations come from the modern
        // modifier-aware GBM path, so keep the true modifier for addfb/dmabuf export.
        GbmBuffer::from_bo($bo, GBM_BUFFER_FROM_BO_PRESERVE_EXPLICIT_MODIFIER)
    }};
}

fn build_host_direct_scanout_state(
    drm_device_fd: &DrmDeviceFd,
    gbm_device: &GbmDevice<DeviceFd>,
    device_path: &Path,
    size: Size<i32, BufferCoords>,
    scanout_format: DrmFourcc,
) -> Result<HostDirectScanoutState, RuntimeError> {
    let first = create_host_direct_scanout_buffer(
        drm_device_fd,
        gbm_device,
        device_path,
        size,
        scanout_format,
    )?;
    let second = create_host_direct_scanout_buffer(
        drm_device_fd,
        gbm_device,
        device_path,
        size,
        scanout_format,
    )?;
    Ok(HostDirectScanoutState {
        buffers: [first, second],
        front_buffer: 0,
        back_buffer: 1,
    })
}

fn create_host_direct_scanout_buffer(
    drm_device_fd: &DrmDeviceFd,
    gbm_device: &GbmDevice<DeviceFd>,
    device_path: &Path,
    size: Size<i32, BufferCoords>,
    scanout_format: DrmFourcc,
) -> Result<HostDirectScanoutBuffer, RuntimeError> {
    let bo = gbm_device
        .create_buffer_object(
            size.w.max(1) as u32,
            size.h.max(1) as u32,
            scanout_format,
            GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to allocate gbm direct scanout buffer: {err}"),
        })?;
    let gbm_buffer = gbm_buffer_from_allocated_bo_preserving_modifier!(bo);
    let framebuffer = framebuffer_from_bo(drm_device_fd, &gbm_buffer, false).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to export drm framebuffer from gbm buffer: {err}"),
        }
    })?;
    let dmabuf = gbm_buffer
        .export()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to export dmabuf from gbm buffer: {err}"),
        })?;
    Ok(HostDirectScanoutBuffer {
        _gbm_buffer: gbm_buffer,
        dmabuf,
        framebuffer,
    })
}

fn ensure_direct_scanout_state(
    gles_state: &mut HostGlesRendererState,
    device_path: &Path,
    size: Size<i32, BufferCoords>,
) -> Result<(), RuntimeError> {
    if gles_state.direct_scanout.is_some() {
        return Ok(());
    }
    let state = build_host_direct_scanout_state(
        &gles_state._drm_device_fd,
        &gles_state._scanout_gbm_device,
        device_path,
        size,
        gles_state.primary_scanout_format,
    )?;
    gles_state.direct_scanout = Some(state);
    Ok(())
}

fn ensure_overlay_scanout_state(
    gles_state: &mut HostGlesRendererState,
    device_path: &Path,
    size: Size<i32, BufferCoords>,
) -> Result<(), RuntimeError> {
    let recreate = match &gles_state.overlay_scanout {
        Some(state) => state.size != size,
        None => true,
    };
    if !recreate {
        return Ok(());
    }
    let gbm_buffer = gles_state
        ._scanout_gbm_device
        .create_buffer_object(
            size.w.max(1) as u32,
            size.h.max(1) as u32,
            gles_state
                .overlay_scanout_format
                .unwrap_or(gles_state.primary_scanout_format),
            GbmBufferFlags::SCANOUT | GbmBufferFlags::RENDERING,
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to allocate overlay gbm buffer: {err}"),
        })?;
    let gbm_buffer = gbm_buffer_from_allocated_bo_preserving_modifier!(gbm_buffer);
    let framebuffer =
        framebuffer_from_bo(&gles_state._drm_device_fd, &gbm_buffer, false).map_err(|err| {
            RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to export overlay framebuffer from gbm buffer: {err}"),
            }
        })?;
    let dmabuf = gbm_buffer
        .export()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to export overlay dmabuf from gbm buffer: {err}"),
        })?;
    gles_state.overlay_scanout = Some(HostOverlayScanoutState {
        buffer: HostOverlayScanoutBuffer {
            _gbm_buffer: gbm_buffer,
            dmabuf,
            framebuffer,
        },
        size,
    });
    Ok(())
}

fn prime_direct_startup_frame(
    gles_state: &mut HostGlesRendererState,
    device_path: &Path,
    mode_size: (u16, u16),
) -> Result<Option<drm_framebuffer::Handle>, RuntimeError> {
    let size = Size::<i32, BufferCoords>::from((mode_size.0 as i32, mode_size.1 as i32));
    ensure_direct_scanout_state(gles_state, device_path, size)?;
    let Some(direct_scanout) = gles_state.direct_scanout.as_mut() else {
        return Ok(None);
    };
    let mut startup_dmabuf = direct_scanout.buffers[direct_scanout.front_buffer]
        .dmabuf
        .clone();
    let mut render_target = gles_state
        .renderer
        .bind(&mut startup_dmabuf)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to bind direct gbm startup dmabuf: {err}"),
        })?;
    let render_size = Size::<i32, Physical>::from((mode_size.0 as i32, mode_size.1 as i32));
    let damage = Rectangle::from_size(render_size);
    let mut frame = gles_state
        .renderer
        .render(&mut render_target, render_size, Transform::Normal)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin direct startup render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear direct startup render target: {err}"),
        })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish direct startup render pass: {err}"),
        })?;

    Ok(Some(
        *direct_scanout.buffers[direct_scanout.front_buffer]
            .framebuffer
            .as_ref(),
    ))
}

fn render_host_scene_with_gles_direct(
    gles_state: &mut HostGlesRendererState,
    wayland_state: &mut RuntimeWaylandState,
    device_path: &Path,
    output_w: i32,
    output_h: i32,
    prefer_overlay_plane_split: bool,
    screen_capture: &ScreenCaptureStore,
) -> Result<DirectRenderTargets, RuntimeError> {
    let scanout_size = Size::<i32, BufferCoords>::from((output_w.max(1), output_h.max(1)));
    let scene_render_size = render_output_size_before_transform(wayland_state);
    let scene_size = Size::<i32, BufferCoords>::from((scene_render_size.w, scene_render_size.h));
    ensure_gles_render_target_size(
        &mut gles_state.renderer,
        device_path,
        &mut gles_state.target_texture,
        scene_size,
        "gles offscreen render target",
    )?;
    let direct_scanout_needs_resize = gles_state
        .direct_scanout
        .as_ref()
        .map(|direct_scanout| {
            direct_scanout.buffers[direct_scanout.front_buffer]
                .dmabuf
                .size()
                != scanout_size
                || direct_scanout.buffers[direct_scanout.back_buffer]
                    .dmabuf
                    .size()
                    != scanout_size
        })
        .unwrap_or(true);
    if direct_scanout_needs_resize {
        gles_state.direct_scanout = None;
    }

    ensure_direct_scanout_state(gles_state, device_path, scanout_size)?;

    let (mut scanout_dmabuf, main_framebuffer) = {
        let Some(direct_scanout) = gles_state.direct_scanout.as_mut() else {
            return Ok(DirectRenderTargets {
                main: None,
                overlay: None,
            });
        };
        let back_buffer = direct_scanout.back_buffer;
        (
            direct_scanout.buffers[back_buffer].dmabuf.clone(),
            *direct_scanout.buffers[back_buffer].framebuffer.as_ref(),
        )
    };

    let rotation = wayland_state
        .root_geometry_snapshot()
        .map(|snapshot| snapshot.rotation)
        .unwrap_or_else(|| lock_state(&wayland_state.shared_state).output_rotation());
    let transform = transform_from_rotation(rotation);
    let logical_size = wayland_state.runtime_output_size();
    let capture = wayland_state.collect_render_elements(
        &mut gles_state.renderer,
        logical_size.w,
        logical_size.h,
    );
    if let Some(failure) = capture.failure.as_ref() {
        return Err(RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: failure.clone(),
        });
    }
    let overlay_framebuffer = if prefer_overlay_plane_split {
        render_overlay_plane_framebuffer(gles_state, wayland_state, device_path)?
    } else {
        gles_state.overlay_scanout = None;
        None
    };
    if matches!(rotation, OutputRotation::Deg90 | OutputRotation::Deg270) {
        if overlay_framebuffer.is_some() {
            let primary_elements = capture.primary_plane_elements();
            render_elements_to_texture(
                &mut gles_state.renderer,
                device_path,
                &mut gles_state.target_texture,
                scene_render_size,
                &primary_elements,
                wayland_state.root_display_scale(),
                "quarter-turn scene texture",
            )?;
        } else {
            render_elements_to_texture(
                &mut gles_state.renderer,
                device_path,
                &mut gles_state.target_texture,
                scene_render_size,
                &capture.elements,
                wayland_state.root_display_scale(),
                "quarter-turn scene texture",
            )?;
        }

        let mut render_target = gles_state
            .renderer
            .bind(&mut scanout_dmabuf)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to bind direct gbm scanout dmabuf: {err}"),
            })?;
        composite_scene_texture_to_physical_scanout(
            &mut gles_state.renderer,
            device_path,
            &mut render_target,
            &gles_state.target_texture,
            Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
            wayland_state.root_display_scale(),
            rotation,
            wayland_state.cursor_render_location(),
            scene_render_size,
        )?;
        let debug_borders_drawn = draw_overlay_region_debug_borders_to_gles_target(
            &mut gles_state.renderer,
            device_path,
            &mut render_target,
            wayland_state,
            scene_render_size,
            Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
            rotation,
            "direct scanout overlay-region debug borders",
        )?;
        if debug_borders_drawn {
            draw_software_cursor_to_gles_target(
                &mut gles_state.renderer,
                device_path,
                &mut render_target,
                wayland_state.cursor_render_location(),
                scene_render_size,
                Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
                wayland_state.root_display_scale(),
                rotation,
                "direct scanout cursor",
            )?;
        }
        capture_screen_from_render_target(
            screen_capture,
            &mut gles_state.renderer,
            &render_target,
            scanout_size.w.max(1) as usize,
            scanout_size.h.max(1) as usize,
            rotation,
            wayland_state.root_capture_snapshot(),
            &wayland_state.root_pane_capture_geometries(),
        );

        return Ok(DirectRenderTargets {
            main: Some(main_framebuffer),
            overlay: overlay_framebuffer,
        });
    }

    let render_size = Size::<i32, Physical>::from((scanout_size.w, scanout_size.h));
    let damage = Rectangle::from_size(render_size);
    let mut render_target = gles_state
        .renderer
        .bind(&mut scanout_dmabuf)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to bind direct gbm scanout dmabuf: {err}"),
        })?;
    let mut frame = gles_state
        .renderer
        .render(&mut render_target, render_size, transform)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin direct gles render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear direct scanout render target: {err}"),
        })?;
    if overlay_framebuffer.is_some() {
        let primary_elements = capture.primary_plane_elements();
        draw_render_elements(
            &mut frame,
            wayland_state.root_display_scale(),
            &primary_elements,
            &[damage],
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw scene elements into direct scanout buffer: {err}"),
        })?;
    } else {
        draw_render_elements(
            &mut frame,
            wayland_state.root_display_scale(),
            &capture.elements,
            &[damage],
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw scene elements into direct scanout buffer: {err}"),
        })?;
    }
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish direct gles render pass: {err}"),
        })?;
    let _ = draw_overlay_region_debug_borders_to_gles_target(
        &mut gles_state.renderer,
        device_path,
        &mut render_target,
        wayland_state,
        render_size,
        render_size,
        rotation,
        "direct scanout overlay-region debug borders",
    )?;
    draw_software_cursor_to_gles_target(
        &mut gles_state.renderer,
        device_path,
        &mut render_target,
        wayland_state.cursor_render_location(),
        render_size,
        render_size,
        wayland_state.root_display_scale(),
        rotation,
        "direct scanout cursor",
    )?;
    capture_screen_from_render_target(
        screen_capture,
        &mut gles_state.renderer,
        &render_target,
        scanout_size.w.max(1) as usize,
        scanout_size.h.max(1) as usize,
        rotation,
        wayland_state.root_capture_snapshot(),
        &wayland_state.root_pane_capture_geometries(),
    );

    Ok(DirectRenderTargets {
        main: Some(main_framebuffer),
        overlay: overlay_framebuffer,
    })
}

fn render_overlay_plane_framebuffer(
    gles_state: &mut HostGlesRendererState,
    wayland_state: &RuntimeWaylandState,
    device_path: &Path,
) -> Result<Option<drm_framebuffer::Handle>, RuntimeError> {
    if wayland_state.overlay_toplevel.is_none() {
        gles_state.overlay_scanout = None;
        return Ok(None);
    }
    let overlay_rect = wayland_state.overlay_rect();
    if overlay_rect.size.w <= 0 || overlay_rect.size.h <= 0 {
        gles_state.overlay_scanout = None;
        return Ok(None);
    }
    let overlay_elements =
        wayland_state.collect_overlay_plane_elements_local(&mut gles_state.renderer);
    if overlay_elements.is_empty() {
        gles_state.overlay_scanout = None;
        return Ok(None);
    }
    let size =
        Size::<i32, BufferCoords>::from((overlay_rect.size.w.max(1), overlay_rect.size.h.max(1)));
    ensure_overlay_scanout_state(gles_state, device_path, size)?;

    let Some(overlay_state) = gles_state.overlay_scanout.as_mut() else {
        return Ok(None);
    };
    let mut overlay_dmabuf = overlay_state.buffer.dmabuf.clone();
    let mut render_target = gles_state
        .renderer
        .bind(&mut overlay_dmabuf)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to bind overlay gbm dmabuf: {err}"),
        })?;
    let damage = Rectangle::from_size(Size::<i32, Physical>::from((size.w, size.h)));
    let mut frame = gles_state
        .renderer
        .render(
            &mut render_target,
            Size::<i32, Physical>::from((size.w, size.h)),
            Transform::Normal,
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin overlay gles render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear overlay render target: {err}"),
        })?;
    draw_render_elements(
        &mut frame,
        wayland_state.root_display_scale(),
        &overlay_elements,
        &[damage],
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to draw overlay elements: {err}"),
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish overlay render pass: {err}"),
        })?;
    Ok(Some(*overlay_state.buffer.framebuffer.as_ref()))
}

fn render_host_scene_with_gles_readback(
    gles_state: &mut HostGlesRendererState,
    wayland_state: &mut RuntimeWaylandState,
    device_path: &Path,
    screen_capture: &ScreenCaptureStore,
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) -> Result<(), RuntimeError> {
    let scanout_size = Size::<i32, BufferCoords>::from((output_w.max(1), output_h.max(1)));
    let rotation = wayland_state
        .root_geometry_snapshot()
        .map(|snapshot| snapshot.rotation)
        .unwrap_or_else(|| lock_state(&wayland_state.shared_state).output_rotation());
    let logical_size = wayland_state.runtime_output_size();
    let capture = wayland_state.collect_render_elements(
        &mut gles_state.renderer,
        logical_size.w,
        logical_size.h,
    );
    if let Some(failure) = capture.failure.as_ref() {
        return Err(RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: failure.clone(),
        });
    }
    let render_size = render_output_size_before_transform(wayland_state);

    if matches!(rotation, OutputRotation::Deg90 | OutputRotation::Deg270) {
        let scene_size = Size::<i32, BufferCoords>::from((render_size.w, render_size.h));
        ensure_gles_render_target_size(
            &mut gles_state.renderer,
            device_path,
            &mut gles_state.target_texture,
            scene_size,
            "gles quarter-turn scene render target",
        )?;
        ensure_gles_render_target_size(
            &mut gles_state.renderer,
            device_path,
            &mut gles_state.scanout_texture,
            scanout_size,
            "gles quarter-turn scanout composite target",
        )?;
        render_elements_to_texture(
            &mut gles_state.renderer,
            device_path,
            &mut gles_state.target_texture,
            render_size,
            &capture.elements,
            wayland_state.root_display_scale(),
            "quarter-turn scene texture",
        )?;

        let mut render_target = gles_state
            .renderer
            .bind(&mut gles_state.scanout_texture)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to bind quarter-turn scanout composite target: {err}"),
            })?;
        composite_scene_texture_to_physical_scanout(
            &mut gles_state.renderer,
            device_path,
            &mut render_target,
            &gles_state.target_texture,
            Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
            wayland_state.root_display_scale(),
            rotation,
            wayland_state.cursor_render_location(),
            render_size,
        )?;
        let debug_borders_drawn = draw_overlay_region_debug_borders_to_gles_target(
            &mut gles_state.renderer,
            device_path,
            &mut render_target,
            wayland_state,
            render_size,
            Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
            rotation,
            "readback overlay-region debug borders",
        )?;
        if debug_borders_drawn {
            draw_software_cursor_to_gles_target(
                &mut gles_state.renderer,
                device_path,
                &mut render_target,
                wayland_state.cursor_render_location(),
                render_size,
                Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
                wayland_state.root_display_scale(),
                rotation,
                "readback scanout cursor",
            )?;
        }
        let readback_region = Rectangle::from_size(scanout_size);
        let mapping = gles_state
            .renderer
            .copy_framebuffer(&render_target, readback_region, DrmFourcc::Xrgb8888)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to read back quarter-turn scanout buffer: {err}"),
            })?;
        let pixels = gles_state.renderer.map_texture(&mapping).map_err(|err| {
            RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to map quarter-turn scanout pixels: {err}"),
            }
        })?;
        if let Some(snapshot) = wayland_state.root_capture_snapshot() {
            screen_capture.update_root4_scanout_xrgb8888(
                pixels,
                scanout_size.w.max(1) as usize * 4,
                scanout_size.w.max(1) as usize,
                scanout_size.h.max(1) as usize,
                screen_capture_src_flipped(mapping.flipped(), rotation),
                snapshot,
                &wayland_state.root_pane_capture_geometries(),
            );
        }
        copy_renderer_pixels_to_dumb(
            pixels,
            mapping.flipped(),
            rotation,
            target,
            target_stride,
            scanout_size.w.max(1) as usize,
            scanout_size.h.max(1) as usize,
        );
        return Ok(());
    }

    ensure_gles_render_target_size(
        &mut gles_state.renderer,
        device_path,
        &mut gles_state.target_texture,
        scanout_size,
        "gles host render target",
    )?;
    let transform = transform_from_rotation(rotation);
    let damage = Rectangle::from_size(transform.transform_size(render_size));
    let mut render_target = gles_state
        .renderer
        .bind(&mut gles_state.target_texture)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to bind gles render target: {err}"),
        })?;
    let mut frame = gles_state
        .renderer
        .render(&mut render_target, render_size, transform)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin gles render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear gles render target: {err}"),
        })?;
    draw_render_elements(
        &mut frame,
        wayland_state.root_display_scale(),
        &capture.elements,
        &[damage],
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to draw scene elements with gles: {err}"),
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish gles render pass: {err}"),
        })?;
    let _ = draw_overlay_region_debug_borders_to_gles_target(
        &mut gles_state.renderer,
        device_path,
        &mut render_target,
        wayland_state,
        render_size,
        Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
        rotation,
        "readback overlay-region debug borders",
    )?;
    draw_software_cursor_to_gles_target(
        &mut gles_state.renderer,
        device_path,
        &mut render_target,
        wayland_state.cursor_render_location(),
        render_size,
        Size::<i32, Physical>::from((scanout_size.w, scanout_size.h)),
        wayland_state.root_display_scale(),
        rotation,
        "readback scanout cursor",
    )?;

    let readback_region = Rectangle::from_size(scanout_size);
    let mapping = gles_state
        .renderer
        .copy_framebuffer(&render_target, readback_region, DrmFourcc::Xrgb8888)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to read back gles framebuffer: {err}"),
        })?;

    let pixels =
        gles_state
            .renderer
            .map_texture(&mapping)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to map gles framebuffer pixels: {err}"),
            })?;
    copy_renderer_pixels_to_dumb(
        pixels,
        mapping.flipped(),
        rotation,
        target,
        target_stride,
        scanout_size.w.max(1) as usize,
        scanout_size.h.max(1) as usize,
    );
    Ok(())
}
fn render_output_size_before_transform(wayland_state: &RuntimeWaylandState) -> Size<i32, Physical> {
    let size = wayland_state
        .root_geometry_snapshot()
        .map(|snapshot| snapshot.oriented_physical_size_i32())
        .unwrap_or_else(|| {
            let size = wayland_state.runtime_output_size();
            crate::root_geometry::RootSizeI32 {
                width: size.w,
                height: size.h,
            }
        });
    Size::<i32, Physical>::from((size.width.max(1), size.height.max(1)))
}

fn draw_software_cursor_to_gles_target(
    renderer: &mut GlesRenderer,
    device_path: &Path,
    render_target: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    location: Point<f64, Logical>,
    logical_size: Size<i32, Physical>,
    scanout_size: Size<i32, Physical>,
    scale: f64,
    rotation: OutputRotation,
    target_name: &str,
) -> Result<(), RuntimeError> {
    let mut frame = renderer
        .render(render_target, scanout_size, Transform::Normal)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin {target_name} render pass: {err}"),
        })?;
    draw_software_cursor_frame(
        &mut frame,
        location,
        logical_size,
        scanout_size,
        scale,
        rotation,
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to draw {target_name}: {err}"),
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish {target_name} render pass: {err}"),
        })?;
    Ok(())
}

fn draw_overlay_region_debug_borders_to_gles_target(
    renderer: &mut GlesRenderer,
    device_path: &Path,
    render_target: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    wayland_state: &RuntimeWaylandState,
    logical_size: Size<i32, Physical>,
    scanout_size: Size<i32, Physical>,
    rotation: OutputRotation,
    target_name: &str,
) -> Result<bool, RuntimeError> {
    let (debug_enabled, regions) =
        lock_state(&wayland_state.shared_state).overlay_region_debug_render_snapshot();
    if !debug_enabled || regions.is_empty() {
        return Ok(false);
    }

    let mut frame = renderer
        .render(render_target, scanout_size, Transform::Normal)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin {target_name} render pass: {err}"),
        })?;
    draw_overlay_region_debug_borders_frame(
        &mut frame,
        &regions,
        logical_size,
        scanout_size,
        wayland_state.root_display_scale(),
        rotation,
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to draw {target_name}: {err}"),
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish {target_name} render pass: {err}"),
        })?;
    Ok(true)
}

fn draw_overlay_region_debug_borders_frame<F: Frame>(
    frame: &mut F,
    regions: &[OverlayRegionStatus],
    logical_size: Size<i32, Physical>,
    scanout_size: Size<i32, Physical>,
    scale: f64,
    rotation: OutputRotation,
) -> Result<(), F::Error> {
    let logical_size = Size::<i32, Logical>::from((logical_size.w, logical_size.h));
    let output_bounds = Rectangle::from_size(scanout_size);
    let output_damage = [output_bounds];
    for rect in overlay_region_debug_border_rects(regions, logical_size.w, logical_size.h) {
        let Some(rect) = cursor_rect_to_scanout(rect, scanout_size, scale, rotation)
            .and_then(|rect| rect.intersection(output_bounds))
        else {
            continue;
        };
        frame.draw_solid(rect, &output_damage, Color32F::new(1.0, 0.0, 0.85, 1.0))?;
    }
    Ok(())
}

fn draw_software_cursor_frame<F: Frame>(
    frame: &mut F,
    location: Point<f64, Logical>,
    logical_size: Size<i32, Physical>,
    scanout_size: Size<i32, Physical>,
    scale: f64,
    rotation: OutputRotation,
) -> Result<(), F::Error> {
    let logical_size = Size::<i32, Logical>::from((logical_size.w, logical_size.h));
    let output_bounds = Rectangle::from_size(scanout_size);
    let output_damage = [output_bounds];
    for (rect, color) in software_cursor_rects(location, logical_size.w, logical_size.h)
        .into_iter()
        .rev()
    {
        let Some(rect) = cursor_rect_to_scanout(rect, scanout_size, scale, rotation)
            .and_then(|rect| rect.intersection(output_bounds))
        else {
            continue;
        };
        let color = match color {
            SoftwareCursorColor::White => Color32F::new(1.0, 1.0, 1.0, 1.0),
            SoftwareCursorColor::Black => Color32F::new(0.0, 0.0, 0.0, 1.0),
        };
        frame.draw_solid(rect, &output_damage, color)?;
    }
    Ok(())
}

fn cursor_rect_to_scanout(
    rect: Rectangle<i32, Logical>,
    scanout_size: Size<i32, Physical>,
    scale: f64,
    rotation: OutputRotation,
) -> Option<Rectangle<i32, Physical>> {
    if rect.size.w <= 0
        || rect.size.h <= 0
        || scanout_size.w <= 0
        || scanout_size.h <= 0
        || !scale.is_finite()
        || scale <= 0.0
    {
        return None;
    }
    let x0 = rect.loc.x as f64;
    let y0 = rect.loc.y as f64;
    let x1 = (rect.loc.x + rect.size.w) as f64;
    let y1 = (rect.loc.y + rect.size.h) as f64;
    let raw_w = scanout_size.w as f64;
    let raw_h = scanout_size.h as f64;
    let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| match rotation {
        OutputRotation::Deg0 => (x * scale, y * scale),
        OutputRotation::Deg90 => (raw_w - y * scale, x * scale),
        OutputRotation::Deg180 => (raw_w - x * scale, raw_h - y * scale),
        OutputRotation::Deg270 => (y * scale, raw_h - x * scale),
    });
    let min_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f64::INFINITY, f64::min);
    let min_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f64::INFINITY, f64::min);
    let max_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let left = min_x.floor().clamp(0.0, raw_w) as i32;
    let top = min_y.floor().clamp(0.0, raw_h) as i32;
    let right = max_x.ceil().clamp(0.0, raw_w) as i32;
    let bottom = max_y.ceil().clamp(0.0, raw_h) as i32;
    Some(Rectangle::new(
        (left, top).into(),
        (right - left, bottom - top).into(),
    ))
}

fn ensure_gles_render_target_size(
    renderer: &mut GlesRenderer,
    device_path: &Path,
    target_texture: &mut GlesTexture,
    size: Size<i32, BufferCoords>,
    target_name: &str,
) -> Result<(), RuntimeError> {
    if target_texture.size() == size {
        return Ok(());
    }

    *target_texture = renderer
        .create_buffer(GLES_INTERMEDIATE_RENDER_FORMAT, size)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to resize {target_name}: {err}"),
        })?;
    Ok(())
}

fn render_elements_to_texture<E>(
    renderer: &mut GlesRenderer,
    device_path: &Path,
    target_texture: &mut GlesTexture,
    render_size: Size<i32, Physical>,
    elements: &[E],
    scale: f64,
    target_name: &str,
) -> Result<(), RuntimeError>
where
    E: smithay::backend::renderer::element::RenderElement<GlesRenderer>,
{
    let damage = Rectangle::from_size(render_size);
    let mut render_target =
        renderer
            .bind(target_texture)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to bind {target_name}: {err}"),
            })?;
    let mut frame = renderer
        .render(&mut render_target, render_size, Transform::Normal)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin {target_name} render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear {target_name}: {err}"),
        })?;
    draw_render_elements(&mut frame, scale, elements, &[damage]).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw scene elements into {target_name}: {err}"),
        }
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish {target_name} render pass: {err}"),
        })?;
    Ok(())
}

fn composite_scene_texture_to_physical_scanout(
    renderer: &mut GlesRenderer,
    device_path: &Path,
    render_target: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    scene_texture: &GlesTexture,
    scanout_size: Size<i32, Physical>,
    scale: f64,
    rotation: OutputRotation,
    cursor_location: Point<f64, Logical>,
    logical_size: Size<i32, Physical>,
) -> Result<(), RuntimeError> {
    let scanout_damage = Rectangle::from_size(scanout_size);
    let scene_src = Rectangle::from_size(scene_texture.size()).to_f64();
    // Rotation remains an output concern. Quarter-turn paths therefore render the
    // scene once in logical coordinates, then realize the panel/capture image by
    // copying that scene texture into a physical scanout-sized target.
    let mut frame = renderer
        .render(render_target, scanout_size, Transform::Normal)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to begin quarter-turn scanout render pass: {err}"),
        })?;
    frame
        .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[scanout_damage])
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to clear quarter-turn scanout buffer: {err}"),
        })?;
    frame
        .render_texture_from_to(
            scene_texture,
            scene_src,
            Rectangle::from_size(scanout_size),
            &[scanout_damage],
            &[],
            scene_texture_transform(rotation),
            1.0,
            None,
            &[],
        )
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!(
                "failed to composite quarter-turn scene texture into scanout buffer: {err}"
            ),
        })?;
    draw_software_cursor_frame(
        &mut frame,
        cursor_location,
        logical_size,
        scanout_size,
        scale,
        rotation,
    )
    .map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to draw quarter-turn scanout cursor: {err}"),
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish quarter-turn scanout render pass: {err}"),
        })?;
    Ok(())
}

fn scene_texture_transform(rotation: OutputRotation) -> Transform {
    OutputRotationModel::new(rotation).scene_texture_transform()
}

fn screen_capture_src_flipped(mapping_flipped: bool, rotation: OutputRotation) -> bool {
    OutputRotationModel::new(rotation).capture_src_flipped(mapping_flipped)
}

fn capture_screen_from_render_target(
    screen_capture: &ScreenCaptureStore,
    renderer: &mut GlesRenderer,
    render_target: &smithay::backend::renderer::gles::GlesTarget<'_>,
    width: usize,
    height: usize,
    rotation: OutputRotation,
    root_geometry: Option<crate::root_geometry::RootGeometrySnapshot>,
    pane_captures: &[(PaneId, crate::root_geometry::CaptureGeometry)],
) {
    let region = Rectangle::from_size(Size::<i32, BufferCoords>::from((
        width.max(1) as i32,
        height.max(1) as i32,
    )));
    let mapping = match renderer.copy_framebuffer(render_target, region, DrmFourcc::Xrgb8888) {
        Ok(mapping) => mapping,
        Err(err) => {
            eprintln!("host screen capture readback failed: {err}");
            return;
        }
    };
    let flipped = mapping.flipped();
    let pixels = match renderer.map_texture(&mapping) {
        Ok(pixels) => pixels,
        Err(err) => {
            eprintln!("host screen capture map failed: {err}");
            return;
        }
    };
    if let Some(root_geometry) = root_geometry {
        screen_capture.update_root4_scanout_xrgb8888(
            pixels,
            width.saturating_mul(4),
            width,
            height,
            screen_capture_src_flipped(flipped, rotation),
            root_geometry,
            pane_captures,
        );
    }
}

fn copy_renderer_pixels_to_dumb(
    src_pixels: &[u8],
    src_flipped: bool,
    rotation: OutputRotation,
    dst_pixels: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    let src_stride = width.saturating_mul(4);
    if src_stride == 0 || dst_stride == 0 {
        return;
    }
    let preserve_readback_row_order =
        OutputRotationModel::new(rotation).present_preserves_readback_row_order();
    for y in 0..height {
        let src_y = if src_flipped && !preserve_readback_row_order {
            height.saturating_sub(1).saturating_sub(y)
        } else {
            y
        };
        let src_start = src_y.saturating_mul(src_stride);
        let src_end = src_start.saturating_add(src_stride).min(src_pixels.len());
        let dst_start = y.saturating_mul(dst_stride);
        let dst_end = dst_start.saturating_add(src_stride).min(dst_pixels.len());
        if src_start >= src_end || dst_start >= dst_end {
            continue;
        }
        let copy_len = (src_end - src_start).min(dst_end - dst_start);
        dst_pixels[dst_start..dst_start + copy_len]
            .copy_from_slice(&src_pixels[src_start..src_start + copy_len]);
        for alpha in dst_pixels[dst_start + 3..dst_start + copy_len]
            .iter_mut()
            .step_by(4)
        {
            *alpha = 0x00;
        }
    }
}

fn select_connector_mode(modes: &[DrmMode]) -> Option<DrmMode> {
    modes.iter().copied().max_by_key(|mode| {
        let preferred = mode.mode_type().contains(ModeTypeFlags::PREFERRED) as u8;
        let (width, height) = mode.size();
        let area = u64::from(width) * u64::from(height);
        (preferred, area, mode.vrefresh(), width, height)
    })
}

fn host_device_sort_key(path: &Path, preferred_primary_path: Option<&Path>) -> (u8, String) {
    let preferred_rank = if preferred_primary_path == Some(path) {
        0
    } else {
        1
    };
    (preferred_rank, path.to_string_lossy().to_string())
}

fn select_primary_path<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    preferred_primary_path: Option<&Path>,
) -> Option<String> {
    let mut all_paths: Vec<&PathBuf> = paths.collect();
    if all_paths.is_empty() {
        return None;
    }
    if let Some(preferred) = preferred_primary_path {
        if let Some(path) = all_paths.iter().find(|path| path.as_path() == preferred) {
            return Some(path.to_string_lossy().to_string());
        }
    }
    all_paths.sort();
    all_paths
        .first()
        .map(|path| path.to_string_lossy().to_string())
}

struct RuntimeLoopData {
    shared_state: Arc<Mutex<CompositorState>>,
    display_handle: DisplayHandle,
    loop_signal: LoopSignal,
    wayland_state: RuntimeWaylandState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeSurfaceRole {
    MainApp,
    OverlayNative,
    NativePane(PaneId),
}

fn embedded_toplevel_decoration_mode() -> XdgDecorationMode {
    XdgDecorationMode::ServerSide
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SurfaceClassification {
    MainApp,
    NativePane { pane_id: PaneId, launch_pid: u32 },
    OverlayCandidate,
    PendingIdentity,
}

struct RuntimeWaylandState {
    shared_state: Arc<Mutex<CompositorState>>,
    display_handle: DisplayHandle,
    compositor_state: SmithayCompositorState,
    _output_manager_state: OutputManagerState,
    _data_device_state: DataDeviceState,
    _fractional_scale_manager_state: FractionalScaleManagerState,
    _viewporter_state: ViewporterState,
    output: Output,
    xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    shm_state: ShmState,
    dmabuf_state: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    dmabuf_main_device: Option<DrmNode>,
    dmabuf_formats: Vec<Format>,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    main_toplevel: Option<ToplevelSurface>,
    overlay_toplevel: Option<ToplevelSurface>,
    native_pane_toplevels: HashMap<PaneId, ToplevelSurface>,
    pending_toplevels: Vec<ToplevelSurface>,
    popups: Vec<ManagedPopup>,
    pointer_location: Point<f64, Logical>,
    pointer_location_initialized: bool,
    start_time: std::time::Instant,
    host_surface_buffers: HashMap<ObjectId, SurfaceBufferSnapshot>,
    surface_material_serials: HashMap<ObjectId, u64>,
    next_surface_material_serial: u64,
    backend_output_size: Size<i32, Physical>,
    applied_output_rotation: OutputRotation,
    applied_root_geometry_generation: u64,
    staged_root_geometry: Option<StagedRuntimeRootGeometry>,
    presentation_root_geometry: Option<StagedRuntimeRootGeometry>,
    active_root_geometry_consumers: Option<StagedRuntimeRootGeometry>,
    native_clip_program: Option<GlesTexProgram>,
    shell_overlay_toggle_shortcut: ShellOverlayToggleShortcut,
}

#[derive(Clone)]
struct StagedRuntimeRootGeometry {
    committed: crate::root_geometry::CommittedRootGeometry,
    output_global: StagedOutputGlobalState,
    root_layout: crate::root_geometry::ViewportProjection,
    composited_crops: Vec<crate::root_geometry::PhysicalPixelRect>,
    native_materializations: Vec<(PaneId, crate::root_geometry::NativeBufferProjection)>,
    viewports: Vec<(PaneId, crate::root_geometry::ViewportProjection)>,
    captures: Vec<(PaneId, crate::root_geometry::CaptureGeometry)>,
    input: crate::root_geometry::RootGeometrySnapshot,
    status: crate::root_geometry::DisplayScaleStatus,
    topology: crate::model::StatusSnapshot,
    accepted_material_identity: Option<AcceptedMaterialIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedSurfaceMaterial {
    id: ObjectId,
    commit_serial: u64,
    tree_offset: Point<i32, Logical>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedPopupMaterial {
    id: ObjectId,
    owner_role: RuntimeSurfaceRole,
    geometry: Rectangle<i32, Logical>,
    surfaces: Vec<AcceptedSurfaceMaterial>,
}

/// The exact surface material/topology accepted for one root4 presentation.
///
/// Geometry transactions may stage while clients continue committing, but the
/// first presentation attempt accepts one complete tree identity. A later
/// buffer, subsurface position/order, popup role/geometry, or tree membership
/// change therefore rejects that attempt instead of mixing two operations.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedMaterialIdentity {
    surfaces: Vec<AcceptedSurfaceMaterial>,
    popups: Vec<AcceptedPopupMaterial>,
}

#[derive(Clone, Copy)]
struct StagedOutputGlobalState {
    physical_width: i32,
    physical_height: i32,
    factor: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum Root4ConsumerStage {
    RootLayoutBackgroundAndChrome,
    CompositedContentOverlaysAndHitRegions,
    NativeBuffers,
    SurfaceAndPaneViewports,
    Capture,
    Input,
    Status,
}

impl StagedRuntimeRootGeometry {
    fn is_coherent(&self) -> bool {
        let generation = self.committed.snapshot.generation;
        self.root_layout.root_geometry_generation == generation
            && self.output_global.physical_width == self.committed.snapshot.physical_size_px.width
            && self.output_global.physical_height == self.committed.snapshot.physical_size_px.height
            && self.output_global.factor == self.committed.snapshot.factor
            && self.input.generation == generation
            && self.status.root_geometry_generation == generation
            && self
                .viewports
                .iter()
                .all(|(_, viewport)| viewport.root_geometry_generation == generation)
            && self
                .captures
                .iter()
                .all(|(_, capture)| capture.root_geometry_generation == generation)
            && self
                .composited_crops
                .iter()
                .all(|crop| crop.width >= 0 && crop.height >= 0)
            && self.native_materializations.iter().all(|(_, native)| {
                native.width_px > 0
                    && native.height_px > 0
                    && native.logical_clip.width > 0.0
                    && native.logical_clip.height > 0.0
            })
    }

    fn accept_or_validate_material_identity(&mut self, current: AcceptedMaterialIdentity) -> bool {
        match self.accepted_material_identity.as_ref() {
            Some(accepted) => accepted == &current,
            None => {
                self.accepted_material_identity = Some(current);
                true
            }
        }
    }
}

fn stage_runtime_root_geometry_consumers(
    state: &CompositorState,
    committed: crate::root_geometry::CommittedRootGeometry,
    #[cfg(test)] fail_at: Option<Root4ConsumerStage>,
) -> Result<StagedRuntimeRootGeometry, crate::root_geometry::RootGeometryError> {
    let snapshot = committed.snapshot;
    macro_rules! stage {
        ($consumer:expr, $expression:expr) => {{
            #[cfg(test)]
            if fail_at == Some($consumer) {
                return Err(crate::root_geometry::RootGeometryError::DisplayScaleApplyFailed);
            }
            $expression
        }};
    }
    let root_rect = crate::root_geometry::LogicalRect {
        x: 0.0,
        y: 0.0,
        width: snapshot.logical_size.width,
        height: snapshot.logical_size.height,
    };
    let output_global = stage!(
        Root4ConsumerStage::RootLayoutBackgroundAndChrome,
        StagedOutputGlobalState {
            physical_width: snapshot.physical_size_px.width,
            physical_height: snapshot.physical_size_px.height,
            factor: snapshot.factor,
        }
    );
    let root_layout = snapshot.viewport(root_rect);
    let mut status_snapshot = state.status_snapshot();
    if let Some(previous) = state.root_geometry_snapshot()
        && previous.rotation != snapshot.rotation
    {
        for pane in &mut status_snapshot.panes {
            if !matches!(
                pane.render_mode,
                crate::model::PaneRenderMode::ExternalNative { .. }
            ) {
                continue;
            }
            if let Some(rotated) = crate::state::rotate_pane_geometry_between_root_geometries(
                pane.geometry,
                previous,
                snapshot,
            ) {
                pane.geometry = rotated;
            }
        }
    }
    let composited_crops = stage!(
        Root4ConsumerStage::CompositedContentOverlaysAndHitRegions,
        status_snapshot
            .panes
            .iter()
            .map(|pane| {
                snapshot.physical_crop(crate::root_geometry::LogicalRect {
                    x: pane.geometry.x as f64,
                    y: pane.geometry.y as f64,
                    width: pane.geometry.width as f64,
                    height: pane.geometry.height as f64,
                })
            })
            .collect()
    );
    let native_materializations = stage!(
        Root4ConsumerStage::NativeBuffers,
        status_snapshot
            .panes
            .iter()
            .filter(|pane| matches!(
                pane.render_mode,
                crate::model::PaneRenderMode::ExternalNative { .. }
            ))
            .map(|pane| {
                let rect = crate::root_geometry::LogicalRect {
                    x: pane.geometry.x as f64,
                    y: pane.geometry.y as f64,
                    width: pane.geometry.width as f64,
                    height: pane.geometry.height as f64,
                };
                (pane.id.clone(), snapshot.native_buffer(rect))
            })
            .collect()
    );
    let viewports: Vec<(PaneId, crate::root_geometry::ViewportProjection)> = stage!(
        Root4ConsumerStage::SurfaceAndPaneViewports,
        status_snapshot
            .panes
            .iter()
            .map(|pane| {
                let rect = crate::root_geometry::LogicalRect {
                    x: pane.geometry.x as f64,
                    y: pane.geometry.y as f64,
                    width: pane.geometry.width as f64,
                    height: pane.geometry.height as f64,
                };
                (pane.id.clone(), snapshot.viewport(rect))
            })
            .collect()
    );
    for pane in &mut status_snapshot.panes {
        pane.viewport = viewports
            .iter()
            .find(|(pane_id, _)| pane_id == &pane.id)
            .map(|(_, viewport)| viewport.clone());
    }
    let captures = stage!(
        Root4ConsumerStage::Capture,
        status_snapshot
            .panes
            .iter()
            .map(|pane| {
                let rect = crate::root_geometry::LogicalRect {
                    x: pane.geometry.x as f64,
                    y: pane.geometry.y as f64,
                    width: pane.geometry.width as f64,
                    height: pane.geometry.height as f64,
                };
                (pane.id.clone(), snapshot.capture_geometry(rect))
            })
            .collect()
    );
    let input = stage!(Root4ConsumerStage::Input, committed.input.0);
    let status = stage!(Root4ConsumerStage::Status, committed.status.0.status());
    let staged = StagedRuntimeRootGeometry {
        committed,
        output_global,
        root_layout,
        composited_crops,
        native_materializations,
        viewports,
        captures,
        input,
        status,
        topology: status_snapshot,
        accepted_material_identity: None,
    };
    if staged.is_coherent() {
        Ok(staged)
    } else {
        Err(crate::root_geometry::RootGeometryError::DisplayScaleApplyFailed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderElementSource {
    MainOverlayRegion,
    Main,
    MainPopup,
    NativePane,
    NativePanePopup,
    OverlayRegionDebug,
    Overlay,
    OverlayPopup,
}

render_elements! {
    SurfAceRenderElement<=GlesRenderer>;
    Wayland=WaylandSurfaceRenderElement<GlesRenderer>,
    CroppedWayland=CropRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    NativeMaterialized=NativeMaterializedRenderElement,
    Solid=SolidColorRenderElement,
}

struct NativeMaterializedRenderElement {
    element: WaylandSurfaceRenderElement<GlesRenderer>,
    projection: crate::root_geometry::NativeBufferProjection,
    clip_program: GlesTexProgram,
    materialize_root: bool,
    target_height_px: f32,
}

fn materialize_native_surface_elements(
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    root_element_id: Option<&Id>,
    projection: crate::root_geometry::NativeBufferProjection,
    clip_program: &GlesTexProgram,
    target_height_px: f32,
) -> Vec<NativeMaterializedRenderElement> {
    elements
        .into_iter()
        .map(|element| NativeMaterializedRenderElement {
            materialize_root: root_element_id.is_some_and(|root| element.id() == root),
            element,
            projection,
            clip_program: clip_program.clone(),
            target_height_px,
        })
        .collect()
}

const NATIVE_CLIP_FRAGMENT_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
uniform vec2 clip_min;
uniform vec2 clip_max;
uniform float target_height;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec2 root_position = vec2(gl_FragCoord.x, target_height - gl_FragCoord.y);
    if (root_position.x < clip_min.x || root_position.y < clip_min.y ||
        root_position.x >= clip_max.x || root_position.y >= clip_max.y) {
        discard;
    }
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

fn compile_native_clip_program(
    renderer: &mut GlesRenderer,
) -> Result<GlesTexProgram, smithay::backend::renderer::gles::GlesError> {
    renderer.compile_custom_texture_shader(
        NATIVE_CLIP_FRAGMENT_SHADER,
        &[
            UniformName::new("clip_min", UniformType::_2f),
            UniformName::new("clip_max", UniformType::_2f),
            UniformName::new("target_height", UniformType::_1f),
        ],
    )
}

fn remap_damage_to_materialized_destination(
    damage: DamageSet<i32, Physical>,
    source_size: Size<i32, Physical>,
    destination_size: Size<i32, Physical>,
) -> DamageSet<i32, Physical> {
    if source_size.w <= 0
        || source_size.h <= 0
        || destination_size.w <= 0
        || destination_size.h <= 0
    {
        return DamageSet::default();
    }
    damage
        .iter()
        .filter_map(|rect| {
            remap_rect_to_materialized_destination(*rect, source_size, destination_size)
        })
        .collect()
}

fn remap_rect_to_materialized_destination(
    rect: Rectangle<i32, Physical>,
    source_size: Size<i32, Physical>,
    destination_size: Size<i32, Physical>,
) -> Option<Rectangle<i32, Physical>> {
    let x0 = (i64::from(rect.loc.x) * i64::from(destination_size.w))
        .div_euclid(i64::from(source_size.w));
    let y0 = (i64::from(rect.loc.y) * i64::from(destination_size.h))
        .div_euclid(i64::from(source_size.h));
    let x1_numerator = i64::from(rect.loc.x + rect.size.w) * i64::from(destination_size.w);
    let y1_numerator = i64::from(rect.loc.y + rect.size.h) * i64::from(destination_size.h);
    let x1 = x1_numerator.div_euclid(i64::from(source_size.w))
        + i64::from(x1_numerator.rem_euclid(i64::from(source_size.w)) != 0);
    let y1 = y1_numerator.div_euclid(i64::from(source_size.h))
        + i64::from(y1_numerator.rem_euclid(i64::from(source_size.h)) != 0);
    Rectangle::new(
        (i32::try_from(x0).ok()?, i32::try_from(y0).ok()?).into(),
        (i32::try_from(x1 - x0).ok()?, i32::try_from(y1 - y0).ok()?).into(),
    )
    .intersection(Rectangle::from_size(destination_size))
}

fn native_materialized_source_rect(
    projection: crate::root_geometry::NativeBufferProjection,
) -> Rectangle<f64, BufferCoords> {
    Rectangle::new(
        (0.0, 0.0).into(),
        (projection.width_px as f64, projection.height_px as f64).into(),
    )
}

fn native_materialized_destination_rect(
    projection: crate::root_geometry::NativeBufferProjection,
) -> Rectangle<i32, Physical> {
    Rectangle::new(
        (projection.origin_x, projection.origin_y).into(),
        (projection.width_px, projection.height_px).into(),
    )
}

fn native_materialized_local_clip(
    projection: crate::root_geometry::NativeBufferProjection,
    destination: Rectangle<i32, Physical>,
) -> Rectangle<i32, Physical> {
    let clip_min_x = projection.logical_clip.x * projection.scale_factor;
    let clip_min_y = projection.logical_clip.y * projection.scale_factor;
    let clip_max_x =
        (projection.logical_clip.x + projection.logical_clip.width) * projection.scale_factor;
    let clip_max_y =
        (projection.logical_clip.y + projection.logical_clip.height) * projection.scale_factor;
    let x0 = (clip_min_x - destination.loc.x as f64 - 0.5).ceil() as i32;
    let y0 = (clip_min_y - destination.loc.y as f64 - 0.5).ceil() as i32;
    let x1 = (clip_max_x - destination.loc.x as f64 - 0.5).ceil() as i32;
    let y1 = (clip_max_y - destination.loc.y as f64 - 0.5).ceil() as i32;
    Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
}

impl Element for NativeMaterializedRenderElement {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        if self.materialize_root {
            native_materialized_source_rect(self.projection)
        } else {
            self.element.src()
        }
    }

    fn transform(&self) -> Transform {
        self.element.transform()
    }

    fn geometry(&self, scale: SurfaceScale<f64>) -> Rectangle<i32, Physical> {
        if self.materialize_root {
            native_materialized_destination_rect(self.projection)
        } else {
            self.element.geometry(scale)
        }
    }

    fn damage_since(
        &self,
        scale: SurfaceScale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let damage = self.element.damage_since(scale, commit);
        let damage = if self.materialize_root {
            remap_damage_to_materialized_destination(
                damage,
                self.element.geometry(scale).size,
                native_materialized_destination_rect(self.projection).size,
            )
        } else {
            damage
        };
        let geometry = self.geometry(scale);
        let clip = native_materialized_local_clip(self.projection, geometry);
        damage
            .iter()
            .filter_map(|rect| rect.intersection(clip))
            .collect()
    }

    fn opaque_regions(&self, scale: SurfaceScale<f64>) -> OpaqueRegions<i32, Physical> {
        let geometry = self.geometry(scale);
        let clip = native_materialized_local_clip(self.projection, geometry);
        self.element
            .opaque_regions(scale)
            .iter()
            .filter_map(|rect| {
                if self.materialize_root {
                    remap_rect_to_materialized_destination(
                        *rect,
                        self.element.geometry(scale).size,
                        geometry.size,
                    )
                } else {
                    Some(*rect)
                }
            })
            .filter_map(|rect| rect.intersection(clip))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl RenderElement<GlesRenderer> for NativeMaterializedRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        let local_clip = native_materialized_local_clip(self.projection, dst);
        let clipped_damage: Vec<_> = damage
            .iter()
            .filter_map(|rect| rect.intersection(local_clip))
            .collect();
        if clipped_damage.is_empty() {
            return Ok(());
        }
        match self.element.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                let clip_min = (
                    (self.projection.logical_clip.x * self.projection.scale_factor) as f32,
                    (self.projection.logical_clip.y * self.projection.scale_factor) as f32,
                );
                let clip_max = (
                    ((self.projection.logical_clip.x + self.projection.logical_clip.width)
                        * self.projection.scale_factor) as f32,
                    ((self.projection.logical_clip.y + self.projection.logical_clip.height)
                        * self.projection.scale_factor) as f32,
                );
                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    &clipped_damage,
                    opaque_regions,
                    self.transform(),
                    self.alpha(),
                    Some(&self.clip_program),
                    &[
                        Uniform::new("clip_min", clip_min),
                        Uniform::new("clip_max", clip_max),
                        Uniform::new("target_height", self.target_height_px),
                    ],
                )
            }
            WaylandSurfaceTexture::SolidColor(_) => {
                self.element
                    .draw(frame, src, dst, &clipped_damage, opaque_regions)
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.element.underlying_storage(renderer)
    }
}

#[derive(Default)]
struct RenderElementCapture {
    elements: Vec<SurfAceRenderElement>,
    counts: RenderElementCounts,
    sources: Vec<RenderElementSource>,
    failure: Option<String>,
}

impl RenderElementCapture {
    fn push(
        &mut self,
        source: RenderElementSource,
        new_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    ) {
        self.push_elements(
            source,
            new_elements
                .into_iter()
                .map(SurfAceRenderElement::from)
                .collect(),
        );
    }

    fn push_elements(
        &mut self,
        source: RenderElementSource,
        new_elements: Vec<SurfAceRenderElement>,
    ) {
        let added = new_elements.len();
        if added == 0 {
            return;
        }
        self.elements.extend(new_elements);
        self.sources.extend(std::iter::repeat(source).take(added));
        match source {
            RenderElementSource::MainOverlayRegion => self.counts.main_overlay_regions += added,
            RenderElementSource::Main => self.counts.main += added,
            RenderElementSource::MainPopup => self.counts.main_popups += added,
            RenderElementSource::NativePane => self.counts.native_panes += added,
            RenderElementSource::NativePanePopup => self.counts.native_pane_popups += added,
            RenderElementSource::OverlayRegionDebug => self.counts.overlay_region_debug += added,
            RenderElementSource::Overlay => self.counts.overlay += added,
            RenderElementSource::OverlayPopup => self.counts.overlay_popups += added,
        }
    }

    fn primary_plane_elements(&self) -> Vec<&SurfAceRenderElement> {
        self.elements
            .iter()
            .zip(self.sources.iter())
            .filter_map(|(element, source)| {
                (!matches!(
                    source,
                    RenderElementSource::Overlay | RenderElementSource::OverlayPopup
                ))
                .then_some(element)
            })
            .collect()
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct RenderElementCounts {
    main_overlay_regions: usize,
    main: usize,
    main_popups: usize,
    native_panes: usize,
    native_pane_popups: usize,
    overlay_region_debug: usize,
    overlay: usize,
    overlay_popups: usize,
}

struct ManagedPopup {
    surface: PopupSurface,
    owner_role: RuntimeSurfaceRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceBufferKind {
    Shm,
    Dmabuf,
    Other,
}

#[derive(Clone, Copy)]
struct SurfaceDmabufInfo {
    width: i32,
    height: i32,
    format: Fourcc,
    modifier: Modifier,
}

struct SurfaceBufferSnapshot {
    buffer: wl_buffer::WlBuffer,
    kind: SurfaceBufferKind,
    size: Option<Size<i32, Logical>>,
    dmabuf: Option<SurfaceDmabufInfo>,
    native_materialization: Option<crate::root_geometry::NativeBufferProjection>,
    damage: Option<crate::root_geometry::PhysicalPixelRect>,
    root_geometry_generation: Option<u64>,
}

struct HostSceneSurface {
    buffer: wl_buffer::WlBuffer,
    kind: SurfaceBufferKind,
    target: Rectangle<i32, Logical>,
    dmabuf: Option<SurfaceDmabufInfo>,
    native_materialization: Option<crate::root_geometry::NativeBufferProjection>,
    damage: Option<crate::root_geometry::PhysicalPixelRect>,
}

#[derive(Default, Debug, Clone, Copy)]
struct HostSceneComposeStats {
    attempted_surfaces: u32,
    composed_surfaces: u32,
}

#[derive(Debug, Clone, Copy)]
struct RoleSurfaceMapping {
    origin: Point<f64, Logical>,
    scale: SurfaceScale<f64>,
}

fn renderer_surface_tree_bbox(
    surface: &WlSurface,
    location: impl Into<Point<i32, Logical>>,
) -> Rectangle<i32, Logical> {
    let mut bbox = Rectangle::new(location.into(), (0, 0).into());
    with_surface_tree_downward(
        surface,
        bbox.loc,
        |_, states, &loc| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            let mut next_loc = loc;
            if let Some(view) = data.and_then(|state| state.lock().ok()?.view()) {
                next_loc += view.offset;
                bbox = bbox.merge(Rectangle::new(next_loc, view.dst));
                TraversalAction::DoChildren(next_loc)
            } else {
                TraversalAction::SkipChildren
            }
        },
        |_, _, &_| {},
        |_, _, _| true,
    );
    bbox
}

fn source_rect_from_bbox_and_geometry(
    bbox: Rectangle<i32, Logical>,
    geometry: Option<Rectangle<i32, Logical>>,
) -> Rectangle<i32, Logical> {
    match geometry.filter(|geo| geo.size.w > 0 && geo.size.h > 0) {
        Some(geo) => geo,
        None => bbox,
    }
}

fn toplevel_surface_source_rect(surface: &ToplevelSurface) -> Rectangle<i32, Logical> {
    let bbox = renderer_surface_tree_bbox(surface.wl_surface(), (0, 0));
    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
        source_rect_from_bbox_and_geometry(
            bbox,
            states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry,
        )
    })
}

fn toplevel_identity(surface: &ToplevelSurface) -> (Option<String>, Option<String>) {
    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
        let attrs = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok());
        let app_id = attrs.as_ref().and_then(|attrs| attrs.app_id.clone());
        let title = attrs.as_ref().and_then(|attrs| attrs.title.clone());
        (app_id, title)
    })
}

fn pid_matches_or_descends_from(pid: u32, expected_ancestor: u32) -> bool {
    if pid == expected_ancestor {
        return true;
    }

    let mut current = pid;
    for _ in 0..64 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            return false;
        };
        let Some(close_paren) = stat.rfind(')') else {
            return false;
        };
        let mut fields = stat[close_paren + 1..].split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            return false;
        };
        if ppid == expected_ancestor {
            return true;
        }
        if ppid <= 1 || ppid == current {
            return false;
        }
        current = ppid;
    }

    false
}

fn process_env_value(pid: u32, key: &str) -> Result<Option<String>, std::io::Error> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ"))?;
    for entry in bytes.split(|byte| *byte == 0) {
        let Some(separator) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (name, value_with_separator) = entry.split_at(separator);
        let value = &value_with_separator[1..];
        if name == key.as_bytes() {
            return Ok(Some(String::from_utf8_lossy(value).into_owned()));
        }
    }
    Ok(None)
}

fn process_launch_token_matches(pid: u32, expected_token: &str) -> bool {
    matches!(
        process_env_value(pid, LAUNCH_TOKEN_ENV),
        Ok(Some(actual)) if actual == expected_token
    )
}

fn launch_token_evidence_for_pid(
    pid: u32,
    expected_token: Option<&str>,
) -> Option<LaunchTokenEvidence> {
    let expected_token = expected_token?;
    Some(match process_env_value(pid, LAUNCH_TOKEN_ENV) {
        Ok(Some(actual)) if actual == expected_token => LaunchTokenEvidence::Matched,
        Ok(Some(_)) => LaunchTokenEvidence::Mismatched,
        Ok(None) => LaunchTokenEvidence::Missing,
        Err(_) => LaunchTokenEvidence::Unavailable,
    })
}

fn rectangle_from_pane_geometry(geometry: PaneGeometry) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (geometry.x.floor() as i32, geometry.y.floor() as i32).into(),
        (
            geometry.width.ceil().max(1.0) as i32,
            geometry.height.ceil().max(1.0) as i32,
        )
            .into(),
    )
}

impl RoleSurfaceMapping {
    fn new(source_bbox: Rectangle<i32, Logical>, target_rect: Rectangle<i32, Logical>) -> Self {
        Self::new_logical(
            source_bbox,
            crate::root_geometry::LogicalRect {
                x: target_rect.loc.x as f64,
                y: target_rect.loc.y as f64,
                width: target_rect.size.w as f64,
                height: target_rect.size.h as f64,
            },
        )
    }

    fn new_logical(
        source_bbox: Rectangle<i32, Logical>,
        target_rect: crate::root_geometry::LogicalRect,
    ) -> Self {
        if source_bbox.size.w <= 0
            || source_bbox.size.h <= 0
            || target_rect.width <= 0.0
            || target_rect.height <= 0.0
        {
            return Self {
                origin: (target_rect.x, target_rect.y).into(),
                scale: 1.0.into(),
            };
        }

        let scale_x = target_rect.width / source_bbox.size.w as f64;
        let scale_y = target_rect.height / source_bbox.size.h as f64;
        Self {
            origin: (
                target_rect.x - source_bbox.loc.x as f64 * scale_x,
                target_rect.y - source_bbox.loc.y as f64 * scale_y,
            )
                .into(),
            scale: (scale_x, scale_y).into(),
        }
    }

    fn new_native_materialization(
        source_bbox: Rectangle<i32, Logical>,
        projection: crate::root_geometry::NativeBufferProjection,
    ) -> Self {
        let scale = 1.0 / projection.scale_factor;
        Self {
            origin: (
                projection.logical_clip.x
                    - projection.fractional_phase_x * scale
                    - source_bbox.loc.x as f64 * scale,
                projection.logical_clip.y
                    - projection.fractional_phase_y * scale
                    - source_bbox.loc.y as f64 * scale,
            )
                .into(),
            scale: (scale, scale).into(),
        }
    }

    fn map_point(self, source_point: Point<i32, Logical>) -> Point<i32, Logical> {
        Point::<f64, Logical>::from((
            self.origin.x + source_point.x as f64 * self.scale.x,
            self.origin.y + source_point.y as f64 * self.scale.y,
        ))
        .to_i32_round()
    }

    fn map_rect(self, source_rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        Rectangle::from_extremities(
            self.map_point(source_rect.loc),
            self.map_point(source_rect.loc + source_rect.size),
        )
    }

    fn focus_origin(self) -> Point<f64, Logical> {
        self.origin
    }

    fn render_element_location(self) -> Point<i32, Physical> {
        Point::from((self.origin.x.round() as i32, self.origin.y.round() as i32))
    }

    fn render_element_scale(self) -> SurfaceScale<f64> {
        self.scale
    }

    fn map_render_element_location(
        self,
        source_point: Point<i32, Logical>,
    ) -> Point<i32, Physical> {
        let mapped = self.map_point(source_point);
        Point::from((mapped.x, mapped.y))
    }
}

impl RuntimeWaylandState {
    fn has_dmabuf_surface_material(&self) -> bool {
        self.host_surface_buffers
            .values()
            .any(|snapshot| snapshot.dmabuf.is_some())
    }

    fn accepted_material_identity(&self) -> AcceptedMaterialIdentity {
        let mut surfaces = Vec::new();
        if let Some(main) = self.main_toplevel.as_ref() {
            self.collect_accepted_surface_tree(main.wl_surface(), &mut surfaces);
        }
        if let Some(overlay) = self.overlay_toplevel.as_ref() {
            self.collect_accepted_surface_tree(overlay.wl_surface(), &mut surfaces);
        }
        let mut native_roots = self.native_pane_toplevels.iter().collect::<Vec<_>>();
        native_roots.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (_, native) in native_roots {
            self.collect_accepted_surface_tree(native.wl_surface(), &mut surfaces);
        }

        let popups = self
            .popups
            .iter()
            .map(|popup| {
                let mut popup_surfaces = Vec::new();
                self.collect_accepted_surface_tree(popup.surface.wl_surface(), &mut popup_surfaces);
                AcceptedPopupMaterial {
                    id: surface_key(popup.surface.wl_surface()),
                    owner_role: popup.owner_role.clone(),
                    geometry: self.popup_geometry_local(&popup.surface),
                    surfaces: popup_surfaces,
                }
            })
            .collect();
        AcceptedMaterialIdentity { surfaces, popups }
    }

    fn collect_accepted_surface_tree(
        &self,
        root: &WlSurface,
        surfaces: &mut Vec<AcceptedSurfaceMaterial>,
    ) {
        with_surface_tree_downward(
            root,
            Point::<i32, Logical>::from((0, 0)),
            |_surface, data, &parent_offset| {
                let location = data
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location;
                TraversalAction::DoChildren(parent_offset + location)
            },
            |surface, _, &tree_offset| {
                let id = surface_key(surface);
                surfaces.push(AcceptedSurfaceMaterial {
                    commit_serial: self.surface_material_serials.get(&id).copied().unwrap_or(0),
                    id,
                    tree_offset,
                });
            },
            |_, _, _| true,
        );
    }

    fn staged_native_materializations_ready(&self) -> bool {
        let Some(staged) = self.staged_root_geometry.as_ref() else {
            return true;
        };
        let generation = staged.committed.snapshot.generation;
        staged.native_materializations.iter().all(|(pane_id, _)| {
            let Some(toplevel) = self.native_pane_toplevels.get(pane_id) else {
                return true;
            };
            self.host_surface_buffers
                .get(&surface_key(toplevel.wl_surface()))
                .is_some_and(|buffer| buffer.root_geometry_generation == Some(generation))
        })
    }

    fn root_geometry_projections(&self) -> Option<crate::root_geometry::CommittedRootGeometry> {
        if let Some(presentation) = self.presentation_root_geometry.as_ref() {
            return Some(presentation.committed);
        }
        if let Some(active) = self.active_root_geometry_consumers.as_ref() {
            return Some(active.committed);
        }
        lock_state(&self.shared_state).root_geometry_projections()
    }

    fn root_geometry_snapshot(&self) -> Option<crate::root_geometry::RootGeometrySnapshot> {
        self.root_geometry_projections()
            .map(|projections| projections.root_layout.0)
    }

    fn root_capture_snapshot(&self) -> Option<crate::root_geometry::RootGeometrySnapshot> {
        self.root_geometry_projections()
            .map(|projections| projections.captures.0)
    }

    fn root_pane_capture_geometries(&self) -> Vec<(PaneId, crate::root_geometry::CaptureGeometry)> {
        self.presentation_root_geometry
            .as_ref()
            .or(self.active_root_geometry_consumers.as_ref())
            .map(|geometry| geometry.captures.clone())
            .unwrap_or_default()
    }

    fn root_display_scale(&self) -> f64 {
        match self.root_geometry_projections() {
            Some(projections) => projections.composited_content.0.factor,
            None if lock_state(&self.shared_state)
                .status_snapshot()
                .host_mode_active =>
            {
                panic!("root4 host operation requires an activated geometry generation")
            }
            None => crate::root_geometry::DEFAULT_DISPLAY_SCALE_FACTOR,
        }
    }

    fn runtime_output_size(&self) -> Size<i32, Logical> {
        if let Some(snapshot) = self.root_geometry_snapshot() {
            let size = snapshot.logical_size_i32();
            return (size.width, size.height).into();
        }
        let state = lock_state(&self.shared_state);
        assert!(
            !state.status_snapshot().host_mode_active,
            "root4 host output size requires an activated geometry generation"
        );
        let width = state
            .status_snapshot()
            .runtime
            .window_width
            .unwrap_or(1280)
            .max(1);
        let height = state
            .status_snapshot()
            .runtime
            .window_height
            .unwrap_or(800)
            .max(1);
        let (width, height) =
            OutputRotationModel::new(state.output_rotation()).logical_size_i32(width, height);
        let factor = crate::root_geometry::DEFAULT_DISPLAY_SCALE_FACTOR;
        let width = (width as f64 / factor).floor().max(1.0) as i32;
        let height = (height as f64 / factor).floor().max(1.0) as i32;
        (width, height).into()
    }

    fn runtime_physical_output_size(&self) -> Size<i32, Physical> {
        let state = lock_state(&self.shared_state);
        let width = state
            .status_snapshot()
            .runtime
            .window_width
            .unwrap_or(1280)
            .max(1);
        let height = state
            .status_snapshot()
            .runtime
            .window_height
            .unwrap_or(800)
            .max(1);
        (width, height).into()
    }

    fn new(
        display_handle: DisplayHandle,
        shared_state: Arc<Mutex<CompositorState>>,
    ) -> Result<Self, crate::root_geometry::RootGeometryError> {
        Self::new_with_initial_root_geometry(display_handle, shared_state, None)
    }

    fn new_with_initial_root_geometry(
        display_handle: DisplayHandle,
        shared_state: Arc<Mutex<CompositorState>>,
        initial_root_geometry: Option<crate::root_geometry::CommittedRootGeometry>,
    ) -> Result<Self, crate::root_geometry::RootGeometryError> {
        let (
            backend_output_size,
            initial_pointer_output_size,
            applied_output_rotation,
            applied_root_geometry_generation,
            shell_overlay_toggle_shortcut,
        ) = {
            let state = lock_state(&shared_state);
            let width = state
                .status_snapshot()
                .runtime
                .window_width
                .unwrap_or(1280)
                .max(1);
            let height = state
                .status_snapshot()
                .runtime
                .window_height
                .unwrap_or(800)
                .max(1);
            let shell_overlay_toggle_shortcut = parse_shell_overlay_toggle_shortcut(
                &state
                    .status_snapshot()
                    .runtime
                    .shell_overlay_toggle_shortcut,
            )
            .unwrap_or_else(|_| {
                parse_shell_overlay_toggle_shortcut("Super+`")
                    .expect("default shell overlay shortcut must remain valid")
            });
            let applied_output_rotation = state.output_rotation();
            let initial_snapshot = initial_root_geometry
                .map(|committed| committed.snapshot)
                .or_else(|| state.root_geometry_snapshot());
            if state.status_snapshot().host_mode_active && initial_snapshot.is_none() {
                return Err(crate::root_geometry::RootGeometryError::DisplayScaleApplyFailed);
            }
            let (logical_width, logical_height) = initial_snapshot
                .map(|snapshot| {
                    let size = snapshot.logical_size_i32();
                    (size.width, size.height)
                })
                .unwrap_or_else(|| {
                    let (width, height) = OutputRotationModel::new(applied_output_rotation)
                        .logical_size_i32(width, height);
                    (
                        (width as f64 / crate::root_geometry::DEFAULT_DISPLAY_SCALE_FACTOR)
                            .floor()
                            .max(1.0) as i32,
                        (height as f64 / crate::root_geometry::DEFAULT_DISPLAY_SCALE_FACTOR)
                            .floor()
                            .max(1.0) as i32,
                    )
                });
            (
                Size::<i32, Physical>::from((width, height)),
                Size::<i32, Logical>::from((logical_width, logical_height)),
                applied_output_rotation,
                initial_snapshot
                    .map(|snapshot| snapshot.generation)
                    .unwrap_or(0),
                shell_overlay_toggle_shortcut,
            )
        };
        let active_root_geometry_consumers = {
            let state = lock_state(&shared_state);
            initial_root_geometry
                .or_else(|| state.root_geometry_projections())
                .map(|committed| {
                    #[cfg(test)]
                    {
                        stage_runtime_root_geometry_consumers(&state, committed, None)
                    }
                    #[cfg(not(test))]
                    {
                        stage_runtime_root_geometry_consumers(&state, committed)
                    }
                })
                .transpose()?
        };
        if let Some(prepared) = initial_root_geometry {
            let consumers = active_root_geometry_consumers
                .as_ref()
                .expect("prepared root4 geometry must stage consumers");
            lock_state(&shared_state).commit_root4_geometry_consumers(
                prepared,
                consumers.root_layout.clone(),
                &consumers.viewports,
            );
        }
        let compositor_state = SmithayCompositorState::new::<Self>(&display_handle);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let fractional_scale_manager_state =
            FractionalScaleManagerState::new::<Self>(&display_handle);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let output = Output::new(
            "surf-ace-output-0".to_string(),
            PhysicalProperties {
                size: (600, 340).into(),
                subpixel: Subpixel::Unknown,
                make: "Surf Ace".to_string(),
                model: "Host Output".to_string(),
            },
        );
        let _ = output.create_global::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let dmabuf_state = DmabufState::new();
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "winit");
        let _ = seat.add_keyboard(Default::default(), 200, 25);
        seat.add_pointer();

        let state = Self {
            shared_state,
            display_handle,
            compositor_state,
            _output_manager_state: output_manager_state,
            _data_device_state: data_device_state,
            _fractional_scale_manager_state: fractional_scale_manager_state,
            _viewporter_state: viewporter_state,
            output,
            xdg_shell_state,
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            dmabuf_state,
            dmabuf_global: None,
            dmabuf_main_device: None,
            dmabuf_formats: Vec::new(),
            seat_state,
            seat,
            main_toplevel: None,
            overlay_toplevel: None,
            native_pane_toplevels: HashMap::new(),
            pending_toplevels: Vec::new(),
            popups: Vec::new(),
            pointer_location: software_cursor_default_location(initial_pointer_output_size),
            pointer_location_initialized: false,
            start_time: std::time::Instant::now(),
            host_surface_buffers: HashMap::new(),
            surface_material_serials: HashMap::new(),
            next_surface_material_serial: 0,
            backend_output_size,
            applied_output_rotation,
            applied_root_geometry_generation,
            staged_root_geometry: None,
            presentation_root_geometry: None,
            active_root_geometry_consumers,
            native_clip_program: None,
            shell_overlay_toggle_shortcut,
        };
        state.sync_output_state();
        state.sync_runtime_dmabuf_protocol_status();
        Ok(state)
    }

    fn sync_output_state(&self) {
        let size = self
            .root_geometry_snapshot()
            .map(|snapshot| snapshot.physical_size_px)
            .map(|size| Size::<i32, Physical>::from((size.width, size.height)))
            .unwrap_or(self.backend_output_size);
        let mode = OutputMode {
            size: (size.w, size.h).into(),
            refresh: 60_000,
        };
        self.output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(OutputScale::Fractional(self.root_display_scale())),
            Some((0, 0).into()),
        );
        self.output.set_preferred(mode);
    }

    fn activate_output_global(&self, staged: StagedOutputGlobalState) {
        let mode = OutputMode {
            size: (staged.physical_width, staged.physical_height).into(),
            refresh: 60_000,
        };
        self.output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(OutputScale::Fractional(staged.factor)),
            Some((0, 0).into()),
        );
        self.output.set_preferred(mode);
    }

    fn sync_runtime_dmabuf_protocol_status(&self) {
        let formats = self
            .dmabuf_formats
            .iter()
            .map(|format| RuntimeDmabufFormatStatus {
                code: format.code as u32,
                modifier: format.modifier.into(),
            })
            .collect();
        let mut state = lock_state(&self.shared_state);
        state.set_runtime_dmabuf_protocol_formats(formats);
    }

    fn sync_dmabuf_protocol_formats(&mut self, advertisement: Option<(DrmNode, Vec<Format>)>) {
        let Some((main_device, formats)) = advertisement else {
            if let Some(global) = self.dmabuf_global.take() {
                self.dmabuf_state
                    .disable_global::<Self>(&self.display_handle, &global);
                self.dmabuf_state
                    .destroy_global::<Self>(&self.display_handle, global);
            }
            self.dmabuf_main_device = None;
            self.dmabuf_formats.clear();
            self.sync_runtime_dmabuf_protocol_status();
            return;
        };

        if formats.is_empty() {
            if let Some(global) = self.dmabuf_global.take() {
                self.dmabuf_state
                    .disable_global::<Self>(&self.display_handle, &global);
                self.dmabuf_state
                    .destroy_global::<Self>(&self.display_handle, global);
            }
            self.dmabuf_main_device = None;
            self.dmabuf_formats = formats;
            self.sync_runtime_dmabuf_protocol_status();
            return;
        }

        let default_feedback = match DmabufFeedbackBuilder::new(
            main_device.dev_id(),
            formats.iter().copied(),
        )
        .build()
        {
            Ok(default_feedback) => default_feedback,
            Err(err) => {
                eprintln!(
                    "host dmabuf protocol advertisement disabled: failed to build default feedback for {}: {err}",
                    main_device
                        .dev_path()
                        .unwrap_or_else(|| PathBuf::from("<unknown-drm-node>"))
                        .display()
                );
                if let Some(global) = self.dmabuf_global.take() {
                    self.dmabuf_state
                        .disable_global::<Self>(&self.display_handle, &global);
                    self.dmabuf_state
                        .destroy_global::<Self>(&self.display_handle, global);
                }
                self.dmabuf_main_device = None;
                self.dmabuf_formats.clear();
                self.sync_runtime_dmabuf_protocol_status();
                return;
            }
        };

        if formats == self.dmabuf_formats && self.dmabuf_main_device == Some(main_device) {
            self.sync_runtime_dmabuf_protocol_status();
            return;
        }

        if formats == self.dmabuf_formats {
            if let Some(global) = self.dmabuf_global.as_ref() {
                self.dmabuf_state
                    .set_default_feedback(global, &default_feedback);
            } else {
                self.dmabuf_global = Some(
                    self.dmabuf_state
                        .create_global_with_default_feedback::<Self>(
                            &self.display_handle,
                            &default_feedback,
                        ),
                );
            }
            self.dmabuf_main_device = Some(main_device);
            self.sync_runtime_dmabuf_protocol_status();
            return;
        }

        if let Some(global) = self.dmabuf_global.take() {
            self.dmabuf_state
                .disable_global::<Self>(&self.display_handle, &global);
            self.dmabuf_state
                .destroy_global::<Self>(&self.display_handle, global);
        }
        self.dmabuf_global = Some(
            self.dmabuf_state
                .create_global_with_default_feedback::<Self>(
                    &self.display_handle,
                    &default_feedback,
                ),
        );
        self.dmabuf_main_device = Some(main_device);
        self.dmabuf_formats = formats;
        self.sync_runtime_dmabuf_protocol_status();
    }

    fn forward_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        self.process_input_event(&event);
    }

    fn process_input_event<B: InputBackend>(&mut self, event: &InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                self.apply_focus_route();
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(keyboard) = self.seat.get_keyboard() {
                    let intercepted = keyboard.input::<bool, _>(
                        self,
                        event.key_code(),
                        event.state(),
                        serial,
                        event.time_msec(),
                        |data, modifiers, handle| {
                            let raw_syms = handle.raw_syms();
                            let is_pressed =
                                event.state() == smithay::backend::input::KeyState::Pressed;
                            let matched = data
                                .shell_overlay_toggle_shortcut
                                .matches(modifiers, &raw_syms);
                            if is_pressed {
                                eprintln!(
                                    "shell overlay shortcut check: key_code={:?} state={:?} shortcut={} modifiers={} raw_syms=[{}] matched={}",
                                    event.key_code(),
                                    event.state(),
                                    data.shell_overlay_toggle_shortcut.display_string(),
                                    format_shell_overlay_modifiers(modifiers),
                                    format_shell_overlay_raw_syms(&raw_syms),
                                    matched,
                                );
                            }
                            if is_pressed && matched {
                                FilterResult::Intercept(true)
                            } else {
                                FilterResult::Forward
                            }
                        },
                    );
                    if intercepted == Some(true) {
                        self.handle_shell_overlay_toggle();
                        return;
                    }
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                let delta = event.delta();
                let (snapshot, status) = self.input_operation_snapshot();
                let (dx, dy) = snapshot
                    .map(|snapshot| {
                        let (dx, dy) = OutputRotationModel::new(snapshot.rotation)
                            .physical_delta_to_logical(delta.x, delta.y);
                        (dx / snapshot.factor, dy / snapshot.factor)
                    })
                    .unwrap_or((delta.x, delta.y));
                let pos = self.update_pointer_location(
                    (self.pointer_location.x + dx, self.pointer_location.y + dy).into(),
                );
                let serial = SERIAL_COUNTER.next_serial();

                let under = self.surface_under_point_for_capture(
                    pos,
                    OverlayCaptureCapability::PointerHover,
                    &status,
                );
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.motion(
                        self,
                        under,
                        &MotionEvent {
                            location: pos,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let (snapshot, status) = self.input_operation_snapshot();
                let physical_size = snapshot
                    .map(|snapshot| snapshot.physical_size_px)
                    .map(|size| Size::<i32, Physical>::from((size.width, size.height)))
                    .unwrap_or_else(|| self.runtime_physical_output_size());
                let physical_pos =
                    event.position_transformed((physical_size.w, physical_size.h).into());
                let logical_pos = snapshot
                    .map(|snapshot| snapshot.physical_to_logical(physical_pos.x, physical_pos.y))
                    .map(Point::from)
                    .unwrap_or_else(|| self.map_physical_pointer_point_to_logical(physical_pos));
                let pos = self.update_pointer_location(logical_pos);
                let serial = SERIAL_COUNTER.next_serial();

                let under = self.surface_under_point_for_capture(
                    pos,
                    OverlayCaptureCapability::PointerHover,
                    &status,
                );
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.motion(
                        self,
                        under,
                        &MotionEvent {
                            location: pos,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerButton { event, .. } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                        let (_, status) = self.input_operation_snapshot();
                        let surface_under = self.surface_under_point_for_capture(
                            self.pointer_location,
                            OverlayCaptureCapability::PointerButton,
                            &status,
                        );
                        pointer.motion(
                            self,
                            surface_under.clone(),
                            &MotionEvent {
                                location: self.pointer_location,
                                serial,
                                time: event.time_msec(),
                            },
                        );
                        let focus_target =
                            surface_under.as_ref().map(|(surface, _)| surface.clone());
                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, focus_target, serial);
                        }
                    }
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button: event.button_code(),
                            state: event.state(),
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerAxis { event, .. } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    let (_, status) = self.input_operation_snapshot();
                    let under = self.surface_under_point_for_capture(
                        self.pointer_location,
                        OverlayCaptureCapability::PointerAxis,
                        &status,
                    );
                    pointer.motion(
                        self,
                        under,
                        &MotionEvent {
                            location: self.pointer_location,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    let source = event.source();
                    let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                        event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0
                    });
                    let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                        event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                    });

                    let mut frame = AxisFrame::new(event.time_msec()).source(source);
                    if horizontal_amount != 0.0 {
                        frame = frame.value(Axis::Horizontal, horizontal_amount);
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.value(Axis::Vertical, vertical_amount);
                    }
                    if source == AxisSource::Finger {
                        if event.amount(Axis::Horizontal) == Some(0.0) {
                            frame = frame.stop(Axis::Horizontal);
                        }
                        if event.amount(Axis::Vertical) == Some(0.0) {
                            frame = frame.stop(Axis::Vertical);
                        }
                    }
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            _ => {}
        }
    }

    fn apply_focus_route(&mut self) {
        let (requested_target, shell_overlay_focus_requested) = {
            let state = lock_state(&self.shared_state);
            (
                state.status_snapshot().runtime.active_focus_target,
                state.shell_overlay_focus_requested(),
            )
        };
        let resolved = shell_overlay_focus_requested
            .then(|| {
                self.overlay_toplevel.as_ref().map(|surface| {
                    (
                        RuntimeFocusTarget::OverlayNative,
                        surface.wl_surface().clone(),
                    )
                })
            })
            .flatten()
            .or_else(|| match requested_target.clone() {
                Some(RuntimeFocusTarget::MainApp) => self
                    .main_toplevel
                    .as_ref()
                    .map(|surface| (RuntimeFocusTarget::MainApp, surface.wl_surface().clone())),
                Some(RuntimeFocusTarget::OverlayNative) => {
                    self.overlay_toplevel.as_ref().map(|surface| {
                        (
                            RuntimeFocusTarget::OverlayNative,
                            surface.wl_surface().clone(),
                        )
                    })
                }
                Some(RuntimeFocusTarget::NativePane { pane_id }) => {
                    self.native_pane_toplevels.get(&pane_id).map(|surface| {
                        (
                            RuntimeFocusTarget::NativePane { pane_id },
                            surface.wl_surface().clone(),
                        )
                    })
                }
                None => None,
            })
            .or_else(|| {
                self.overlay_toplevel.as_ref().map(|surface| {
                    (
                        RuntimeFocusTarget::OverlayNative,
                        surface.wl_surface().clone(),
                    )
                })
            })
            .or_else(|| {
                self.main_toplevel
                    .as_ref()
                    .map(|surface| (RuntimeFocusTarget::MainApp, surface.wl_surface().clone()))
            });

        let focus_surface = resolved.as_ref().map(|(_, surface)| surface.clone());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, focus_surface, 0.into());
        }

        let resolved_target = resolved.map(|(target, _)| target);
        if requested_target != resolved_target || shell_overlay_focus_requested {
            let mut state = lock_state(&self.shared_state);
            if shell_overlay_focus_requested
                && resolved_target == Some(RuntimeFocusTarget::OverlayNative)
            {
                state.mark_shell_overlay_focus_applied();
            }
            state.set_runtime_focus_target(resolved_target);
        }
    }

    fn handle_shell_overlay_toggle(&mut self) {
        let (before, result, after) = {
            let mut state = lock_state(&self.shared_state);
            let before = state.status_snapshot();
            let result = state.toggle_shell_overlay();
            let after = state.status_snapshot();
            (before, result, after)
        };
        match &result {
            Ok(()) => eprintln!(
                "shell overlay toggle result: ok shortcut={} focus_before={:?} focus_after={:?} active_overlay_before={:?} active_overlay_after={:?} overlay_surface_before={:?} overlay_surface_after={:?} overlay_bound_pane_before={:?} overlay_bound_pane_after={:?}",
                before.runtime.shell_overlay_toggle_shortcut,
                before.runtime.active_focus_target,
                after.runtime.active_focus_target,
                before.overlay_role_policy.active_overlay_pane,
                after.overlay_role_policy.active_overlay_pane,
                before.runtime.overlay_surface_id,
                after.runtime.overlay_surface_id,
                before.runtime.overlay_bound_pane_id,
                after.runtime.overlay_bound_pane_id,
            ),
            Err(err) => eprintln!(
                "shell overlay toggle result: err={err} shortcut={} focus_before={:?} focus_after={:?} active_overlay_before={:?} active_overlay_after={:?} overlay_surface_before={:?} overlay_surface_after={:?} overlay_bound_pane_before={:?} overlay_bound_pane_after={:?}",
                before.runtime.shell_overlay_toggle_shortcut,
                before.runtime.active_focus_target,
                after.runtime.active_focus_target,
                before.overlay_role_policy.active_overlay_pane,
                after.overlay_role_policy.active_overlay_pane,
                before.runtime.overlay_surface_id,
                after.runtime.overlay_surface_id,
                before.runtime.overlay_bound_pane_id,
                after.runtime.overlay_bound_pane_id,
            ),
        }
        if let Err(err) = result {
            eprintln!("shell overlay toggle failed: {err}");
            return;
        }
        self.enforce_overlay_binding_policy();
        self.sync_runtime_status_with_roles();
        self.apply_focus_route();
    }

    fn assign_toplevel_role(&mut self, surface: ToplevelSurface) {
        self.enforce_main_app_binding_policy();
        match self.classify_toplevel(&surface) {
            SurfaceClassification::MainApp => self.assign_main_role(surface),
            SurfaceClassification::NativePane {
                pane_id,
                launch_pid,
            } => self.assign_native_pane_role(surface, pane_id, launch_pid),
            SurfaceClassification::OverlayCandidate => self.assign_overlay_role_or_queue(surface),
            SurfaceClassification::PendingIdentity => self.pending_toplevels.push(surface),
        }
    }

    fn assign_main_role(&mut self, surface: ToplevelSurface) {
        if self.main_toplevel.is_none() {
            let Some(client_pid) = self.client_pid_for_toplevel(&surface) else {
                surface.send_close();
                let mut state = lock_state(&self.shared_state);
                state.increment_runtime_denied_toplevel();
                return;
            };
            let evidence = self.main_app_binding_evidence_for_surface(&surface);
            let launch_pid = self.expected_main_app_client_pid().unwrap_or(client_pid);
            self.configure_toplevel_for_role(&surface, RuntimeSurfaceRole::MainApp);
            self.main_toplevel = Some(surface);
            self.bridge_main_app_surface_attached(launch_pid, client_pid, evidence);
            self.promote_pending_toplevels();
            self.sync_runtime_status_with_roles();
            self.apply_focus_route();
        } else {
            if self
                .main_toplevel
                .as_ref()
                .map(|main| !same_surface(main.wl_surface(), surface.wl_surface()))
                .unwrap_or(true)
            {
                surface.send_close();
                let mut state = lock_state(&self.shared_state);
                state.increment_runtime_denied_toplevel();
            }
        }
    }

    fn assign_native_pane_role(
        &mut self,
        surface: ToplevelSurface,
        pane_id: PaneId,
        launch_pid: u32,
    ) {
        let Some(client_pid) = self.client_pid_for_toplevel(&surface) else {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
            return;
        };
        if self
            .native_pane_toplevels
            .get(&pane_id)
            .map(|existing| !same_surface(existing.wl_surface(), surface.wl_surface()))
            .unwrap_or(false)
        {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
            return;
        }

        let evidence = self.native_pane_binding_evidence_for_surface(&surface, launch_pid);
        self.configure_toplevel_for_role(&surface, RuntimeSurfaceRole::NativePane(pane_id.clone()));
        let surface_id = surface_id(surface.wl_surface());
        self.native_pane_toplevels.insert(pane_id.clone(), surface);
        self.bridge_native_pane_surface_attached(
            launch_pid,
            client_pid,
            Some(surface_id),
            evidence,
        );
        self.bridge_native_pane_window_group_observed(
            &pane_id,
            surface_id.to_string(),
            Some(surface_id),
        );
        self.promote_pending_toplevels();
        self.sync_runtime_status_with_roles();
        self.apply_focus_route();
    }

    fn assign_overlay_role_or_queue(&mut self, surface: ToplevelSurface) {
        let Some(expected_pid) = self.expected_overlay_client_pid() else {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
            return;
        };
        let Some(client_pid) = self.client_pid_for_toplevel(&surface) else {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
            return;
        };
        if client_pid != expected_pid {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
            return;
        }
        if self.overlay_toplevel.is_none() {
            self.configure_toplevel_for_role(&surface, RuntimeSurfaceRole::OverlayNative);
            self.overlay_toplevel = Some(surface);
            self.bridge_overlay_surface_attached(client_pid);
            self.sync_runtime_status_with_roles();
            self.apply_focus_route();
        } else {
            surface.send_close();
            let mut state = lock_state(&self.shared_state);
            state.increment_runtime_denied_toplevel();
        }
    }

    fn promote_pending_toplevels(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_toplevels);
        for surface in pending.drain(..) {
            match self.classify_toplevel(&surface) {
                SurfaceClassification::MainApp => self.assign_main_role(surface),
                SurfaceClassification::NativePane {
                    pane_id,
                    launch_pid,
                } => self.assign_native_pane_role(surface, pane_id, launch_pid),
                SurfaceClassification::OverlayCandidate => {
                    self.assign_overlay_role_or_queue(surface)
                }
                SurfaceClassification::PendingIdentity => self.pending_toplevels.push(surface),
            }
        }
    }

    fn main_app_binding_evidence_for_surface(
        &self,
        surface: &ToplevelSurface,
    ) -> Option<SurfaceBindingEvidence> {
        let expectation =
            lock_state(&self.shared_state).runtime_expected_main_app_binding_with_token()?;
        let (app_id, title) = toplevel_identity(surface);
        let outcome = SurfaceBindingEvidenceOutcome::from(
            expectation
                .binding
                .match_identity(app_id.as_deref(), title.as_deref()),
        );
        let launch_token = self.client_pid_for_toplevel(surface).and_then(|pid| {
            launch_token_evidence_for_pid(pid, expectation.launch_token.as_deref())
        });
        Some(SurfaceBindingEvidence {
            app_id,
            title,
            launch_token,
            outcome,
        })
    }

    fn native_pane_binding_evidence_for_surface(
        &self,
        surface: &ToplevelSurface,
        launch_pid: u32,
    ) -> Option<SurfaceBindingEvidence> {
        let expected_token = lock_state(&self.shared_state)
            .runtime_expected_native_pane_bindings()
            .into_iter()
            .find(|expectation| expectation.pid == launch_pid)
            .and_then(|expectation| expectation.launch_token);
        let (app_id, title) = toplevel_identity(surface);
        let launch_token = self
            .client_pid_for_toplevel(surface)
            .and_then(|pid| launch_token_evidence_for_pid(pid, expected_token.as_deref()));
        Some(SurfaceBindingEvidence {
            app_id,
            title,
            launch_token,
            outcome: SurfaceBindingEvidenceOutcome::NotRequired,
        })
    }

    fn classify_toplevel(&self, surface: &ToplevelSurface) -> SurfaceClassification {
        if let Some(expectation) = self.expected_main_app_binding() {
            match self.client_pid_for_toplevel(surface) {
                Some(client_pid) if pid_matches_or_descends_from(client_pid, expectation.pid) => {
                    return SurfaceClassification::MainApp;
                }
                Some(client_pid)
                    if expectation
                        .launch_token
                        .as_deref()
                        .is_some_and(|token| process_launch_token_matches(client_pid, token)) =>
                {
                    return SurfaceClassification::MainApp;
                }
                Some(_) | None => {}
            }
        }
        if let Some((pane_id, launch_pid)) = self.expected_native_pane_for_toplevel(surface) {
            return SurfaceClassification::NativePane {
                pane_id,
                launch_pid,
            };
        }

        let (app_id, title) = toplevel_identity(surface);
        if app_id.is_none() && title.is_none() {
            return SurfaceClassification::PendingIdentity;
        }

        SurfaceClassification::OverlayCandidate
    }

    fn main_surface_matches_current_contract(&self, surface: &ToplevelSurface) -> bool {
        let Some(expectation) = self.expected_main_app_binding() else {
            return false;
        };
        let Some(client_pid) = self.client_pid_for_toplevel(surface) else {
            return false;
        };
        pid_matches_or_descends_from(client_pid, expectation.pid)
            || expectation
                .launch_token
                .as_deref()
                .is_some_and(|token| process_launch_token_matches(client_pid, token))
    }

    fn enforce_main_app_binding_policy(&mut self) {
        let should_clear = self
            .main_toplevel
            .as_ref()
            .map(|surface| !self.main_surface_matches_current_contract(surface))
            .unwrap_or(false);
        if !should_clear {
            return;
        }

        if let Some(main) = self.main_toplevel.as_ref() {
            if let Some(pid) = self
                .client_pid_for_toplevel(main)
                .or_else(|| self.expected_main_app_client_pid())
            {
                self.bridge_main_app_surface_detached(pid);
            }
            main.send_close();
        }
        self.main_toplevel = None;
    }

    fn runtime_output_width(&self) -> i32 {
        self.runtime_output_size().w
    }

    fn runtime_output_height(&self) -> i32 {
        self.runtime_output_size().h
    }

    fn map_physical_pointer_point_to_logical(
        &self,
        point: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        let Some(snapshot) = self.root_geometry_snapshot() else {
            return point;
        };
        let (x, y) = snapshot.physical_to_logical(point.x, point.y);
        (x, y).into()
    }

    fn update_pointer_location(&mut self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let pos = clamp_pointer_location(pos, self.runtime_output_size());
        self.pointer_location = pos;
        self.pointer_location_initialized = true;
        pos
    }

    fn cursor_render_location(&self) -> Point<f64, Logical> {
        if self.pointer_location_initialized {
            clamp_pointer_location(self.pointer_location, self.runtime_output_size())
        } else {
            software_cursor_default_location(self.runtime_output_size())
        }
    }

    fn overlay_rect(&self) -> Rectangle<i32, Logical> {
        let output_w = self.runtime_output_width();
        let output_h = self.runtime_output_height();
        let desired_w = (output_w / 2).max(480);
        let desired_h = (output_h / 2).max(320);
        let overlay_x = (output_w - desired_w - 16).max(0);
        let overlay_y = 16.min(output_h.saturating_sub(1));
        let available_w = (output_w - overlay_x).max(1);
        let available_h = (output_h - overlay_y).max(1);
        let overlay_w = desired_w.min(available_w);
        let overlay_h = desired_h.min(available_h);
        Rectangle::new((overlay_x, overlay_y).into(), (overlay_w, overlay_h).into())
    }

    fn native_pane_rect(&self, pane_id: &PaneId) -> Option<Rectangle<i32, Logical>> {
        let geometry = lock_state(&self.shared_state)
            .status_snapshot()
            .panes
            .into_iter()
            .find(|pane| &pane.id == pane_id)
            .map(|pane| pane.geometry)?;
        Some(rectangle_from_pane_geometry(geometry))
    }

    fn input_operation_snapshot(
        &self,
    ) -> (
        Option<crate::root_geometry::RootGeometrySnapshot>,
        crate::model::StatusSnapshot,
    ) {
        let state = lock_state(&self.shared_state);
        (
            state
                .root_geometry_projections()
                .map(|projections| projections.input.0),
            state.status_snapshot(),
        )
    }

    fn surface_under_point_for_capture(
        &self,
        pos: Point<f64, Logical>,
        capture: OverlayCaptureCapability,
        status: &crate::model::StatusSnapshot,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let pane_rect = |pane_id: &PaneId| {
            status
                .panes
                .iter()
                .find(|pane| &pane.id == pane_id)
                .map(|pane| rectangle_from_pane_geometry(pane.geometry))
        };
        let output_size = self.runtime_output_size();
        for popup in self.popups.iter().rev() {
            let local = self.popup_geometry_local(&popup.surface);
            let base = match popup.owner_role {
                RuntimeSurfaceRole::MainApp => Point::from((0, 0)),
                RuntimeSurfaceRole::OverlayNative => self.overlay_rect().loc,
                RuntimeSurfaceRole::NativePane(ref pane_id) => pane_rect(pane_id)
                    .map(|rect| rect.loc)
                    .unwrap_or_else(|| Point::from((0, 0))),
            };
            let popup_geometry = Rectangle::new(
                (base.x + local.loc.x, base.y + local.loc.y).into(),
                local.size,
            );
            let hit = pos.x >= popup_geometry.loc.x as f64
                && pos.x < (popup_geometry.loc.x + popup_geometry.size.w) as f64
                && pos.y >= popup_geometry.loc.y as f64
                && pos.y < (popup_geometry.loc.y + popup_geometry.size.h) as f64;
            if hit {
                return Some((
                    popup.surface.wl_surface().clone(),
                    popup_geometry.loc.to_f64(),
                ));
            }
        }

        let overlay_rect = self.overlay_rect();
        let overlay_hit = pos.x >= overlay_rect.loc.x as f64
            && pos.x < (overlay_rect.loc.x + overlay_rect.size.w) as f64
            && pos.y >= overlay_rect.loc.y as f64
            && pos.y < (overlay_rect.loc.y + overlay_rect.size.h) as f64;
        if overlay_hit {
            if let Some(overlay) = &self.overlay_toplevel {
                let focus_origin = self
                    .role_surface_mapping(RuntimeSurfaceRole::OverlayNative, overlay_rect)
                    .map(RoleSurfaceMapping::focus_origin)
                    .unwrap_or_else(|| overlay_rect.loc.to_f64());
                return Some((overlay.wl_surface().clone(), focus_origin));
            }
        }

        if overlay_region_capture_contains(&status.overlay_regions.regions, pos, capture) {
            if let Some(main) = &self.main_toplevel {
                let output_rect =
                    Rectangle::new((0, 0).into(), (output_size.w, output_size.h).into());
                let focus_origin = self
                    .role_surface_mapping(RuntimeSurfaceRole::MainApp, output_rect)
                    .map(RoleSurfaceMapping::focus_origin)
                    .unwrap_or_else(|| output_rect.loc.to_f64());
                return Some((main.wl_surface().clone(), focus_origin));
            }
        }

        for (pane_id, native) in &self.native_pane_toplevels {
            let Some(rect) = pane_rect(pane_id) else {
                continue;
            };
            let hit = pos.x >= rect.loc.x as f64
                && pos.x < (rect.loc.x + rect.size.w) as f64
                && pos.y >= rect.loc.y as f64
                && pos.y < (rect.loc.y + rect.size.h) as f64;
            if hit {
                let focus_origin = self
                    .role_surface_mapping(RuntimeSurfaceRole::NativePane(pane_id.clone()), rect)
                    .map(RoleSurfaceMapping::focus_origin)
                    .unwrap_or_else(|| rect.loc.to_f64());
                return Some((native.wl_surface().clone(), focus_origin));
            }
        }
        self.main_toplevel.as_ref().map(|main| {
            let output_rect = Rectangle::new((0, 0).into(), (output_size.w, output_size.h).into());
            let focus_origin = self
                .role_surface_mapping(RuntimeSurfaceRole::MainApp, output_rect)
                .map(RoleSurfaceMapping::focus_origin)
                .unwrap_or_else(|| output_rect.loc.to_f64());
            (main.wl_surface().clone(), focus_origin)
        })
    }

    fn popup_owner_role(&self, popup: &PopupSurface) -> Option<RuntimeSurfaceRole> {
        let parent = popup.get_parent_surface()?;
        if self
            .main_toplevel
            .as_ref()
            .map(|main| same_surface(main.wl_surface(), &parent))
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::MainApp);
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|overlay| same_surface(overlay.wl_surface(), &parent))
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::OverlayNative);
        }
        for (pane_id, native) in &self.native_pane_toplevels {
            if same_surface(native.wl_surface(), &parent) {
                return Some(RuntimeSurfaceRole::NativePane(pane_id.clone()));
            }
        }
        None
    }

    fn popup_target_rect(&self, role: RuntimeSurfaceRole) -> Rectangle<i32, Logical> {
        match role {
            RuntimeSurfaceRole::MainApp => Rectangle::new(
                (0, 0).into(),
                (self.runtime_output_width(), self.runtime_output_height()).into(),
            ),
            RuntimeSurfaceRole::OverlayNative => {
                let overlay_size = self
                    .overlay_toplevel
                    .as_ref()
                    .and_then(|overlay| overlay.current_state().size)
                    .unwrap_or_else(|| self.overlay_rect().size);
                Rectangle::new((0, 0).into(), overlay_size)
            }
            RuntimeSurfaceRole::NativePane(ref pane_id) => {
                let size = self
                    .native_pane_toplevels
                    .get(pane_id)
                    .and_then(|surface| surface.current_state().size)
                    .or_else(|| self.native_pane_rect(pane_id).map(|rect| rect.size))
                    .unwrap_or_else(|| Size::from((1, 1)));
                Rectangle::new((0, 0).into(), size)
            }
        }
    }

    fn popup_geometry_local(&self, popup: &PopupSurface) -> Rectangle<i32, Logical> {
        smithay::wayland::compositor::with_states(popup.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgPopupSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|attrs| attrs.current.geometry)
                .unwrap_or_else(|| Rectangle::new((0, 0).into(), (1, 1).into()))
        })
    }

    fn role_surface_mapping(
        &self,
        role: RuntimeSurfaceRole,
        target_rect: Rectangle<i32, Logical>,
    ) -> Option<RoleSurfaceMapping> {
        let source_rect = match role {
            RuntimeSurfaceRole::MainApp => self
                .main_toplevel
                .as_ref()
                .map(toplevel_surface_source_rect),
            RuntimeSurfaceRole::OverlayNative => self
                .overlay_toplevel
                .as_ref()
                .map(toplevel_surface_source_rect),
            RuntimeSurfaceRole::NativePane(ref pane_id) => self
                .native_pane_toplevels
                .get(pane_id)
                .map(toplevel_surface_source_rect),
        }?;
        Some(RoleSurfaceMapping::new(source_rect, target_rect))
    }

    fn native_pane_surface_mapping(
        &self,
        pane_id: &PaneId,
        geometry: PaneGeometry,
    ) -> Option<RoleSurfaceMapping> {
        let source_rect = self
            .native_pane_toplevels
            .get(pane_id)
            .map(toplevel_surface_source_rect)?;
        let projection = self
            .presentation_root_geometry
            .as_ref()
            .or(self.staged_root_geometry.as_ref())
            .or(self.active_root_geometry_consumers.as_ref())
            .and_then(|root| {
                root.native_materializations
                    .iter()
                    .find(|(candidate, _)| candidate == pane_id)
                    .map(|(_, projection)| *projection)
            })
            .unwrap_or_else(|| {
                self.root_geometry_snapshot()
                    .expect("native pane mapping requires root4 geometry")
                    .native_buffer(crate::root_geometry::LogicalRect {
                        x: geometry.x,
                        y: geometry.y,
                        width: geometry.width,
                        height: geometry.height,
                    })
            });
        Some(RoleSurfaceMapping::new_native_materialization(
            source_rect,
            projection,
        ))
    }

    fn handle_surface_identity_update(&mut self, surface: &ToplevelSurface) {
        let updated_id = surface_key(surface.wl_surface());
        if let Some(idx) = self
            .pending_toplevels
            .iter()
            .position(|pending| surface_key(pending.wl_surface()) == updated_id)
        {
            let pending = self.pending_toplevels.remove(idx);
            self.assign_toplevel_role(pending);
            return;
        }
        if self.main_toplevel.is_none()
            && matches!(
                self.classify_toplevel(surface),
                SurfaceClassification::MainApp
            )
        {
            self.assign_main_role(surface.clone());
        }
        if let Some((pane_id, launch_pid)) = self.expected_native_pane_for_toplevel(surface) {
            if !self.native_pane_toplevels.contains_key(&pane_id) {
                self.assign_native_pane_role(surface.clone(), pane_id, launch_pid);
            }
        }
        if self
            .main_toplevel
            .as_ref()
            .map(|main| same_surface(main.wl_surface(), surface.wl_surface()))
            .unwrap_or(false)
        {
            if let Some(pid) = self.client_pid_for_toplevel(surface) {
                let evidence = self.main_app_binding_evidence_for_surface(surface);
                self.bridge_main_app_surface_attached(pid, pid, evidence);
            }
        }
        self.enforce_main_app_binding_policy();
    }

    fn role_for_surface(&self, surface: &WlSurface) -> Option<RuntimeSurfaceRole> {
        if self
            .main_toplevel
            .as_ref()
            .map(|main| same_surface(main.wl_surface(), surface))
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::MainApp);
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|overlay| same_surface(overlay.wl_surface(), surface))
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::OverlayNative);
        }
        for (pane_id, toplevel) in &self.native_pane_toplevels {
            if same_surface(toplevel.wl_surface(), surface) {
                return Some(RuntimeSurfaceRole::NativePane(pane_id.clone()));
            }
        }
        for popup in &self.popups {
            if same_surface(popup.surface.wl_surface(), surface) {
                return Some(popup.owner_role.clone());
            }
        }
        None
    }

    fn configure_toplevel_for_role(&self, surface: &ToplevelSurface, role: RuntimeSurfaceRole) {
        let output_w = self.runtime_output_width();
        let output_h = self.runtime_output_height();

        surface.with_pending_state(|pending| {
            pending.states.set(xdg_toplevel::State::Activated);
            pending.decoration_mode = Some(embedded_toplevel_decoration_mode());
            match role {
                RuntimeSurfaceRole::MainApp => {
                    pending.states.set(xdg_toplevel::State::Fullscreen);
                    pending.size = Some((output_w, output_h).into());
                }
                RuntimeSurfaceRole::OverlayNative => {
                    let overlay_rect = self.overlay_rect();
                    pending.size = Some((overlay_rect.size.w, overlay_rect.size.h).into());
                }
                RuntimeSurfaceRole::NativePane(ref pane_id) => {
                    let staged_rect = self.presentation_root_geometry.as_ref().and_then(|root| {
                        root.topology
                            .panes
                            .iter()
                            .find(|pane| &pane.id == pane_id)
                            .map(|pane| rectangle_from_pane_geometry(pane.geometry))
                    });
                    if let Some(rect) = staged_rect.or_else(|| self.native_pane_rect(pane_id)) {
                        pending.size = Some((rect.size.w, rect.size.h).into());
                    }
                }
            }
        });
        let _ = surface.send_pending_configure();
    }

    fn configure_toplevel_with_current_role(&self, surface: &ToplevelSurface) {
        if let Some(role) = self.role_for_surface(surface.wl_surface()) {
            self.configure_toplevel_for_role(surface, role);
            return;
        }
        surface.with_pending_state(|pending| {
            pending.decoration_mode = Some(embedded_toplevel_decoration_mode());
        });
        let _ = surface.send_pending_configure();
    }

    fn reconfigure_roles(&mut self, width: i32, height: i32) {
        self.backend_output_size = Size::<i32, Physical>::from((width.max(1), height.max(1)));
        {
            let mut state = lock_state(&self.shared_state);
            state.mark_runtime_resize(width, height);
        }
        self.sync_output_state();
        self.stage_role_configures();
    }

    fn stage_role_configures(&self) {
        if let Some(main) = &self.main_toplevel {
            self.configure_toplevel_for_role(main, RuntimeSurfaceRole::MainApp);
        }
        if let Some(overlay) = &self.overlay_toplevel {
            self.configure_toplevel_for_role(overlay, RuntimeSurfaceRole::OverlayNative);
        }
        for (pane_id, native) in &self.native_pane_toplevels {
            self.configure_toplevel_for_role(
                native,
                RuntimeSurfaceRole::NativePane(pane_id.clone()),
            );
        }
    }

    fn sync_output_rotation_reconfigure_if_needed(&mut self) {
        let (rotation, generation, active) = {
            let state = lock_state(&self.shared_state);
            let committed = state.root_geometry_projections();
            (
                state.output_rotation(),
                state
                    .root_geometry_snapshot()
                    .map(|snapshot| snapshot.generation)
                    .unwrap_or(0),
                committed.and_then(|committed| {
                    #[cfg(test)]
                    {
                        stage_runtime_root_geometry_consumers(&state, committed, None).ok()
                    }
                    #[cfg(not(test))]
                    {
                        stage_runtime_root_geometry_consumers(&state, committed).ok()
                    }
                }),
            )
        };
        if generation == self.applied_root_geometry_generation {
            return;
        }
        self.applied_output_rotation = rotation;
        self.applied_root_geometry_generation = generation;
        self.active_root_geometry_consumers = active;
        self.reconfigure_roles(self.backend_output_size.w, self.backend_output_size.h);
    }

    fn sync_runtime_status_with_roles(&self) {
        let main_id = self
            .main_toplevel
            .as_ref()
            .map(|surface| surface_id(surface.wl_surface()));
        let overlay_id = self
            .overlay_toplevel
            .as_ref()
            .map(|surface| surface_id(surface.wl_surface()));
        let mut state = lock_state(&self.shared_state);
        let overlay_pane = if overlay_id.is_some() {
            state.active_overlay_pane_id()
        } else {
            None
        };
        state.set_runtime_surface_roles(main_id, overlay_id, overlay_pane);
    }

    fn prune_dead_surfaces(&mut self) {
        self.pending_toplevels.retain(ToplevelSurface::alive);
        self.enforce_main_app_binding_policy();
        self.enforce_overlay_binding_policy();
        if self
            .main_toplevel
            .as_ref()
            .map(|surface| !surface.alive())
            .unwrap_or(false)
        {
            if let Some(main) = self.main_toplevel.as_ref() {
                if let Some(pid) = self
                    .client_pid_for_toplevel(main)
                    .or_else(|| self.expected_main_app_client_pid())
                {
                    self.bridge_main_app_surface_detached(pid);
                }
            }
            self.main_toplevel = None;
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|surface| !surface.alive())
            .unwrap_or(false)
        {
            if let Some(overlay) = self.overlay_toplevel.as_ref() {
                if let Some(pid) = self
                    .client_pid_for_toplevel(overlay)
                    .or_else(|| self.expected_overlay_client_pid())
                {
                    self.bridge_overlay_surface_detached(pid);
                }
            }
            self.overlay_toplevel = None;
        }
        self.native_pane_toplevels
            .retain(|_, surface| surface.alive());
        self.popups.retain(|popup| popup.surface.alive());
        self.promote_pending_toplevels();
        self.sync_runtime_status_with_roles();
    }

    fn collect_render_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        _output_width: i32,
        _output_height: i32,
    ) -> RenderElementCapture {
        if self.native_clip_program.is_none() {
            match compile_native_clip_program(renderer) {
                Ok(program) => self.native_clip_program = Some(program),
                Err(err) => eprintln!("root4 native clip shader unavailable: {err:?}"),
            }
        }
        let mut capture = RenderElementCapture::default();
        if self.native_clip_program.is_none() && !self.native_pane_toplevels.is_empty() {
            capture.failure = Some("root4 native fractional clip shader unavailable".to_string());
        }
        let mut main_elements = Vec::new();
        let mut main_overlay_region_elements = Vec::new();
        let mut main_popup_elements = Vec::new();
        let mut native_pane_elements = Vec::new();
        let mut cropped_native_pane_elements = Vec::new();
        let mut native_pane_popup_elements = Vec::new();
        let mut cropped_native_pane_popup_elements = Vec::new();
        let mut overlay_elements = Vec::new();
        let mut overlay_popup_elements = Vec::new();
        let output_rect = Rectangle::new(
            (0, 0).into(),
            (self.runtime_output_width(), self.runtime_output_height()).into(),
        );
        let main_mapping = self.role_surface_mapping(RuntimeSurfaceRole::MainApp, output_rect);
        let overlay_mapping =
            self.role_surface_mapping(RuntimeSurfaceRole::OverlayNative, self.overlay_rect());
        let pinned_status = self
            .presentation_root_geometry
            .as_ref()
            .or(self.active_root_geometry_consumers.as_ref())
            .map(|geometry| geometry.topology.clone())
            .unwrap_or_else(|| lock_state(&self.shared_state).status_snapshot());
        let overlay_region_status = (
            pinned_status.runtime.overlay_region_debug_borders,
            pinned_status.overlay_regions.regions.clone(),
        );

        if let Some(main) = &self.main_toplevel {
            if let Some(mapping) = main_mapping {
                if let Err(err) = import_surface_tree(renderer, main.wl_surface()) {
                    eprintln!(
                        "host renderer could not import main surface tree: {err:#?}",
                        err = err
                    );
                }
                let elements = render_elements_from_surface_tree(
                    renderer,
                    main.wl_surface(),
                    mapping.render_element_location(),
                    mapping.render_element_scale(),
                    1.0,
                    Kind::Unspecified,
                );
                main_elements.extend(elements);
                if !overlay_region_status.1.is_empty() {
                    main_overlay_region_elements.extend(main_overlay_region_render_elements(
                        renderer,
                        main.wl_surface(),
                        mapping,
                        &overlay_region_status.1,
                    ));
                }
            }
        }

        for popup in &self.popups {
            if popup.owner_role != RuntimeSurfaceRole::MainApp {
                continue;
            }
            if let Some(mapping) = main_mapping {
                if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                    eprintln!(
                        "host renderer could not import main popup surface tree: {err:#?}",
                        err = err
                    );
                }
                let popup_geo = self.popup_geometry_local(&popup.surface);
                let elements = render_elements_from_surface_tree(
                    renderer,
                    popup.surface.wl_surface(),
                    mapping.map_render_element_location(popup_geo.loc),
                    mapping.render_element_scale(),
                    1.0,
                    Kind::Unspecified,
                );
                main_popup_elements.extend(elements);
            }
        }

        if let Some(overlay) = &self.overlay_toplevel {
            if let Some(mapping) = overlay_mapping {
                if let Err(err) = import_surface_tree(renderer, overlay.wl_surface()) {
                    eprintln!(
                        "host renderer could not import overlay surface tree: {err:#?}",
                        err = err
                    );
                }
                let elements = render_elements_from_surface_tree(
                    renderer,
                    overlay.wl_surface(),
                    mapping.render_element_location(),
                    mapping.render_element_scale(),
                    1.0,
                    Kind::Unspecified,
                );
                overlay_elements.extend(elements);
            }
        }

        let mut native_pane_ids: Vec<_> = self.native_pane_toplevels.keys().cloned().collect();
        native_pane_ids.sort();
        for pane_id in native_pane_ids {
            let Some(native) = self.native_pane_toplevels.get(&pane_id) else {
                continue;
            };
            let Some(geometry) = pinned_status
                .panes
                .iter()
                .find(|pane| pane.id == pane_id)
                .map(|pane| pane.geometry)
            else {
                continue;
            };
            let Some(mapping) = self.native_pane_surface_mapping(&pane_id, geometry) else {
                continue;
            };
            let pinned_generation = self
                .presentation_root_geometry
                .as_ref()
                .or(self.active_root_geometry_consumers.as_ref())
                .map(|root| root.committed.snapshot.generation);
            if self
                .host_surface_buffers
                .get(&surface_key(native.wl_surface()))
                .is_none_or(|buffer| buffer.root_geometry_generation != pinned_generation)
            {
                continue;
            }
            if let Err(err) = import_surface_tree(renderer, native.wl_surface()) {
                capture.failure = Some(format!(
                    "host renderer could not import native pane surface tree: {err:#?}"
                ));
                continue;
            }
            let elements = render_elements_from_surface_tree(
                renderer,
                native.wl_surface(),
                mapping.render_element_location(),
                mapping.render_element_scale(),
                1.0,
                Kind::Unspecified,
            );
            let native_projection = self
                .presentation_root_geometry
                .as_ref()
                .or(self.active_root_geometry_consumers.as_ref())
                .and_then(|geometry| {
                    geometry
                        .native_materializations
                        .iter()
                        .find(|(candidate, _)| candidate == &pane_id)
                        .map(|(_, projection)| *projection)
                });
            if let (Some(projection), Some(clip_program)) =
                (native_projection, self.native_clip_program.as_ref())
            {
                let target_height_px = render_output_size_before_transform(self).h as f32;
                let root_element_id = Id::from_wayland_resource(native.wl_surface());
                let root_element_present =
                    elements
                        .iter()
                        .any(|element: &WaylandSurfaceRenderElement<GlesRenderer>| {
                            element.id() == &root_element_id
                        });
                if !root_element_present {
                    capture.failure = Some(format!(
                        "root4 native pane {} has no imported root material",
                        pane_id.0
                    ));
                    continue;
                }
                cropped_native_pane_elements.extend(
                    materialize_native_surface_elements(
                        elements,
                        Some(&root_element_id),
                        projection,
                        clip_program,
                        target_height_px,
                    )
                    .into_iter()
                    .map(SurfAceRenderElement::from),
                );
            } else if native_projection.is_none() {
                native_pane_elements.extend(elements);
            }
        }

        for popup in &self.popups {
            if popup.owner_role != RuntimeSurfaceRole::OverlayNative {
                continue;
            }
            if let Some(mapping) = overlay_mapping {
                if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                    eprintln!(
                        "host renderer could not import overlay popup surface tree: {err:#?}",
                        err = err
                    );
                }
                let popup_geo = self.popup_geometry_local(&popup.surface);
                let elements = render_elements_from_surface_tree(
                    renderer,
                    popup.surface.wl_surface(),
                    mapping.map_render_element_location(popup_geo.loc),
                    mapping.render_element_scale(),
                    1.0,
                    Kind::Unspecified,
                );
                overlay_popup_elements.extend(elements);
            }
        }

        for popup in &self.popups {
            let RuntimeSurfaceRole::NativePane(ref pane_id) = popup.owner_role else {
                continue;
            };
            let Some(geometry) = pinned_status
                .panes
                .iter()
                .find(|pane| &pane.id == pane_id)
                .map(|pane| pane.geometry)
            else {
                continue;
            };
            if let Some(mapping) = self.native_pane_surface_mapping(pane_id, geometry) {
                if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                    capture.failure = Some(format!(
                        "host renderer could not import native pane popup surface tree: {err:#?}"
                    ));
                    continue;
                }
                let popup_geo = self.popup_geometry_local(&popup.surface);
                let elements = render_elements_from_surface_tree(
                    renderer,
                    popup.surface.wl_surface(),
                    mapping.map_render_element_location(popup_geo.loc),
                    mapping.render_element_scale(),
                    1.0,
                    Kind::Unspecified,
                );
                let native_clip = self
                    .presentation_root_geometry
                    .as_ref()
                    .or(self.active_root_geometry_consumers.as_ref())
                    .and_then(|root| {
                        root.native_materializations
                            .iter()
                            .find(|(candidate, _)| candidate == pane_id)
                            .map(|(_, projection)| crate::root_geometry::PhysicalPixelRect {
                                x: projection.origin_x,
                                y: projection.origin_y,
                                width: projection.width_px,
                                height: projection.height_px,
                            })
                    });
                if native_clip.is_some()
                    && let (Some(projection), Some(clip_program)) = (
                        self.presentation_root_geometry
                            .as_ref()
                            .or(self.active_root_geometry_consumers.as_ref())
                            .and_then(|root| {
                                root.native_materializations
                                    .iter()
                                    .find(|(candidate, _)| candidate == pane_id)
                                    .map(|(_, projection)| *projection)
                            }),
                        self.native_clip_program.as_ref(),
                    )
                {
                    let target_height_px = render_output_size_before_transform(self).h as f32;
                    cropped_native_pane_popup_elements.extend(
                        materialize_native_surface_elements(
                            elements,
                            None,
                            projection,
                            clip_program,
                            target_height_px,
                        )
                        .into_iter()
                        .map(SurfAceRenderElement::from),
                    );
                } else {
                    native_pane_popup_elements.extend(elements);
                }
            }
        }

        let debug_border_regions = overlay_region_status
            .0
            .then_some(overlay_region_status.1.as_slice());
        let logical_output_w = self.runtime_output_width();
        let logical_output_h = self.runtime_output_height();
        // Smithay expects top-to-bottom order; otherwise an opaque main surface
        // can cull the overlay before it is drawn. The compositor cursor is
        // drawn in the final output pass so it is not clipped by scene surfaces.
        capture.push(RenderElementSource::OverlayPopup, overlay_popup_elements);
        capture.push(RenderElementSource::Overlay, overlay_elements);
        if let Some(regions) = debug_border_regions {
            capture.push_elements(
                RenderElementSource::OverlayRegionDebug,
                overlay_region_debug_border_elements(regions, logical_output_w, logical_output_h),
            );
        }
        capture.push_elements(
            RenderElementSource::MainOverlayRegion,
            main_overlay_region_elements,
        );
        capture.push(
            RenderElementSource::NativePanePopup,
            native_pane_popup_elements,
        );
        capture.push_elements(
            RenderElementSource::NativePanePopup,
            cropped_native_pane_popup_elements,
        );
        capture.push_elements(
            RenderElementSource::NativePane,
            cropped_native_pane_elements,
        );
        capture.push(RenderElementSource::NativePane, native_pane_elements);
        capture.push(RenderElementSource::MainPopup, main_popup_elements);
        capture.push(RenderElementSource::Main, main_elements);

        capture
    }

    fn collect_overlay_plane_elements_local(
        &self,
        renderer: &mut GlesRenderer,
    ) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
        let Some(overlay) = &self.overlay_toplevel else {
            return Vec::new();
        };

        if let Err(err) = import_surface_tree(renderer, overlay.wl_surface()) {
            eprintln!(
                "host renderer could not import overlay surface tree for overlay plane: {err:#?}",
                err = err
            );
        }
        let overlay_elements = render_elements_from_surface_tree(
            renderer,
            overlay.wl_surface(),
            (0, 0),
            1.0,
            1.0,
            Kind::Unspecified,
        );
        let mut elements = Vec::new();

        for popup in &self.popups {
            if popup.owner_role != RuntimeSurfaceRole::OverlayNative {
                continue;
            }
            let popup_local = self.popup_geometry_local(&popup.surface);
            if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                eprintln!(
                    "host renderer could not import overlay popup surface tree for overlay plane: {err:#?}",
                    err = err
                );
            }
            let popup_elements = render_elements_from_surface_tree(
                renderer,
                popup.surface.wl_surface(),
                (popup_local.loc.x, popup_local.loc.y),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            elements.extend(popup_elements);
        }
        elements.extend(overlay_elements);

        elements
    }

    fn send_frame_callbacks(&self) {
        let elapsed_ms = self.start_time.elapsed().as_millis() as u32;
        if let Some(main) = &self.main_toplevel {
            send_frames_surface_tree(main.wl_surface(), elapsed_ms);
        }
        if let Some(overlay) = &self.overlay_toplevel {
            send_frames_surface_tree(overlay.wl_surface(), elapsed_ms);
        }
        for native in self.native_pane_toplevels.values() {
            send_frames_surface_tree(native.wl_surface(), elapsed_ms);
        }
        for popup in &self.popups {
            send_frames_surface_tree(popup.surface.wl_surface(), elapsed_ms);
        }
    }

    fn overlay_binding_expected(&self) -> bool {
        lock_state(&self.shared_state).runtime_overlay_binding_expected()
    }

    fn expected_main_app_binding(&self) -> Option<crate::state::MainAppBindingExpectation> {
        lock_state(&self.shared_state).runtime_expected_main_app_binding_with_token()
    }

    fn expected_main_app_client_pid(&self) -> Option<u32> {
        self.expected_main_app_binding()
            .map(|expectation| expectation.pid)
    }

    fn expected_overlay_client_pid(&self) -> Option<u32> {
        lock_state(&self.shared_state)
            .runtime_expected_overlay_binding()
            .map(|(_pane_id, pid)| pid)
    }

    fn expected_native_pane_for_toplevel(
        &self,
        surface: &ToplevelSurface,
    ) -> Option<(PaneId, u32)> {
        let client_pid = self.client_pid_for_toplevel(surface)?;
        let expectations = lock_state(&self.shared_state).runtime_expected_native_pane_bindings();
        expectations
            .iter()
            .find_map(|expectation| {
                pid_matches_or_descends_from(client_pid, expectation.pid)
                    .then_some((expectation.pane_id.clone(), expectation.pid))
            })
            .or_else(|| {
                expectations.into_iter().find_map(|expectation| {
                    expectation
                        .launch_token
                        .as_deref()
                        .is_some_and(|token| process_launch_token_matches(client_pid, token))
                        .then_some((expectation.pane_id, expectation.pid))
                })
            })
    }

    fn client_pid_for_toplevel(&self, surface: &ToplevelSurface) -> Option<u32> {
        let client = surface.wl_surface().client()?;
        let credentials = client.get_credentials(&self.display_handle).ok()?;
        let pid_i64 = i64::from(credentials.pid);
        let pid = u32::try_from(pid_i64).ok()?;
        (pid > 0).then_some(pid)
    }

    fn bridge_overlay_surface_attached(&self, client_pid: u32) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_overlay_surface_attached_for_pid(client_pid);
    }

    fn bridge_main_app_surface_attached(
        &self,
        launch_pid: u32,
        client_pid: u32,
        evidence: Option<SurfaceBindingEvidence>,
    ) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_main_app_surface_attached_for_launch_pid_with_evidence(
            launch_pid, client_pid, evidence,
        );
    }

    fn bridge_native_pane_surface_attached(
        &self,
        launch_pid: u32,
        client_pid: u32,
        surface_id: Option<u32>,
        evidence: Option<SurfaceBindingEvidence>,
    ) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_native_pane_surface_attached_for_launch_pid_with_evidence(
            launch_pid, client_pid, surface_id, evidence,
        );
    }

    fn bridge_native_pane_window_group_observed(
        &self,
        pane_id: &PaneId,
        primary_window_id: String,
        surface_id: Option<u32>,
    ) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_native_pane_window_group_observed(
            pane_id,
            primary_window_id,
            surface_id,
        );
    }

    fn bridge_native_pane_surface_detached(&self, client_pid: u32) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_native_pane_surface_detached_for_pid(client_pid);
    }

    fn bridge_overlay_surface_detached(&self, client_pid: u32) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_overlay_surface_detached_for_pid(client_pid);
    }

    fn bridge_main_app_surface_detached(&self, client_pid: u32) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_main_app_surface_detached_for_pid(client_pid);
    }

    fn enforce_overlay_binding_policy(&self) {
        if self.overlay_toplevel.is_some() && !self.overlay_binding_expected() {
            if let Some(overlay) = &self.overlay_toplevel {
                overlay.send_close();
            }
        }
    }

    fn capture_surface_buffer_commit(&mut self, surface: &WlSurface) {
        let assignment = smithay::wayland::compositor::with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            match guard.current().buffer.as_ref() {
                Some(smithay::wayland::compositor::BufferAssignment::NewBuffer(buffer)) => {
                    Some(Some(buffer.clone()))
                }
                Some(smithay::wayland::compositor::BufferAssignment::Removed) => Some(None),
                None => None,
            }
        });

        let id = surface_key(surface);
        // Every wl_surface commit is material to the accepted operation: in
        // addition to buffers it can publish subsurface topology/position or
        // xdg popup/toplevel state consumed by the same render tree.
        self.next_surface_material_serial = self.next_surface_material_serial.saturating_add(1);
        self.surface_material_serials
            .insert(id.clone(), self.next_surface_material_serial);
        let native_materialization = self
            .native_pane_toplevels
            .iter()
            .find(|(_, toplevel)| same_surface(toplevel.wl_surface(), surface))
            .and_then(|(pane_id, _)| {
                let geometry = self
                    .presentation_root_geometry
                    .as_ref()
                    .or(self.staged_root_geometry.as_ref())
                    .or(self.active_root_geometry_consumers.as_ref())?;
                geometry
                    .native_materializations
                    .iter()
                    .find(|(candidate, _)| candidate == pane_id)
                    .map(|(_, projection)| (*projection, geometry.committed.snapshot))
            });
        let damage = native_materialization.and_then(|(projection, root)| {
            let damaged = smithay::wayland::compositor::with_states(surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                guard
                    .current()
                    .damage
                    .iter()
                    .map(|damage| match damage {
                        smithay::wayland::compositor::Damage::Surface(rect) => {
                            crate::root_geometry::LogicalRect {
                                x: projection.logical_clip.x + rect.loc.x as f64,
                                y: projection.logical_clip.y + rect.loc.y as f64,
                                width: rect.size.w as f64,
                                height: rect.size.h as f64,
                            }
                        }
                        smithay::wayland::compositor::Damage::Buffer(rect) => {
                            crate::root_geometry::LogicalRect {
                                x: projection.logical_clip.x
                                    + (rect.loc.x as f64 - projection.fractional_phase_x)
                                        / projection.scale_factor,
                                y: projection.logical_clip.y
                                    + (rect.loc.y as f64 - projection.fractional_phase_y)
                                        / projection.scale_factor,
                                width: rect.size.w as f64 / projection.scale_factor,
                                height: rect.size.h as f64 / projection.scale_factor,
                            }
                        }
                    })
                    .map(|rect| root.physical_crop(rect))
                    .reduce(|left, right| {
                        let x0 = left.x.min(right.x);
                        let y0 = left.y.min(right.y);
                        let x1 = (left.x + left.width).max(right.x + right.width);
                        let y1 = (left.y + left.height).max(right.y + right.height);
                        crate::root_geometry::PhysicalPixelRect {
                            x: x0,
                            y: y0,
                            width: x1 - x0,
                            height: y1 - y0,
                        }
                    })
            });
            damaged
        });
        match assignment {
            Some(Some(buffer)) => {
                let mut kind = SurfaceBufferKind::Other;
                let mut size = None;
                let mut dmabuf = None;
                match with_buffer_contents(&buffer, |_, _, data| {
                    kind = SurfaceBufferKind::Shm;
                    size = Some(Size::new(data.width, data.height));
                }) {
                    Ok(()) => {}
                    Err(BufferAccessError::NotManaged) => match get_dmabuf(&buffer) {
                        Ok(dmabuf_handle) => {
                            kind = SurfaceBufferKind::Dmabuf;
                            let dmabuf_size = dmabuf_handle.size();
                            let dmabuf_width = dmabuf_size.w;
                            let dmabuf_height = dmabuf_size.h;
                            size = Some(Size::new(dmabuf_width, dmabuf_height));
                            let dmabuf_format = dmabuf_handle.format();
                            dmabuf = Some(SurfaceDmabufInfo {
                                width: dmabuf_width,
                                height: dmabuf_height,
                                format: dmabuf_format.code,
                                modifier: dmabuf_format.modifier,
                            });
                        }
                        Err(err) => {
                            eprintln!(
                                "host fallback could not inspect dmabuf for surface {}: {err:?}",
                                surface_id(surface)
                            );
                        }
                    },
                    Err(err) => {
                        eprintln!(
                            "host fallback could not read shm buffer for surface {}: {err:?}",
                            surface_id(surface)
                        );
                    }
                }
                if let (Some((projection, _)), Some(buffer_size)) = (native_materialization, size)
                    && (buffer_size.w != projection.width_px
                        || buffer_size.h != projection.height_px)
                {
                    self.host_surface_buffers.remove(&id);
                    eprintln!(
                        "root4 native buffer rejected: expected {}x{}, received {}x{}",
                        projection.width_px, projection.height_px, buffer_size.w, buffer_size.h
                    );
                    return;
                }
                self.host_surface_buffers.insert(
                    id,
                    SurfaceBufferSnapshot {
                        buffer,
                        kind,
                        size,
                        dmabuf,
                        native_materialization: native_materialization.map(|value| value.0),
                        damage,
                        root_geometry_generation: native_materialization
                            .map(|value| value.1.generation),
                    },
                );
            }
            Some(None) => {
                self.host_surface_buffers.remove(&id);
            }
            None => {}
        }
    }

    fn drop_surface_buffer(&mut self, surface: &WlSurface) {
        let id = surface_key(surface);
        self.host_surface_buffers.remove(&id);
        self.surface_material_serials.remove(&id);
    }

    fn host_scene_surfaces(
        &self,
        output_w: i32,
        output_h: i32,
        status: &crate::model::StatusSnapshot,
    ) -> Vec<HostSceneSurface> {
        let mut surfaces = Vec::new();
        let output_rect = Rectangle::new((0, 0).into(), (output_w, output_h).into());
        let main_mapping = self.role_surface_mapping(RuntimeSurfaceRole::MainApp, output_rect);
        let overlay_mapping =
            self.role_surface_mapping(RuntimeSurfaceRole::OverlayNative, self.overlay_rect());
        if let Some(main) = &self.main_toplevel {
            if let Some(mapping) = main_mapping {
                self.collect_surface_tree_surfaces(
                    main.wl_surface(),
                    (0, 0).into(),
                    mapping,
                    &mut surfaces,
                );
            }
        }

        if let Some(overlay) = &self.overlay_toplevel {
            if let Some(mapping) = overlay_mapping {
                self.collect_surface_tree_surfaces(
                    overlay.wl_surface(),
                    (0, 0).into(),
                    mapping,
                    &mut surfaces,
                );
            }
        }

        let mut native_pane_ids: Vec<_> = self.native_pane_toplevels.keys().cloned().collect();
        native_pane_ids.sort();
        for pane_id in native_pane_ids {
            let Some(native) = self.native_pane_toplevels.get(&pane_id) else {
                continue;
            };
            let Some(geometry) = status
                .panes
                .iter()
                .find(|pane| pane.id == pane_id)
                .map(|pane| pane.geometry)
            else {
                continue;
            };
            if let Some(mapping) = self.native_pane_surface_mapping(&pane_id, geometry) {
                self.collect_surface_tree_surfaces(
                    native.wl_surface(),
                    (0, 0).into(),
                    mapping,
                    &mut surfaces,
                );
            }
        }

        for popup in &self.popups {
            if let Some(snapshot) = self
                .host_surface_buffers
                .get(&surface_key(popup.surface.wl_surface()))
            {
                let mapping = match &popup.owner_role {
                    RuntimeSurfaceRole::MainApp => main_mapping,
                    RuntimeSurfaceRole::OverlayNative => overlay_mapping,
                    RuntimeSurfaceRole::NativePane(pane_id) => status
                        .panes
                        .iter()
                        .find(|pane| &pane.id == pane_id)
                        .and_then(|pane| self.native_pane_surface_mapping(pane_id, pane.geometry)),
                };
                let Some(mapping) = mapping else {
                    continue;
                };
                let target_size = if let Some(size) = snapshot.size.as_ref() {
                    Some(Size::new(size.w, size.h))
                } else if let Some(info) = snapshot.dmabuf.as_ref() {
                    Some(Size::new(info.width, info.height))
                } else {
                    None
                };
                let Some(target_size) = target_size else {
                    continue;
                };
                let popup_geo = self.popup_geometry_local(&popup.surface);
                surfaces.push(HostSceneSurface {
                    buffer: snapshot.buffer.clone(),
                    kind: snapshot.kind,
                    target: mapping.map_rect(Rectangle::new(popup_geo.loc, target_size)),
                    dmabuf: snapshot.dmabuf,
                    native_materialization: snapshot.native_materialization,
                    damage: snapshot.damage,
                });
            }
        }

        surfaces
    }

    fn collect_surface_tree_surfaces(
        &self,
        surface: &WlSurface,
        base_loc: Point<i32, Logical>,
        mapping: RoleSurfaceMapping,
        surfaces: &mut Vec<HostSceneSurface>,
    ) {
        with_surface_tree_downward(
            surface,
            base_loc,
            |_surface, data, &offset| {
                let location = data
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location;
                let next_offset = Point::new(offset.x + location.x, offset.y + location.y);
                TraversalAction::DoChildren(next_offset)
            },
            |surface, _, &offset| {
                if let Some(snapshot) = self.host_surface_buffers.get(&surface_key(surface)) {
                    let target_size = if let Some(size) = snapshot.size.as_ref() {
                        Some(Size::new(size.w, size.h))
                    } else if let Some(info) = snapshot.dmabuf.as_ref() {
                        Some(Size::new(info.width, info.height))
                    } else {
                        None
                    };
                    if let Some(size) = target_size {
                        surfaces.push(HostSceneSurface {
                            buffer: snapshot.buffer.clone(),
                            kind: snapshot.kind,
                            target: mapping.map_rect(Rectangle::new(offset, size)),
                            dmabuf: snapshot.dmabuf,
                            native_materialization: snapshot.native_materialization,
                            damage: snapshot.damage,
                        });
                    }
                }
            },
            |_, _, &_offset| true,
        );
    }

    fn compose_host_scene(
        &self,
        target: &mut [u8],
        target_stride: usize,
        output_w: i32,
        output_h: i32,
    ) -> HostSceneComposeStats {
        let mut stats = HostSceneComposeStats::default();
        clear_host_scene_background(target, target_stride, output_w, output_h);
        let Some(root_geometry) = self.root_geometry_snapshot() else {
            return stats;
        };
        let status = self
            .presentation_root_geometry
            .as_ref()
            .or(self.active_root_geometry_consumers.as_ref())
            .map(|geometry| geometry.topology.clone())
            .unwrap_or_else(|| lock_state(&self.shared_state).status_snapshot());
        for surface in self.host_scene_surfaces(output_w, output_h, &status) {
            stats.attempted_surfaces = stats.attempted_surfaces.saturating_add(1);
            let crop = root_geometry.physical_crop(crate::root_geometry::LogicalRect {
                x: surface.target.loc.x as f64,
                y: surface.target.loc.y as f64,
                width: surface.target.size.w as f64,
                height: surface.target.size.h as f64,
            });
            let target_rect =
                Rectangle::new((crop.x, crop.y).into(), (crop.width, crop.height).into());
            let damage_rect = surface.damage.map(|damage| {
                Rectangle::new(
                    (damage.x, damage.y).into(),
                    (damage.width, damage.height).into(),
                )
            });
            match surface.kind {
                SurfaceBufferKind::Shm => {
                    if blit_shm_surface(
                        &surface.buffer,
                        target_rect,
                        damage_rect,
                        surface.native_materialization,
                        root_geometry.rotation,
                        target,
                        target_stride,
                        output_w,
                        output_h,
                    ) {
                        stats.composed_surfaces = stats.composed_surfaces.saturating_add(1);
                    }
                }
                SurfaceBufferKind::Dmabuf => {
                    if let Some(info) = surface.dmabuf {
                        eprintln!(
                            "host fallback skipping dmabuf surface at {:?} ({}x{}, fmt={:#X}, mod={:?})",
                            surface.target,
                            info.width,
                            info.height,
                            info.format as u32,
                            info.modifier
                        );
                    } else {
                        eprintln!(
                            "host fallback skipping dmabuf surface at {:?}",
                            surface.target
                        );
                    }
                }
                SurfaceBufferKind::Other => {
                    eprintln!(
                        "host fallback skipping unsupported surface at {:?}",
                        surface.target
                    );
                }
            }
        }
        if status.runtime.overlay_region_debug_borders {
            draw_overlay_region_debug_borders(
                &status.overlay_regions.regions,
                root_geometry,
                target,
                target_stride,
                output_w,
                output_h,
            );
        }
        draw_software_cursor(
            self.cursor_render_location(),
            root_geometry,
            target,
            target_stride,
            output_w,
            output_h,
        );
        stats
    }
}

impl BufferHandler for RuntimeWaylandState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for RuntimeWaylandState {
    fn compositor_state(&mut self) -> &mut SmithayCompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<RuntimeClientState>()
            .expect("runtime client state missing")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        let scale = self.root_display_scale();
        smithay::wayland::compositor::with_states(surface, |states| {
            fractional_scale::with_fractional_scale(states, |fractional_scale| {
                fractional_scale.set_preferred_scale(scale);
            });
        });
        self.capture_surface_buffer_commit(surface);
        on_commit_buffer_handler::<Self>(surface);
    }
}

impl FractionalScaleHandler for RuntimeWaylandState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.root_display_scale();
        smithay::wayland::compositor::with_states(&surface, |states| {
            fractional_scale::with_fractional_scale(states, |fractional_scale| {
                fractional_scale.set_preferred_scale(scale);
            });
        });
    }
}

impl XdgShellHandler for RuntimeWaylandState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.assign_toplevel_role(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if let Some(owner_role) = self.popup_owner_role(&surface) {
            let target = self.popup_target_rect(owner_role.clone());
            surface.with_pending_state(|pending| {
                pending.geometry = pending.positioner.get_unconstrained_geometry(target);
            });
            let _ = surface.send_configure();
            self.popups.push(ManagedPopup {
                surface,
                owner_role,
            });
            return;
        }
        surface.send_popup_done();
    }

    fn ack_configure(&mut self, surface: WlSurface, _configure: Configure) {
        if let Some(main) = &self.main_toplevel {
            if main.wl_surface() == &surface {
                self.configure_toplevel_for_role(main, RuntimeSurfaceRole::MainApp);
            }
        }
        if let Some(overlay) = &self.overlay_toplevel {
            if overlay.wl_surface() == &surface {
                self.configure_toplevel_for_role(overlay, RuntimeSurfaceRole::OverlayNative);
            }
        }
        for (pane_id, native) in &self.native_pane_toplevels {
            if native.wl_surface() == &surface {
                self.configure_toplevel_for_role(
                    native,
                    RuntimeSurfaceRole::NativePane(pane_id.clone()),
                );
            }
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        _positioner: PositionerState,
        token: u32,
    ) {
        let _ = surface.send_repositioned(token);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let destroyed_id = surface_key(surface.wl_surface());
        self.drop_surface_buffer(surface.wl_surface());
        self.pending_toplevels
            .retain(|pending| surface_key(pending.wl_surface()) != destroyed_id);
        if self
            .main_toplevel
            .as_ref()
            .map(|item| surface_key(item.wl_surface()) == destroyed_id)
            .unwrap_or(false)
        {
            if let Some(pid) = self
                .client_pid_for_toplevel(&surface)
                .or_else(|| self.expected_main_app_client_pid())
            {
                self.bridge_main_app_surface_detached(pid);
            }
            self.main_toplevel = None;
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|item| surface_key(item.wl_surface()) == destroyed_id)
            .unwrap_or(false)
        {
            if let Some(pid) = self
                .client_pid_for_toplevel(&surface)
                .or_else(|| self.expected_overlay_client_pid())
            {
                self.bridge_overlay_surface_detached(pid);
            }
            self.overlay_toplevel = None;
        }
        let destroyed_native_pane =
            self.native_pane_toplevels
                .iter()
                .find_map(|(pane_id, item)| {
                    (surface_key(item.wl_surface()) == destroyed_id).then(|| pane_id.clone())
                });
        if let Some(pane_id) = destroyed_native_pane {
            if let Some(native) = self.native_pane_toplevels.remove(&pane_id) {
                if let Some(pid) = self.client_pid_for_toplevel(&native) {
                    self.bridge_native_pane_surface_detached(pid);
                }
            }
        }
        let mut removed_popup_ids = Vec::new();
        self.popups.retain(|popup| {
            let keep = popup.surface.get_parent_surface().as_ref().map(surface_key)
                != Some(destroyed_id.clone());
            if !keep {
                removed_popup_ids.push(surface_key(popup.surface.wl_surface()));
            }
            keep
        });
        for popup_id in removed_popup_ids {
            self.host_surface_buffers.remove(&popup_id);
        }
        self.promote_pending_toplevels();
        self.sync_runtime_status_with_roles();
        self.apply_focus_route();
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        let destroyed_id = surface_key(surface.wl_surface());
        self.drop_surface_buffer(surface.wl_surface());
        self.popups
            .retain(|popup| surface_key(popup.surface.wl_surface()) != destroyed_id);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.handle_surface_identity_update(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.handle_surface_identity_update(&surface);
    }
}

impl XdgDecorationHandler for RuntimeWaylandState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.configure_toplevel_with_current_role(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: XdgDecorationMode) {
        self.configure_toplevel_with_current_role(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.configure_toplevel_with_current_role(&toplevel);
    }
}

impl ShmHandler for RuntimeWaylandState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for RuntimeWaylandState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let format = dmabuf.format();
        let supported = self.dmabuf_formats.iter().any(|entry| *entry == format);
        if !supported {
            eprintln!(
                "host dmabuf import rejected unsupported format/modifier pair (fmt={:#X}, mod={:?})",
                format.code as u32, format.modifier
            );
            notifier.failed();
            return;
        }

        let _ = notifier.successful::<Self>();
    }
}

impl SeatHandler for RuntimeWaylandState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        set_data_device_focus(
            &self.display_handle,
            &self.seat,
            focused.and_then(Resource::client),
        );
        let target = focused
            .and_then(|surface| self.role_for_surface(surface))
            .map(runtime_focus_target_for_role);
        let mut state = lock_state(&self.shared_state);
        state.set_runtime_focus_target(target);
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

fn runtime_focus_target_for_role(role: RuntimeSurfaceRole) -> RuntimeFocusTarget {
    match role {
        RuntimeSurfaceRole::MainApp => RuntimeFocusTarget::MainApp,
        RuntimeSurfaceRole::OverlayNative => RuntimeFocusTarget::OverlayNative,
        RuntimeSurfaceRole::NativePane(pane_id) => RuntimeFocusTarget::NativePane { pane_id },
    }
}

impl SelectionHandler for RuntimeWaylandState {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for RuntimeWaylandState {}

impl ServerDndGrabHandler for RuntimeWaylandState {}

impl DataDeviceHandler for RuntimeWaylandState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self._data_device_state
    }
}

#[derive(Default)]
struct RuntimeClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for RuntimeClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

fn clear_host_scene_background(
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) {
    let width = output_w.max(0) as usize;
    let height = output_h.max(0) as usize;
    for y in 0..height {
        let row_start = y.saturating_mul(target_stride);
        let row_end = row_start
            .saturating_add(width.saturating_mul(4))
            .min(target.len());
        if row_start >= row_end {
            continue;
        }
        for px in target[row_start..row_end].chunks_exact_mut(4) {
            px.copy_from_slice(&[0x14, 0x14, 0x18, 0x00]);
        }
    }
}

fn blit_shm_surface(
    buffer: &wl_buffer::WlBuffer,
    target_rect: Rectangle<i32, Logical>,
    damage_rect: Option<Rectangle<i32, Logical>>,
    native_materialization: Option<crate::root_geometry::NativeBufferProjection>,
    rotation: OutputRotation,
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) -> bool {
    if target_rect.size.w <= 0 || target_rect.size.h <= 0 || output_w <= 0 || output_h <= 0 {
        return false;
    }

    let clipped = target_rect
        .intersection(Rectangle::new((0, 0).into(), (output_w, output_h).into()))
        .and_then(|rect| damage_rect.map_or(Some(rect), |damage| rect.intersection(damage)));
    let Some(clipped) = clipped else {
        return false;
    };

    let result = with_buffer_contents(buffer, |ptr, len, info| {
        if info.width <= 0 || info.height <= 0 || info.stride <= 0 || info.offset < 0 {
            return false;
        }
        if !matches!(
            info.format,
            wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888
        ) {
            return false;
        }

        // Safety: smithay validated the wl_shm pool bounds for this callback.
        let src = unsafe { std::slice::from_raw_parts(ptr, len) };
        let src_stride = info.stride as usize;
        let src_offset = info.offset as usize;
        let src_w = info.width as usize;
        let src_h = info.height as usize;
        let dst_w = target_rect.size.w as usize;
        let dst_h = target_rect.size.h as usize;

        if dst_w == 0 || dst_h == 0 {
            return false;
        }
        if src_offset >= src.len() {
            return false;
        }

        let clipped_top = (clipped.loc.y - target_rect.loc.y).max(0) as usize;
        let clipped_left = (clipped.loc.x - target_rect.loc.x).max(0) as usize;
        let clipped_bottom = clipped_top + clipped.size.h as usize;
        let clipped_right = clipped_left + clipped.size.w as usize;

        for rel_y in clipped_top..clipped_bottom {
            let dst_y_i32 = target_rect.loc.y + rel_y as i32;
            if !(0..output_h).contains(&dst_y_i32) {
                continue;
            }
            let dst_row = (dst_y_i32 as usize).saturating_mul(target_stride);
            for rel_x in clipped_left..clipped_right {
                let (sample_x, sample_y) = native_source_sample(
                    native_materialization,
                    rotation,
                    rel_x,
                    rel_y,
                    dst_w,
                    dst_h,
                    src_w,
                    src_h,
                );
                let src_x = sample_x.floor().clamp(0.0, src_w.saturating_sub(1) as f64) as usize;
                let src_y = sample_y.floor().clamp(0.0, src_h.saturating_sub(1) as f64) as usize;
                let dst_x_i32 = target_rect.loc.x + rel_x as i32;
                if !(0..output_w).contains(&dst_x_i32) {
                    continue;
                }
                let src_idx = src_offset
                    .saturating_add(src_y.saturating_mul(src_stride))
                    .saturating_add(src_x.saturating_mul(4));
                let dst_idx = dst_row.saturating_add((dst_x_i32 as usize).saturating_mul(4));
                if src_idx.saturating_add(4) > src.len() || dst_idx.saturating_add(4) > target.len()
                {
                    continue;
                }

                let src_b = src[src_idx];
                let src_g = src[src_idx + 1];
                let src_r = src[src_idx + 2];
                let src_a = if info.format == wl_shm::Format::Argb8888 {
                    src[src_idx + 3]
                } else {
                    0xFF
                };

                if src_a == 0xFF {
                    target[dst_idx] = src_b;
                    target[dst_idx + 1] = src_g;
                    target[dst_idx + 2] = src_r;
                    target[dst_idx + 3] = 0x00;
                } else if src_a != 0x00 {
                    let inv_a = 255u16.saturating_sub(src_a as u16);
                    let dst_b = target[dst_idx] as u16;
                    let dst_g = target[dst_idx + 1] as u16;
                    let dst_r = target[dst_idx + 2] as u16;

                    target[dst_idx] = ((src_b as u16 * src_a as u16 + dst_b * inv_a) / 255) as u8;
                    target[dst_idx + 1] =
                        ((src_g as u16 * src_a as u16 + dst_g * inv_a) / 255) as u8;
                    target[dst_idx + 2] =
                        ((src_r as u16 * src_a as u16 + dst_r * inv_a) / 255) as u8;
                    target[dst_idx + 3] = 0x00;
                }
            }
        }

        true
    });

    match result {
        Ok(composed) => composed,
        Err(BufferAccessError::NotManaged) => false,
        Err(BufferAccessError::NotReadable) => false,
        Err(BufferAccessError::BadMap) => false,
        Err(BufferAccessError::NotWritable) => false,
    }
}

fn native_source_sample(
    native_materialization: Option<crate::root_geometry::NativeBufferProjection>,
    rotation: OutputRotation,
    rel_x: usize,
    rel_y: usize,
    dst_w: usize,
    dst_h: usize,
    src_w: usize,
    src_h: usize,
) -> (f64, f64) {
    let raw_u = (rel_x as f64 + 0.5) / dst_w as f64;
    let raw_v = (rel_y as f64 + 0.5) / dst_h as f64;
    let (oriented_u, oriented_v) = match rotation {
        OutputRotation::Deg0 => (raw_u, raw_v),
        OutputRotation::Deg90 => (raw_v, 1.0 - raw_u),
        OutputRotation::Deg180 => (1.0 - raw_u, 1.0 - raw_v),
        OutputRotation::Deg270 => (1.0 - raw_v, raw_u),
    };
    let (source_x, source_y, source_width, source_height) =
        native_materialization.map_or((0.0, 0.0, src_w as f64, src_h as f64), |projection| {
            (
                projection.fractional_phase_x,
                projection.fractional_phase_y,
                projection.logical_clip.width * projection.scale_factor,
                projection.logical_clip.height * projection.scale_factor,
            )
        });
    (
        source_x + oriented_u * source_width,
        source_y + oriented_v * source_height,
    )
}

fn overlay_region_capture_contains(
    regions: &[OverlayRegionStatus],
    pos: Point<f64, Logical>,
    capture: OverlayCaptureCapability,
) -> bool {
    regions.iter().any(|region| {
        region.captures.contains(&capture)
            && pos.x >= region.rect.x
            && pos.x < region.rect.x + region.rect.width
            && pos.y >= region.rect.y
            && pos.y < region.rect.y + region.rect.height
    })
}

fn main_overlay_region_render_elements(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    mapping: RoleSurfaceMapping,
    regions: &[OverlayRegionStatus],
) -> Vec<SurfAceRenderElement> {
    let mut elements = Vec::new();
    for region in regions {
        // CropRenderElement crops in the rendered element's target-space after
        // the source-to-output mapping is applied. Overlay-region rects must
        // therefore be remapped through the same surface mapping or the lifted
        // clone drifts away from the debug rects whenever the main surface is
        // scaled, decorated, or otherwise non-identity mapped.
        let mapped_rect = mapping.map_rect(overlay_region_logical_rect(region));
        let crop_rect = Rectangle::<i32, Physical>::new(
            (mapped_rect.loc.x, mapped_rect.loc.y).into(),
            (mapped_rect.size.w, mapped_rect.size.h).into(),
        );
        let region_elements = render_elements_from_surface_tree(
            renderer,
            surface,
            mapping.render_element_location(),
            mapping.render_element_scale(),
            1.0,
            Kind::Unspecified,
        );
        elements.extend(region_elements.into_iter().filter_map(|element| {
            CropRenderElement::from_element(element, 1.0, crop_rect).map(SurfAceRenderElement::from)
        }));
    }
    elements
}

fn overlay_region_logical_rect(region: &OverlayRegionStatus) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (region.rect.x.floor() as i32, region.rect.y.floor() as i32).into(),
        (
            region.rect.width.ceil().max(1.0) as i32,
            region.rect.height.ceil().max(1.0) as i32,
        )
            .into(),
    )
}

fn overlay_region_debug_border_elements(
    regions: &[OverlayRegionStatus],
    output_w: i32,
    output_h: i32,
) -> Vec<SurfAceRenderElement> {
    overlay_region_debug_border_rects(regions, output_w, output_h)
        .into_iter()
        .map(|rect| {
            SolidColorRenderElement::new(
                Id::new(),
                Rectangle::<i32, Physical>::new(
                    (rect.loc.x, rect.loc.y).into(),
                    (rect.size.w, rect.size.h).into(),
                ),
                CommitCounter::default(),
                Color32F::new(1.0, 0.0, 0.85, 1.0),
                Kind::Unspecified,
            )
            .into()
        })
        .collect()
}

fn overlay_region_debug_border_rects(
    regions: &[OverlayRegionStatus],
    output_w: i32,
    output_h: i32,
) -> Vec<Rectangle<i32, Logical>> {
    let bounds = Rectangle::new((0, 0).into(), (output_w.max(0), output_h.max(0)).into());
    if bounds.size.w == 0 || bounds.size.h == 0 {
        return Vec::new();
    }

    let mut rects = Vec::new();
    for region in regions {
        let region_rect = overlay_region_logical_rect(region);
        let x = region_rect.loc.x;
        let y = region_rect.loc.y;
        let w = region_rect.size.w;
        let h = region_rect.size.h;
        let border = 2.min(w).min(h).max(1);
        let candidates = [
            Rectangle::new((x, y).into(), (w, border).into()),
            Rectangle::new((x, y + h - border).into(), (w, border).into()),
            Rectangle::new((x, y).into(), (border, h).into()),
            Rectangle::new((x + w - border, y).into(), (border, h).into()),
        ];
        rects.extend(
            candidates
                .into_iter()
                .filter_map(|rect| rect.intersection(bounds))
                .filter(|rect| rect.size.w > 0 && rect.size.h > 0),
        );
    }
    rects
}

fn draw_overlay_region_debug_borders(
    regions: &[OverlayRegionStatus],
    root_geometry: crate::root_geometry::RootGeometrySnapshot,
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) {
    let logical_size = root_geometry.logical_size_i32();
    for logical_rect in
        overlay_region_debug_border_rects(regions, logical_size.width, logical_size.height)
    {
        let crop = root_geometry.physical_crop(crate::root_geometry::LogicalRect {
            x: logical_rect.loc.x as f64,
            y: logical_rect.loc.y as f64,
            width: logical_rect.size.w as f64,
            height: logical_rect.size.h as f64,
        });
        let rect = Rectangle::<i32, Physical>::new(
            (crop.x, crop.y).into(),
            (crop.width, crop.height).into(),
        );
        for y in rect.loc.y.max(0)..(rect.loc.y + rect.size.h).min(output_h) {
            let row_start = (y as usize).saturating_mul(target_stride);
            for x in rect.loc.x.max(0)..(rect.loc.x + rect.size.w).min(output_w) {
                let idx = row_start.saturating_add((x as usize).saturating_mul(4));
                if idx.saturating_add(4) <= target.len() {
                    target[idx] = 0xD9;
                    target[idx + 1] = 0x00;
                    target[idx + 2] = 0xFF;
                    target[idx + 3] = 0x00;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoftwareCursorColor {
    White,
    Black,
}

const SOFTWARE_CURSOR_MASK_WIDTH: i32 = 48;
const SOFTWARE_CURSOR_MASK_HEIGHT: i32 = 48;
const SOFTWARE_CURSOR_MASK_STRIDE: usize = 6;
const SOFTWARE_CURSOR_HOTSPOT_X: i32 = 6;
const SOFTWARE_CURSOR_HOTSPOT_Y: i32 = 4;
const SOFTWARE_CURSOR_MASK: [u8; 288] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 0
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 2
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 3
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, // row 4
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, // row 5
    0x03, 0x80, 0x00, 0x00, 0x00, 0x00, // row 6
    0x03, 0xC0, 0x00, 0x00, 0x00, 0x00, // row 7
    0x03, 0xE0, 0x00, 0x00, 0x00, 0x00, // row 8
    0x03, 0xF0, 0x00, 0x00, 0x00, 0x00, // row 9
    0x03, 0xF8, 0x00, 0x00, 0x00, 0x00, // row 10
    0x03, 0xFC, 0x00, 0x00, 0x00, 0x00, // row 11
    0x03, 0xFE, 0x00, 0x00, 0x00, 0x00, // row 12
    0x03, 0xFF, 0x00, 0x00, 0x00, 0x00, // row 13
    0x03, 0xFF, 0x80, 0x00, 0x00, 0x00, // row 14
    0x03, 0xFF, 0xC0, 0x00, 0x00, 0x00, // row 15
    0x03, 0xFF, 0xE0, 0x00, 0x00, 0x00, // row 16
    0x03, 0xFF, 0xF0, 0x00, 0x00, 0x00, // row 17
    0x03, 0xFF, 0xF8, 0x00, 0x00, 0x00, // row 18
    0x03, 0xFF, 0xFC, 0x00, 0x00, 0x00, // row 19
    0x03, 0xFF, 0xFE, 0x00, 0x00, 0x00, // row 20
    0x03, 0xFF, 0xFF, 0x00, 0x00, 0x00, // row 21
    0x03, 0xFF, 0xFF, 0x80, 0x00, 0x00, // row 22
    0x03, 0xFF, 0xFF, 0xC0, 0x00, 0x00, // row 23
    0x03, 0xFF, 0xFF, 0xE0, 0x00, 0x00, // row 24
    0x03, 0xFF, 0xC0, 0x00, 0x00, 0x00, // row 25
    0x03, 0xFF, 0xE0, 0x00, 0x00, 0x00, // row 26
    0x03, 0xF7, 0xE0, 0x00, 0x00, 0x00, // row 27
    0x03, 0xE7, 0xF0, 0x00, 0x00, 0x00, // row 28
    0x03, 0xC3, 0xF0, 0x00, 0x00, 0x00, // row 29
    0x03, 0x83, 0xF8, 0x00, 0x00, 0x00, // row 30
    0x03, 0x03, 0xF8, 0x00, 0x00, 0x00, // row 31
    0x02, 0x01, 0xFC, 0x00, 0x00, 0x00, // row 32
    0x00, 0x01, 0xFC, 0x00, 0x00, 0x00, // row 33
    0x00, 0x01, 0xFE, 0x00, 0x00, 0x00, // row 34
    0x00, 0x00, 0xFE, 0x00, 0x00, 0x00, // row 35
    0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, // row 36
    0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, // row 37
    0x00, 0x00, 0x7F, 0x80, 0x00, 0x00, // row 38
    0x00, 0x00, 0x7F, 0x80, 0x00, 0x00, // row 39
    0x00, 0x00, 0x7F, 0x80, 0x00, 0x00, // row 40
    0x00, 0x00, 0x3E, 0x00, 0x00, 0x00, // row 41
    0x00, 0x00, 0x38, 0x00, 0x00, 0x00, // row 42
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 43
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 44
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 45
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 46
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // row 47
];

fn software_cursor_default_location(output_size: Size<i32, Logical>) -> Point<f64, Logical> {
    (
        (output_size.w.max(1) as f64 / 2.0).floor(),
        (output_size.h.max(1) as f64 / 2.0).floor(),
    )
        .into()
}

fn clamp_pointer_location(
    location: Point<f64, Logical>,
    output_size: Size<i32, Logical>,
) -> Point<f64, Logical> {
    let max_x = (output_size.w.max(1) - 1) as f64;
    let max_y = (output_size.h.max(1) - 1) as f64;
    (location.x.clamp(0.0, max_x), location.y.clamp(0.0, max_y)).into()
}

fn software_cursor_rects(
    location: Point<f64, Logical>,
    output_w: i32,
    output_h: i32,
) -> Vec<(Rectangle<i32, Logical>, SoftwareCursorColor)> {
    let bounds = Rectangle::new((0, 0).into(), (output_w.max(0), output_h.max(0)).into());
    if bounds.size.w == 0 || bounds.size.h == 0 {
        return Vec::new();
    }

    let location =
        clamp_pointer_location(location, Size::<i32, Logical>::from((output_w, output_h)));
    let origin = software_cursor_mask_origin(location);

    let mut rects = Vec::new();
    append_cursor_mask_spans(
        &mut rects,
        bounds,
        origin,
        SoftwareCursorColor::White,
        |mask_x, mask_y| software_cursor_mask_pixel(mask_x, mask_y),
    );
    append_cursor_mask_spans(
        &mut rects,
        bounds,
        origin,
        SoftwareCursorColor::Black,
        |mask_x, mask_y| software_cursor_outline_pixel(mask_x, mask_y),
    );
    rects
}

fn software_cursor_mask_origin(location: Point<f64, Logical>) -> Point<i32, Logical> {
    (
        location.x.round() as i32 - SOFTWARE_CURSOR_HOTSPOT_X,
        location.y.round() as i32 - SOFTWARE_CURSOR_HOTSPOT_Y,
    )
        .into()
}

fn append_cursor_mask_spans(
    rects: &mut Vec<(Rectangle<i32, Logical>, SoftwareCursorColor)>,
    bounds: Rectangle<i32, Logical>,
    origin: Point<i32, Logical>,
    color: SoftwareCursorColor,
    pixel_on: impl Fn(i32, i32) -> bool,
) {
    for mask_y in 0..SOFTWARE_CURSOR_MASK_HEIGHT {
        let mut span_start = None;
        for mask_x in 0..=SOFTWARE_CURSOR_MASK_WIDTH {
            let on = mask_x < SOFTWARE_CURSOR_MASK_WIDTH && pixel_on(mask_x, mask_y);
            match (span_start, on) {
                (None, true) => span_start = Some(mask_x),
                (Some(start), false) => {
                    push_cursor_rect(
                        rects,
                        bounds,
                        Rectangle::new(
                            (origin.x + start, origin.y + mask_y).into(),
                            (mask_x - start, 1).into(),
                        ),
                        color,
                    );
                    span_start = None;
                }
                _ => {}
            }
        }
    }
}

fn push_cursor_rect(
    rects: &mut Vec<(Rectangle<i32, Logical>, SoftwareCursorColor)>,
    bounds: Rectangle<i32, Logical>,
    rect: Rectangle<i32, Logical>,
    color: SoftwareCursorColor,
) {
    if let Some(clipped) = rect.intersection(bounds) {
        if clipped.size.w > 0 && clipped.size.h > 0 {
            rects.push((clipped, color));
        }
    }
}

fn software_cursor_mask_pixel(mask_x: i32, mask_y: i32) -> bool {
    if !(0..SOFTWARE_CURSOR_MASK_WIDTH).contains(&mask_x)
        || !(0..SOFTWARE_CURSOR_MASK_HEIGHT).contains(&mask_y)
    {
        return false;
    }
    let byte_index = mask_y as usize * SOFTWARE_CURSOR_MASK_STRIDE + mask_x as usize / 8;
    let bit = 0x80 >> (mask_x as usize % 8);
    SOFTWARE_CURSOR_MASK[byte_index] & bit != 0
}

fn software_cursor_outline_pixel(mask_x: i32, mask_y: i32) -> bool {
    if software_cursor_mask_pixel(mask_x, mask_y) {
        return false;
    }
    for dy in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if software_cursor_mask_pixel(mask_x - dx, mask_y - dy) {
                return true;
            }
        }
    }
    software_cursor_mask_pixel(mask_x - 2, mask_y - 2)
}

fn draw_software_cursor(
    location: Point<f64, Logical>,
    root_geometry: crate::root_geometry::RootGeometrySnapshot,
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) {
    let logical_size = root_geometry.logical_size_i32();
    for (logical_rect, color) in
        software_cursor_rects(location, logical_size.width, logical_size.height)
            .into_iter()
            .rev()
    {
        let crop = root_geometry.physical_crop(crate::root_geometry::LogicalRect {
            x: logical_rect.loc.x as f64,
            y: logical_rect.loc.y as f64,
            width: logical_rect.size.w as f64,
            height: logical_rect.size.h as f64,
        });
        let rect = Rectangle::<i32, Physical>::new(
            (crop.x, crop.y).into(),
            (crop.width, crop.height).into(),
        );
        let (b, g, r) = match color {
            SoftwareCursorColor::White => (0xFF, 0xFF, 0xFF),
            SoftwareCursorColor::Black => (0x00, 0x00, 0x00),
        };
        for y in rect.loc.y.max(0)..(rect.loc.y + rect.size.h).min(output_h) {
            let row_start = (y as usize).saturating_mul(target_stride);
            for x in rect.loc.x.max(0)..(rect.loc.x + rect.size.w).min(output_w) {
                let idx = row_start.saturating_add((x as usize).saturating_mul(4));
                if idx.saturating_add(4) <= target.len() {
                    target[idx] = b;
                    target[idx + 1] = g;
                    target[idx + 2] = r;
                    target[idx + 3] = 0x00;
                }
            }
        }
    }
}

fn lock_state(
    shared_state: &Arc<Mutex<CompositorState>>,
) -> std::sync::MutexGuard<'_, CompositorState> {
    match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn surface_key(surface: &WlSurface) -> ObjectId {
    surface.id()
}

fn same_surface(left: &WlSurface, right: &WlSurface) -> bool {
    surface_key(left) == surface_key(right)
}

fn surface_id(surface: &WlSurface) -> u32 {
    surface.id().protocol_id()
}

fn transform_from_rotation(rotation: OutputRotation) -> Transform {
    OutputRotationModel::new(rotation).output_transform()
}

fn direct_present_supported_for_rotation(rotation: OutputRotation) -> bool {
    match rotation {
        OutputRotation::Deg0 | OutputRotation::Deg180 => true,
        // Quarter-turn direct present is compositor-rendered into a physical
        // scanout-sized GBM buffer; it does not require KMS plane rotate-90/270.
        OutputRotation::Deg90 | OutputRotation::Deg270 => true,
    }
}

fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

impl OutputHandler for RuntimeWaylandState {}

delegate_xdg_shell!(RuntimeWaylandState);
delegate_xdg_decoration!(RuntimeWaylandState);
delegate_compositor!(RuntimeWaylandState);
delegate_data_device!(RuntimeWaylandState);
delegate_output!(RuntimeWaylandState);
delegate_shm!(RuntimeWaylandState);
delegate_dmabuf!(RuntimeWaylandState);
delegate_fractional_scale!(RuntimeWaylandState);
delegate_viewporter!(RuntimeWaylandState);
delegate_seat!(RuntimeWaylandState);

#[cfg(test)]
mod tests {
    use super::{
        AcceptedMaterialIdentity, AcceptedPopupMaterial, AcceptedSurfaceMaterial,
        AtomicPlaneLayout, DrmFourcc, GBM_BUFFER_FROM_BO_PRESERVE_EXPLICIT_MODIFIER,
        ClaimedHostOutput, ClaimedPresentationPipeline, GLES_INTERMEDIATE_RENDER_FORMAT,
        HostBackendState, HostRuntimeLoopData, OpenedHostDevice, OutputIdentity, PlaneSelection,
        RoleSurfaceMapping, Root4ConsumerStage,
        PresentationToken, RootGeometryFlipBoundary, RuntimeGeometryMutation,
        RootGeometryStageStore, RuntimeGeometryMutationQueue, RuntimeSurfaceRole, RuntimeWaylandState,
        ShellOverlayToggleShortcut, StartupPresentOwnership, build_host_gles_renderer_state,
        compile_native_clip_program, materialize_native_surface_elements,
        copy_renderer_pixels_to_dumb,
        direct_present_supported_for_rotation, native_materialized_destination_rect,
        native_materialized_local_clip, native_materialized_source_rect, native_source_sample,
        overlay_scanout_format_supports_alpha,
        parse_shell_overlay_toggle_shortcut, pid_matches_or_descends_from,
        render_output_size_before_transform, scene_texture_transform, screen_capture_src_flipped,
        render_elements_to_texture,
        composite_scene_texture_to_physical_scanout,
        select_atomic_plane_zpos_values, select_preferred_scanout_format, select_primary_path,
        remap_damage_to_materialized_destination, source_rect_from_bbox_and_geometry,
        stage_runtime_root_geometry_consumers,
        transform_from_rotation, embedded_toplevel_decoration_mode,
        activate_root_geometry_stage, install_root_geometry_stage, lock_state,
    };
    use crate::output_rotation_model::OutputRotationModel;
    use crate::model::{
        CompositorOverlayKind, NativePaneHostRequest, NativeTargetClass, OutputRotation,
        OverlayCaptureCapability, OverlayRect, OverlayRegionStatus, PaneGeometry,
        PaneGeometryCoordinateSpace, PaneId, ProcessSpec, RuntimeFocusTarget,
    };
    use crate::process_manager::{ProcessController, ProcessExit};
    use crate::screen_capture::ScreenCaptureStore;
    use crate::state::CompositorState;
    use smithay::input::keyboard::{Keysym, ModifiersState, keysyms};
    use smithay::backend::drm::DrmNode;
    use smithay::backend::renderer::utils::DamageSet;
    use smithay::backend::renderer::{
        Bind, Color32F, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Renderer,
    };
    use smithay::backend::renderer::element::Element;
    use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
    use smithay::reexports::wayland_server::Display;
    use smithay::reexports::calloop::EventLoop;
    use smithay::reexports::drm as drm_api;
    use smithay::reexports::drm::control::Mode as DrmMode;
    use smithay::utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Size, Transform};
    use std::fs::OpenOptions;
    use std::os::fd::{AsFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, mpsc};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
    use wayland_client::globals::registry_queue_init;
    use wayland_client::protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_registry, wl_region, wl_shm, wl_shm_pool,
        wl_subcompositor, wl_subsurface, wl_surface,
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
    };

    #[derive(Default)]
    struct SurfaceTreeClient {
        configured: bool,
    }

    impl Dispatch<wl_registry::WlRegistry, wayland_client::globals::GlobalListContents>
        for SurfaceTreeClient
    {
        fn event(
            _state: &mut Self,
            _proxy: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &wayland_client::globals::GlobalListContents,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(SurfaceTreeClient: ignore wl_compositor::WlCompositor);
    delegate_noop!(SurfaceTreeClient: ignore wl_subcompositor::WlSubcompositor);
    delegate_noop!(SurfaceTreeClient: ignore wl_shm::WlShm);
    delegate_noop!(SurfaceTreeClient: ignore wl_shm_pool::WlShmPool);
    delegate_noop!(SurfaceTreeClient: ignore wl_surface::WlSurface);
    delegate_noop!(SurfaceTreeClient: ignore wl_subsurface::WlSubsurface);
    delegate_noop!(SurfaceTreeClient: ignore wl_buffer::WlBuffer);
    delegate_noop!(SurfaceTreeClient: ignore wl_callback::WlCallback);
    delegate_noop!(SurfaceTreeClient: ignore wl_region::WlRegion);
    delegate_noop!(SurfaceTreeClient: ignore xdg_positioner::XdgPositioner);
    delegate_noop!(SurfaceTreeClient: ignore xdg_toplevel::XdgToplevel);
    delegate_noop!(SurfaceTreeClient: ignore xdg_popup::XdgPopup);

    impl Dispatch<xdg_wm_base::XdgWmBase, ()> for SurfaceTreeClient {
        fn event(
            _state: &mut Self,
            proxy: &xdg_wm_base::XdgWmBase,
            event: xdg_wm_base::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                proxy.pong(serial);
            }
        }
    }

    impl Dispatch<xdg_surface::XdgSurface, ()> for SurfaceTreeClient {
        fn event(
            state: &mut Self,
            proxy: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                proxy.ack_configure(serial);
                state.configured = true;
            }
        }
    }

    fn spawn_real_shm_surface_tree(
        socket: UnixStream,
    ) -> (mpsc::Receiver<(u32, u32, u32)>, mpsc::Sender<()>) {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let connection = Connection::from_socket(socket).expect("client socket must connect");
            let (globals, mut queue) = registry_queue_init::<SurfaceTreeClient>(&connection)
                .expect("fixture globals must roundtrip");
            let qh = queue.handle();
            let compositor = globals
                .bind::<wl_compositor::WlCompositor, _, _>(&qh, 1..=6, ())
                .expect("wl_compositor must exist");
            let subcompositor = globals
                .bind::<wl_subcompositor::WlSubcompositor, _, _>(&qh, 1..=1, ())
                .expect("wl_subcompositor must exist");
            let shm = globals
                .bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ())
                .expect("wl_shm must exist");
            let wm_base = globals
                .bind::<xdg_wm_base::XdgWmBase, _, _>(&qh, 1..=6, ())
                .expect("xdg_wm_base must exist");

            let width = 100_i32;
            let height = 50_i32;
            let child_width = 16_i32;
            let child_height = 12_i32;
            let popup_width = 18_i32;
            let popup_height = 10_i32;
            let root_bytes = (width * height * 4) as usize;
            let child_bytes = (child_width * child_height * 4) as usize;
            let popup_bytes = (popup_width * popup_height * 4) as usize;
            let fd =
                rustix::fs::memfd_create("t316-real-surface-tree", rustix::fs::MemfdFlags::CLOEXEC)
                    .expect("fixture memfd must allocate");
            rustix::fs::ftruncate(&fd, (root_bytes + child_bytes + popup_bytes) as u64)
                .expect("fixture memfd must size");
            let mut pixels = vec![0_u8; root_bytes + child_bytes + popup_bytes];
            for pixel in pixels[..root_bytes].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0x00, 0x00, 0xff, 0xff]);
            }
            for pixel in pixels[root_bytes..root_bytes + child_bytes].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0x00, 0xff, 0x00, 0xff]);
            }
            for pixel in pixels[root_bytes + child_bytes..].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0xff, 0x00, 0x00, 0xff]);
            }
            assert_eq!(
                rustix::io::pwrite(&fd, &pixels, 0).expect("fixture pixels must write"),
                pixels.len()
            );
            let pool = shm.create_pool(fd.as_fd(), pixels.len() as i32, &qh, ());
            let root_buffer = pool.create_buffer(
                0,
                width,
                height,
                width * 4,
                wl_shm::Format::Argb8888,
                &qh,
                (),
            );
            let child_buffer = pool.create_buffer(
                root_bytes as i32,
                child_width,
                child_height,
                child_width * 4,
                wl_shm::Format::Argb8888,
                &qh,
                (),
            );
            let popup_buffer = pool.create_buffer(
                (root_bytes + child_bytes) as i32,
                popup_width,
                popup_height,
                popup_width * 4,
                wl_shm::Format::Argb8888,
                &qh,
                (),
            );
            let root = compositor.create_surface(&qh, ());
            let child = compositor.create_surface(&qh, ());
            let subsurface = subcompositor.get_subsurface(&child, &root, &qh, ());
            subsurface.set_position(7, 9);
            child.attach(Some(&child_buffer), 0, 0);
            child.damage_buffer(0, 0, child_width, child_height);
            child.commit();
            root.attach(Some(&root_buffer), 0, 0);
            root.damage_buffer(0, 0, width, height);
            root.commit();
            let popup_parent = compositor.create_surface(&qh, ());
            let popup_parent_xdg = wm_base.get_xdg_surface(&popup_parent, &qh, ());
            let _popup_parent_role = popup_parent_xdg.get_toplevel(&qh, ());
            popup_parent.commit();
            connection.flush().expect("popup parent role must flush");
            let mut client_state = SurfaceTreeClient::default();
            while !client_state.configured {
                queue
                    .blocking_dispatch(&mut client_state)
                    .expect("popup parent configure must dispatch");
            }
            let popup_surface = compositor.create_surface(&qh, ());
            let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
            let positioner = wm_base.create_positioner(&qh, ());
            positioner.set_size(popup_width, popup_height);
            positioner.set_anchor_rect(20, 15, 1, 1);
            let _popup_role =
                popup_xdg_surface.get_popup(Some(&popup_parent_xdg), &positioner, &qh, ());
            popup_surface.commit();
            connection.flush().expect("popup initial commit must flush");
            client_state.configured = false;
            while !client_state.configured {
                queue
                    .blocking_dispatch(&mut client_state)
                    .expect("popup configure must dispatch");
            }
            popup_surface.attach(Some(&popup_buffer), 0, 0);
            popup_surface.damage_buffer(0, 0, popup_width, popup_height);
            popup_surface.commit();
            connection.flush().expect("fixture commits must flush");
            ready_tx
                .send((
                    root.id().protocol_id(),
                    child.id().protocol_id(),
                    popup_surface.id().protocol_id(),
                ))
                .expect("fixture ids must publish");
            release_rx.recv().expect("server must finish tree proof");
        });
        (ready_rx, release_tx)
    }

    #[derive(Default)]
    struct NoopProcessController;

    impl ProcessController for NoopProcessController {
        fn spawn(
            &mut self,
            _spec: &ProcessSpec,
            _extra_env: &std::collections::BTreeMap<String, String>,
        ) -> Result<u32, String> {
            Ok(1)
        }

        fn terminate(&mut self, _pid: u32) -> Result<(), String> {
            Ok(())
        }

        fn reap_exited(&mut self) -> Vec<ProcessExit> {
            Vec::new()
        }
    }

    fn test_host_runtime_loop() -> (
        HostRuntimeLoopData,
        EventLoop<'static, HostRuntimeLoopData>,
        ScreenCaptureStore,
    ) {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        let shared_state = Arc::new(Mutex::new(state));
        let display: Display<RuntimeWaylandState> = Display::new().unwrap();
        let display_handle = display.handle();
        let wayland_state =
            RuntimeWaylandState::new(display_handle.clone(), Arc::clone(&shared_state)).unwrap();
        let capture = ScreenCaptureStore::default();
        let host_backend = HostBackendState::for_root_geometry_test(capture.clone());
        let event_loop = EventLoop::<HostRuntimeLoopData>::try_new().unwrap();
        let loop_signal = event_loop.get_signal();
        (
            HostRuntimeLoopData {
                shared_state,
                display_handle,
                loop_signal,
                wayland_state,
                host_backend,
                runtime_control: None,
                pending_rotation_response: None,
                root_geometry_queue: RuntimeGeometryMutationQueue::default(),
                pending_geometry_mutation: None,
                root_geometry_flip_boundary: RootGeometryFlipBoundary::default(),
                pending_reclaim_publication: None,
                root_geometry_stage_failure: None,
            },
            event_loop,
            capture,
        )
    }

    fn stage_capture_for_pending_geometry(runtime: &HostRuntimeLoopData) {
        let staged = runtime
            .wayland_state
            .staged_root_geometry
            .as_ref()
            .expect("root4 transaction must be staged");
        runtime
            .host_backend
            .screen_capture
            .update_root4_scanout_xrgb8888(
                &[0, 0, 0, 0],
                4,
                1,
                1,
                false,
                staged.committed.snapshot,
                &staged.captures,
            );
    }

    fn test_pipeline(pending_atomic_modeset: bool) -> ClaimedPresentationPipeline {
        ClaimedPresentationPipeline {
            crtc: drm_api::control::from_u32(1).unwrap(),
            dumb_buffers: None,
            dumb_front_buffer: 0,
            dumb_back_buffer: 1,
            atomic_commit_state: None,
            pending_atomic_modeset,
            flip_pending: false,
            pending_flip_source: None,
            pending_presentation_token: None,
            gles_renderer: None,
        }
    }

    fn test_claimed_output(device_id: u64, path: &str) -> ClaimedHostOutput {
        // drm::Mode is transparent over the all-integer kernel modeinfo record;
        // a zeroed value is sufficient because lifecycle tests never inspect it.
        let mode: DrmMode = unsafe { std::mem::zeroed() };
        ClaimedHostOutput {
            device_id,
            mode,
            startup_present_ownership: StartupPresentOwnership::Dumb,
            identity: OutputIdentity {
                device_path: PathBuf::from(path),
                connector_name: format!("TEST-{device_id}"),
                connector_id: device_id as u32,
            },
        }
    }

    fn test_opened_device(
        path: &str,
        claimed_pipeline: Option<ClaimedPresentationPipeline>,
        prepared_pipeline: Option<ClaimedPresentationPipeline>,
    ) -> OpenedHostDevice {
        let fd: OwnedFd = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap()
            .into();
        OpenedHostDevice {
            path: PathBuf::from(path),
            // The lifecycle under test does not inspect the node. DrmNode is an
            // integer device id plus NodeType, whose zero discriminant is Primary.
            node: unsafe { std::mem::zeroed() },
            fd,
            claimed_pipeline,
            prepared_pipeline,
        }
    }

    #[derive(Default)]
    struct DeterministicStageStore {
        events: Vec<(&'static str, u64)>,
    }

    impl RootGeometryStageStore for DeterministicStageStore {
        fn begin_stage(&mut self, generation: u64) {
            self.events.push(("begin", generation));
        }

        fn commit_stage(&mut self, generation: u64) {
            self.events.push(("commit", generation));
        }

        fn discard_stage(&mut self, generation: u64) {
            self.events.push(("discard", generation));
        }
    }

    #[test]
    #[ignore = "requires an isolated Linux DRM render device; run with T316_GPU_PROOF_DEVICE"]
    fn gpu_native_clip_shader_import_and_readback_proof() {
        let path = PathBuf::from(
            std::env::var_os("T316_GPU_PROOF_DEVICE")
                .expect("T316_GPU_PROOF_DEVICE must name the non-live DRM primary node"),
        );
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("GPU proof device must be accessible");
        let node = DrmNode::from_path(&path).expect("GPU proof path must be a DRM node");
        let mut state = build_host_gles_renderer_state(
            &file.into(),
            node,
            &path,
            (8, 8),
            DrmFourcc::Xrgb8888,
            None,
        )
        .expect("production GBM/EGL/GLES renderer initialization must succeed");
        let program = compile_native_clip_program(&mut state.renderer)
            .expect("production native clip shader must compile on the target GPU");

        let (server_socket, client_socket) = UnixStream::pair().expect("fixture socket pair");
        let mut display: Display<RuntimeWaylandState> = Display::new().unwrap();
        let mut display_handle = display.handle();
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            false,
            Box::new(NoopProcessController),
        )));
        let mut wayland_state = RuntimeWaylandState::new(display_handle.clone(), shared_state)
            .expect("fixture Wayland state must initialize");
        let server_client = display_handle
            .insert_client(
                server_socket,
                Arc::new(super::RuntimeClientState::default()),
            )
            .expect("fixture client must insert");
        let (surface_ids, release_client) = spawn_real_shm_surface_tree(client_socket);
        let (root_id, child_id, popup_id) = loop {
            display
                .dispatch_clients(&mut wayland_state)
                .expect("fixture requests must dispatch");
            if wayland_state.main_toplevel.is_none() && !wayland_state.pending_toplevels.is_empty()
            {
                let parent = wayland_state.pending_toplevels.remove(0);
                wayland_state
                    .configure_toplevel_for_role(&parent, super::RuntimeSurfaceRole::MainApp);
                wayland_state.main_toplevel = Some(parent);
            }
            display.flush_clients().expect("fixture replies must flush");
            if let Ok(ids) = surface_ids.try_recv() {
                break ids;
            }
            std::thread::yield_now();
        };
        display
            .dispatch_clients(&mut wayland_state)
            .expect("fixture commits must dispatch");
        let root: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface =
            server_client
                .object_from_protocol_id(&display_handle, root_id)
                .expect("server must recover real root surface");
        let child: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface =
            server_client
                .object_from_protocol_id(&display_handle, child_id)
                .expect("server must recover real child surface");
        let popup: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface =
            server_client
                .object_from_protocol_id(&display_handle, popup_id)
                .expect("server must recover real xdg popup surface");
        smithay::backend::renderer::utils::import_surface_tree(&mut state.renderer, &root)
            .expect("production surface-tree import must accept real SHM buffers");
        let raw_elements: Vec<
            smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                smithay::backend::renderer::gles::GlesRenderer,
            >,
        > = smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
            &mut state.renderer,
            &root,
            (0, 0),
            1.0,
            1.0,
            smithay::backend::renderer::element::Kind::Unspecified,
        );
        assert_eq!(
            raw_elements.len(),
            2,
            "real root and subsurface must materialize"
        );
        let child_element_id = smithay::backend::renderer::element::Id::from(&child);
        let expected_root_element_id = smithay::backend::renderer::element::Id::from(&root);
        assert!(
            raw_elements
                .iter()
                .any(|element| element.id() == &child_element_id)
        );
        let root_element_id = raw_elements
            .iter()
            .find(|element| element.id() == &expected_root_element_id)
            .expect("real root render element must retain surface identity")
            .id()
            .clone();
        let projection = crate::root_geometry::NativeBufferProjection {
            origin_x: 13,
            origin_y: 26,
            width_px: 131,
            height_px: 67,
            fractional_phase_x: 0.325,
            fractional_phase_y: 0.65,
            logical_clip: crate::root_geometry::LogicalRect {
                x: 10.25,
                y: 20.5,
                width: 100.25,
                height: 50.75,
            },
            scale_factor: 1.3,
        };
        let mut materialized = materialize_native_surface_elements(
            raw_elements,
            Some(&root_element_id),
            projection,
            &program,
            100.0,
        );
        smithay::backend::renderer::utils::import_surface_tree(&mut state.renderer, &popup)
            .expect("production popup surface-tree import must accept real SHM buffer");
        let popup_raw: Vec<
            smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<
                smithay::backend::renderer::gles::GlesRenderer,
            >,
        > = smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
            &mut state.renderer,
            &popup,
            (30, 20),
            1.0,
            1.0,
            smithay::backend::renderer::element::Kind::Unspecified,
        );
        assert_eq!(
            popup_raw.len(),
            1,
            "real xdg popup must produce one render element"
        );
        let popup_element_id = smithay::backend::renderer::element::Id::from(&popup);
        let popup_materialized =
            materialize_native_surface_elements(popup_raw, None, projection, &program, 100.0);
        assert_eq!(
            popup_materialized[0].id(),
            &popup_element_id,
            "real popup identity must survive production materialization"
        );
        assert_eq!(
            popup_materialized[0].geometry(1.0.into()),
            Rectangle::new((30, 20).into(), (18, 10).into()),
            "popup geometry must remain native while sharing the root clip authority"
        );
        materialized.extend(popup_materialized);
        let root_element = materialized
            .iter()
            .find(|element| element.id() == &root_element_id)
            .expect("materialized root must exist");
        assert_eq!(
            root_element.geometry(1.0.into()),
            Rectangle::new((13, 26).into(), (131, 67).into())
        );
        assert_eq!(
            root_element
                .damage_since(1.0.into(), None)
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![Rectangle::new((0, 1).into(), (131, 66).into())],
            "real committed SHM damage must be remapped and fractional-clipped"
        );
        let mut tree_target = state
            .renderer
            .create_buffer(GLES_INTERMEDIATE_RENDER_FORMAT, (160, 100).into())
            .expect("tree target must allocate");
        render_elements_to_texture(
            &mut state.renderer,
            &path,
            &mut tree_target,
            (160, 100).into(),
            &materialized,
            1.0,
            "real Wayland surface-tree proof target",
        )
        .expect("actual NativeMaterializedRenderElement draw must succeed");
        let mut rotation_red_counts = Vec::new();
        for rotation in [
            OutputRotation::Deg0,
            OutputRotation::Deg90,
            OutputRotation::Deg180,
            OutputRotation::Deg270,
        ] {
            let scanout_size: Size<i32, Physical> = match rotation {
                OutputRotation::Deg0 | OutputRotation::Deg180 => (160, 100).into(),
                OutputRotation::Deg90 | OutputRotation::Deg270 => (100, 160).into(),
            };
            let mut scanout_texture: smithay::backend::renderer::gles::GlesTexture = state
                .renderer
                .create_buffer(
                    GLES_INTERMEDIATE_RENDER_FORMAT,
                    Size::<i32, BufferCoords>::from((scanout_size.w, scanout_size.h)),
                )
                .expect("rotated scanout target must allocate");
            let mut scanout_target = state
                .renderer
                .bind(&mut scanout_texture)
                .expect("rotated scanout target must bind");
            composite_scene_texture_to_physical_scanout(
                &mut state.renderer,
                &path,
                &mut scanout_target,
                &tree_target,
                scanout_size,
                1.3,
                rotation,
                (-1000.0, -1000.0).into(),
                (160, 100).into(),
            )
            .expect("production rotated scene composite must succeed");
            let mapping = state
                .renderer
                .copy_framebuffer(
                    &scanout_target,
                    Rectangle::from_size(Size::<i32, BufferCoords>::from((
                        scanout_size.w,
                        scanout_size.h,
                    ))),
                    DrmFourcc::Xrgb8888,
                )
                .expect("rotated production scanout must read back");
            let pixels = state
                .renderer
                .map_texture(&mapping)
                .expect("rotated scanout mapping must succeed");
            rotation_red_counts.push(
                pixels
                    .chunks_exact(4)
                    .filter(|pixel| pixel[2] > 200 && pixel[1] < 50 && pixel[0] < 50)
                    .count(),
            );
        }
        assert!(
            rotation_red_counts.iter().all(|count| *count > 5_000),
            "all four production rotations must retain the real surface-tree root pixels: {rotation_red_counts:?}"
        );
        assert!(
            rotation_red_counts[0] == rotation_red_counts[2]
                && rotation_red_counts[1] == rotation_red_counts[3],
            "opposite production rotations must preserve identical sampled coverage: {rotation_red_counts:?}"
        );
        release_client
            .send(())
            .expect("fixture client must release");

        let red_xrgb = [0_u8, 0, 255, 0].repeat(64);
        let shm_texture = state
            .renderer
            .import_memory(
                &red_xrgb,
                DrmFourcc::Xrgb8888,
                Size::<i32, BufferCoords>::from((8, 8)),
                false,
            )
            .expect("Smithay SHM/memory texture import must succeed");

        let render_and_count = |renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
                                target: &mut smithay::backend::renderer::gles::GlesTexture,
                                texture: &smithay::backend::renderer::gles::GlesTexture,
                                channel: usize| {
            let size = Size::<i32, Physical>::from((8, 8));
            let damage = Rectangle::from_size(size);
            let mut target = renderer
                .bind(target)
                .expect("offscreen target bind must succeed");
            let mut frame = renderer
                .render(&mut target, size, Transform::Normal)
                .expect("offscreen render pass must begin");
            frame
                .clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[damage])
                .expect("offscreen clear must succeed");
            frame
                .render_texture_from_to(
                    texture,
                    Rectangle::new((0.0, 0.0).into(), (8.0, 8.0).into()),
                    damage,
                    &[damage],
                    &[],
                    Transform::Normal,
                    1.0,
                    Some(&program),
                    &[
                        smithay::backend::renderer::gles::Uniform::new(
                            "clip_min",
                            (2.0_f32, 2.0_f32),
                        ),
                        smithay::backend::renderer::gles::Uniform::new(
                            "clip_max",
                            (6.0_f32, 6.0_f32),
                        ),
                        smithay::backend::renderer::gles::Uniform::new("target_height", 8.0_f32),
                    ],
                )
                .expect("production native clip shader draw must succeed");
            let _ = frame.finish().expect("offscreen render pass must finish");
            let mapping = renderer
                .copy_framebuffer(
                    &target,
                    Rectangle::from_size(Size::<i32, BufferCoords>::from((8, 8))),
                    DrmFourcc::Xrgb8888,
                )
                .expect("shader output readback must succeed");
            renderer
                .map_texture(&mapping)
                .expect("shader output mapping must succeed")
                .chunks_exact(4)
                .filter(|pixel| pixel[channel] > 200)
                .count()
        };

        assert_eq!(
            render_and_count(
                &mut state.renderer,
                &mut state.target_texture,
                &shm_texture,
                2,
            ),
            16,
            "SHM material must be clipped to the exact 4x4 destination"
        );

        let mut dmabuf = state
            .direct_scanout
            .as_ref()
            .expect("production GBM scanout dmabuf allocation must succeed")
            .buffers[0]
            .dmabuf
            .clone();
        {
            let size = Size::<i32, Physical>::from((8, 8));
            let damage = Rectangle::from_size(size);
            let mut target = state
                .renderer
                .bind(&mut dmabuf)
                .expect("production dmabuf render target bind must succeed");
            let mut frame = state
                .renderer
                .render(&mut target, size, Transform::Normal)
                .expect("dmabuf render pass must begin");
            frame
                .clear(Color32F::new(0.0, 1.0, 0.0, 1.0), &[damage])
                .expect("dmabuf clear must succeed");
            let _ = frame.finish().expect("dmabuf render pass must finish");
        }
        let dmabuf_texture = state
            .renderer
            .import_dmabuf(&dmabuf, None)
            .expect("production EGL dmabuf texture import must succeed");
        assert_eq!(
            render_and_count(
                &mut state.renderer,
                &mut state.target_texture,
                &dmabuf_texture,
                1,
            ),
            16,
            "dmabuf material must survive shader import and exact clipped readback"
        );
    }

    #[test]
    fn production_consumer_staging_rolls_back_every_ordered_position() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("native"),
                content_id: Some("content".to_string()),
                binding_id: Some("binding".to_string()),
                launch_token: None,
                revision: 1,
                geometry: PaneGeometry {
                    x: 10.25,
                    y: 20.5,
                    width: 100.25,
                    height: 50.75,
                    coordinate_space: PaneGeometryCoordinateSpace::CompositorLogical,
                },
                target: NativeTargetClass::Terminal,
                process: ProcessSpec {
                    command: "foot".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                },
            }])
            .unwrap();
        let before = state.root_geometry_snapshot().unwrap();
        let prepared = state
            .prepare_root4_display_scale_from_config(
                1.3,
                crate::root_geometry::DisplayScaleSource::Config,
            )
            .unwrap();
        for consumer in [
            Root4ConsumerStage::RootLayoutBackgroundAndChrome,
            Root4ConsumerStage::CompositedContentOverlaysAndHitRegions,
            Root4ConsumerStage::NativeBuffers,
            Root4ConsumerStage::SurfaceAndPaneViewports,
            Root4ConsumerStage::Capture,
            Root4ConsumerStage::Input,
            Root4ConsumerStage::Status,
        ] {
            assert!(
                stage_runtime_root_geometry_consumers(&state, prepared, Some(consumer)).is_err()
            );
            assert_eq!(state.root_geometry_snapshot(), Some(before));
        }
        let staged = stage_runtime_root_geometry_consumers(&state, prepared, None).unwrap();
        assert!(staged.is_coherent());
        assert_eq!(staged.native_materializations.len(), 1);
        assert_eq!(staged.native_materializations[0].1.width_px, 131);
        assert_eq!(staged.native_materializations[0].1.height_px, 67);
    }

    #[test]
    fn host_runtime_rolls_back_each_consumer_and_continues_from_the_committed_base() {
        for consumer in [
            Root4ConsumerStage::RootLayoutBackgroundAndChrome,
            Root4ConsumerStage::CompositedContentOverlaysAndHitRegions,
            Root4ConsumerStage::NativeBuffers,
            Root4ConsumerStage::SurfaceAndPaneViewports,
            Root4ConsumerStage::Capture,
            Root4ConsumerStage::Input,
            Root4ConsumerStage::Status,
        ] {
            let (mut runtime, _event_loop, capture) = test_host_runtime_loop();
            let active = lock_state(&runtime.shared_state)
                .root_geometry_snapshot()
                .unwrap();
            runtime.root_geometry_stage_failure = Some(consumer);
            runtime
                .root_geometry_queue
                .push(RuntimeGeometryMutation::Scale {
                    factor: 1.5,
                    source: crate::root_geometry::DisplayScaleSource::Config,
                });
            runtime.stage_next_root_geometry_mutation();

            assert!(runtime.wayland_state.staged_root_geometry.is_none());
            assert!(runtime.pending_geometry_mutation.is_none());
            assert_eq!(capture.root4_generations_for_test(), (None, None));
            assert_eq!(
                lock_state(&runtime.shared_state).root_geometry_snapshot(),
                Some(active)
            );

            runtime.root_geometry_stage_failure = None;
            runtime
                .root_geometry_queue
                .push(RuntimeGeometryMutation::Scale {
                    factor: 1.25,
                    source: crate::root_geometry::DisplayScaleSource::Config,
                });
            runtime.stage_next_root_geometry_mutation();
            assert_eq!(
                runtime
                    .wayland_state
                    .staged_root_geometry
                    .as_ref()
                    .unwrap()
                    .committed
                    .snapshot
                    .generation,
                active.generation + 1
            );
        }
    }

    #[test]
    fn host_runtime_fifo_activates_only_exact_completions_and_responds_after_activation() {
        let (mut runtime, _event_loop, capture) = test_host_runtime_loop();
        let active = lock_state(&runtime.shared_state)
            .root_geometry_snapshot()
            .unwrap();
        let (rotation_tx, rotation_rx) = std::sync::mpsc::sync_channel(1);
        runtime
            .root_geometry_queue
            .push(RuntimeGeometryMutation::Scale {
                factor: 1.5,
                source: crate::root_geometry::DisplayScaleSource::Config,
            });
        runtime
            .root_geometry_queue
            .push(RuntimeGeometryMutation::Rotation {
                rotation: OutputRotation::Deg90,
                response: rotation_tx,
            });
        runtime
            .root_geometry_queue
            .push(RuntimeGeometryMutation::Scale {
                factor: 1.5,
                source: crate::root_geometry::DisplayScaleSource::Config,
            });

        runtime.stage_next_root_geometry_mutation();
        stage_capture_for_pending_geometry(&runtime);
        runtime
            .root_geometry_flip_boundary
            .mark_queued(PresentationToken(10));
        assert!(!runtime.complete_root_geometry_presentations(&[PresentationToken(9)]));
        assert_eq!(
            lock_state(&runtime.shared_state).root_geometry_snapshot(),
            Some(active)
        );
        assert!(runtime.complete_root_geometry_presentations(&[PresentationToken(10)]));
        assert_eq!(
            capture.root4_generations_for_test(),
            (Some(active.generation + 1), None)
        );

        runtime.stage_next_root_geometry_mutation();
        assert!(matches!(
            rotation_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        stage_capture_for_pending_geometry(&runtime);
        runtime
            .root_geometry_flip_boundary
            .mark_queued(PresentationToken(11));
        assert!(runtime.complete_root_geometry_presentations(&[PresentationToken(11)]));
        assert_eq!(rotation_rx.recv().unwrap(), Ok(()));

        runtime.stage_next_root_geometry_mutation();
        let third = runtime
            .wayland_state
            .staged_root_geometry
            .as_ref()
            .unwrap()
            .committed
            .snapshot;
        assert_eq!(third.generation, active.generation + 3);
        assert_eq!(third.factor, 1.5);
        assert_eq!(third.rotation, OutputRotation::Deg90);
    }

    fn seed_reclaim_lifecycle(backend: &mut HostBackendState, replacement_device_id: u64) {
        backend.opened_devices.insert(
            1,
            test_opened_device("/dev/dri/card-test-old", Some(test_pipeline(false)), None),
        );
        backend.claimed_output = Some(test_claimed_output(1, "/dev/dri/card-test-old"));
        backend.mark_claim_lost();

        if replacement_device_id == 1 {
            backend
                .opened_devices
                .get_mut(&1)
                .unwrap()
                .prepared_pipeline = Some(test_pipeline(true));
        } else {
            backend.opened_devices.insert(
                replacement_device_id,
                test_opened_device("/dev/dri/card-test-new", None, Some(test_pipeline(true))),
            );
        }
        backend.prepared_reclaim_output = Some(test_claimed_output(
            replacement_device_id,
            "/dev/dri/card-test-new",
        ));
    }

    #[test]
    fn host_backend_same_and_cross_device_reclaim_failure_restore_the_retired_claim() {
        for replacement_device_id in [1, 2] {
            let mut backend =
                HostBackendState::for_root_geometry_test(ScreenCaptureStore::default());
            seed_reclaim_lifecycle(&mut backend, replacement_device_id);
            backend.arm_prepared_reclaim_for_presentation().unwrap();
            assert_eq!(
                backend.claimed_output.as_ref().unwrap().device_id,
                replacement_device_id
            );

            backend.discard_unactivated_reclaim();

            assert_eq!(backend.claimed_output.as_ref().unwrap().device_id, 1);
            assert!(backend.retired_claim.is_none());
            assert!(
                backend
                    .opened_devices
                    .get(&1)
                    .unwrap()
                    .claimed_pipeline
                    .is_some()
            );
            if replacement_device_id != 1 {
                assert!(
                    backend
                        .opened_devices
                        .get(&replacement_device_id)
                        .unwrap()
                        .claimed_pipeline
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn host_backend_same_and_cross_device_reclaim_success_retires_old_resources() {
        for replacement_device_id in [1, 2] {
            let mut backend =
                HostBackendState::for_root_geometry_test(ScreenCaptureStore::default());
            seed_reclaim_lifecycle(&mut backend, replacement_device_id);
            backend.arm_prepared_reclaim_for_presentation().unwrap();

            backend.finish_reclaim_activation();

            assert_eq!(
                backend.claimed_output.as_ref().unwrap().device_id,
                replacement_device_id
            );
            assert!(backend.retired_claim.is_none());
            assert!(
                backend
                    .opened_devices
                    .get(&replacement_device_id)
                    .unwrap()
                    .claimed_pipeline
                    .is_some()
            );
            if replacement_device_id != 1 {
                assert!(
                    backend
                        .opened_devices
                        .get(&1)
                        .unwrap()
                        .claimed_pipeline
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn host_runtime_mode_reclaim_activates_at_the_same_exact_completion_boundary() {
        let (mut runtime, _event_loop, capture) = test_host_runtime_loop();
        seed_reclaim_lifecycle(&mut runtime.host_backend, 2);
        let active = lock_state(&runtime.shared_state)
            .root_geometry_snapshot()
            .unwrap();
        runtime
            .root_geometry_queue
            .push(RuntimeGeometryMutation::Mode {
                width: 1920,
                height: 1080,
            });

        runtime.stage_next_root_geometry_mutation();
        assert_eq!(
            runtime
                .host_backend
                .claimed_output
                .as_ref()
                .unwrap()
                .device_id,
            2
        );
        assert!(runtime.host_backend.retired_claim.is_some());
        assert_eq!(
            lock_state(&runtime.shared_state).root_geometry_snapshot(),
            Some(active)
        );
        stage_capture_for_pending_geometry(&runtime);
        runtime
            .root_geometry_flip_boundary
            .mark_queued(PresentationToken(25));
        assert!(!runtime.complete_root_geometry_presentations(&[PresentationToken(24)]));
        assert!(runtime.host_backend.retired_claim.is_some());
        assert!(runtime.complete_root_geometry_presentations(&[PresentationToken(25)]));

        let committed = lock_state(&runtime.shared_state)
            .root_geometry_snapshot()
            .unwrap();
        assert_eq!(committed.generation, active.generation + 1);
        assert_eq!(
            (
                committed.physical_size_px.width,
                committed.physical_size_px.height
            ),
            (1920, 1080)
        );
        assert_eq!(
            capture.root4_generations_for_test(),
            (Some(committed.generation), None)
        );
        assert!(runtime.host_backend.retired_claim.is_none());
    }

    #[test]
    fn production_geometry_queue_preserves_interleaved_fifo_and_repeated_modes() {
        let mut queue = RuntimeGeometryMutationQueue::default();
        queue.push(RuntimeGeometryMutation::Scale {
            factor: 1.5,
            source: crate::root_geometry::DisplayScaleSource::Config,
        });
        queue.push(RuntimeGeometryMutation::Mode {
            width: 3840,
            height: 2160,
        });
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        queue.push(RuntimeGeometryMutation::Rotation {
            rotation: OutputRotation::Deg90,
            response: tx,
        });
        queue.push(RuntimeGeometryMutation::Mode {
            width: 3840,
            height: 2160,
        });

        for expected in 1..=4 {
            assert_eq!(queue.pop().unwrap().sequence, expected);
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn production_geometry_activation_waits_for_a_queued_flip_completion() {
        let mut boundary = RootGeometryFlipBoundary::default();
        let geometry_frame = PresentationToken(7);
        assert!(!boundary.take_completed(&[geometry_frame]));
        boundary.mark_queued(geometry_frame);
        assert!(!boundary.take_completed(&[]));
        assert!(!boundary.take_completed(&[PresentationToken(6)]));
        assert!(boundary.take_completed(&[geometry_frame]));
        assert!(!boundary.take_completed(&[geometry_frame]));
        boundary.mark_queued(PresentationToken(8));
        boundary.discard();
        assert!(!boundary.take_completed(&[PresentationToken(8)]));
    }

    #[test]
    fn production_transaction_stage_survives_stale_completion_and_activates_exact_token() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        let active = state.root_geometry_snapshot().unwrap();
        let prepared = state
            .prepare_root4_display_scale_from_config(
                1.5,
                crate::root_geometry::DisplayScaleSource::Config,
            )
            .unwrap();
        let staged = stage_runtime_root_geometry_consumers(&state, prepared, None).unwrap();
        let shared_state = Arc::new(Mutex::new(state));
        let display: Display<RuntimeWaylandState> = Display::new().unwrap();
        let mut wayland =
            RuntimeWaylandState::new(display.handle(), Arc::clone(&shared_state)).unwrap();
        let mut store = DeterministicStageStore::default();
        let mut boundary = RootGeometryFlipBoundary::default();
        let matching = PresentationToken(42);

        install_root_geometry_stage(&mut wayland, &mut store, staged);
        boundary.mark_queued(matching);
        assert_eq!(store.events, [("begin", prepared.snapshot.generation)]);
        assert_eq!(
            lock_state(&shared_state).root_geometry_snapshot(),
            Some(active)
        );

        // A retired operation on the same physical CRTC cannot publish this stage.
        assert!(!boundary.take_completed(&[PresentationToken(41)]));
        assert!(wayland.staged_root_geometry.is_some());
        assert_eq!(
            lock_state(&shared_state).root_geometry_snapshot(),
            Some(active)
        );

        assert!(boundary.take_completed(&[matching]));
        let activated =
            activate_root_geometry_stage(&shared_state, &mut wayland, &mut store, |_| {}).unwrap();
        assert_eq!(activated.committed.snapshot, prepared.snapshot);
        assert_eq!(
            lock_state(&shared_state).root_geometry_snapshot(),
            Some(prepared.snapshot)
        );
        assert_eq!(
            store.events,
            [
                ("begin", prepared.snapshot.generation),
                ("commit", prepared.snapshot.generation),
            ]
        );
        assert!(wayland.staged_root_geometry.is_none());
        assert_eq!(
            wayland.applied_root_geometry_generation,
            prepared.snapshot.generation
        );
    }

    #[test]
    fn staged_rotation_topology_matches_the_committed_fractional_native_geometry() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        state
            .apply_native_pane_host_plan(vec![NativePaneHostRequest {
                id: PaneId::new("native"),
                content_id: Some("content".to_string()),
                binding_id: Some("binding".to_string()),
                launch_token: None,
                revision: 1,
                geometry: PaneGeometry {
                    x: 10.25,
                    y: 20.5,
                    width: 100.25,
                    height: 50.75,
                    coordinate_space: PaneGeometryCoordinateSpace::CompositorLogical,
                },
                target: NativeTargetClass::Terminal,
                process: ProcessSpec {
                    command: "foot".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                },
            }])
            .unwrap();
        let prepared = state.prepare_root4_rotation(OutputRotation::Deg90).unwrap();
        let staged = stage_runtime_root_geometry_consumers(&state, prepared, None).unwrap();
        state.commit_root4_geometry_consumers(
            staged.committed,
            staged.root_layout.clone(),
            &staged.viewports,
        );

        assert_eq!(staged.topology.panes, state.status_snapshot().panes);
    }

    #[test]
    fn staged_generation_is_private_until_the_presentation_operation() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        let active = state.root_geometry_snapshot().unwrap();
        let prepared = state
            .prepare_root4_display_scale_from_config(
                1.5,
                crate::root_geometry::DisplayScaleSource::Config,
            )
            .unwrap();
        let staged = stage_runtime_root_geometry_consumers(&state, prepared, None).unwrap();
        let shared_state = Arc::new(Mutex::new(state));
        let display: Display<RuntimeWaylandState> = Display::new().unwrap();
        let mut runtime = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        runtime.staged_root_geometry = Some(staged.clone());
        assert_eq!(runtime.root_geometry_snapshot(), Some(active));

        runtime.presentation_root_geometry = Some(staged);
        assert_eq!(runtime.root_geometry_snapshot(), Some(prepared.snapshot));
        runtime.presentation_root_geometry = None;
        assert_eq!(runtime.root_geometry_snapshot(), Some(active));
    }

    #[test]
    fn production_staged_operation_rejects_surface_tree_material_interleaving() {
        let mut state = CompositorState::new(true, Box::new(NoopProcessController));
        state.mark_runtime_resize(3840, 2160);
        let committed = state
            .prepare_root4_display_scale_from_config(
                1.5,
                crate::root_geometry::DisplayScaleSource::Config,
            )
            .unwrap();
        let mut staged = stage_runtime_root_geometry_consumers(&state, committed, None).unwrap();
        let surface_id = smithay::reexports::wayland_server::backend::ObjectId::null();
        let accepted = AcceptedMaterialIdentity {
            surfaces: vec![AcceptedSurfaceMaterial {
                id: surface_id.clone(),
                commit_serial: 7,
                tree_offset: (12, 8).into(),
            }],
            popups: vec![AcceptedPopupMaterial {
                id: surface_id,
                owner_role: RuntimeSurfaceRole::NativePane(PaneId::new("native")),
                geometry: Rectangle::new((3, 4).into(), (20, 10).into()),
                surfaces: vec![AcceptedSurfaceMaterial {
                    id: smithay::reexports::wayland_server::backend::ObjectId::null(),
                    commit_serial: 11,
                    tree_offset: (1, 2).into(),
                }],
            }],
        };

        assert!(staged.accept_or_validate_material_identity(accepted.clone()));
        assert!(staged.accept_or_validate_material_identity(accepted.clone()));

        let mut buffer_commit = accepted.clone();
        buffer_commit.surfaces[0].commit_serial += 1;
        assert!(!staged.accept_or_validate_material_identity(buffer_commit));

        let mut subsurface_move = accepted.clone();
        subsurface_move.surfaces[0].tree_offset = (13, 8).into();
        assert!(!staged.accept_or_validate_material_identity(subsurface_move));

        let mut popup_buffer_commit = accepted.clone();
        popup_buffer_commit.popups[0].surfaces[0].commit_serial += 1;
        assert!(!staged.accept_or_validate_material_identity(popup_buffer_commit));

        let mut popup_subsurface_move = accepted.clone();
        popup_subsurface_move.popups[0].surfaces[0].tree_offset = (2, 2).into();
        assert!(!staged.accept_or_validate_material_identity(popup_subsurface_move));

        let mut popup_move = accepted;
        popup_move.popups[0].geometry.loc = (4, 4).into();
        assert!(!staged.accept_or_validate_material_identity(popup_move));
    }

    #[test]
    fn embedded_toplevels_request_server_side_decoration_mode() {
        assert_eq!(
            embedded_toplevel_decoration_mode(),
            XdgDecorationMode::ServerSide
        );
    }

    #[test]
    fn shell_overlay_shortcut_parser_normalizes_default_super_grave() {
        let shortcut = parse_shell_overlay_toggle_shortcut("super+grave")
            .expect("default shortcut alias should parse");
        assert_eq!(shortcut.display_string(), "Super+`");
    }

    #[test]
    fn shell_overlay_shortcut_parser_rejects_non_super_modifier() {
        let err = parse_shell_overlay_toggle_shortcut("ctrl+grave")
            .expect_err("non-super shortcut should be rejected");
        assert!(err.contains("modifier must be Super"));
    }

    #[test]
    fn shell_overlay_shortcut_match_requires_super_and_raw_bound_key() {
        let shortcut = ShellOverlayToggleShortcut {
            normalized: "Super+`".to_string(),
            keysym: Keysym::new(keysyms::KEY_grave),
        };
        let modifiers = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        assert!(shortcut.matches(&modifiers, &[Keysym::new(keysyms::KEY_grave)]));
        assert!(!shortcut.matches(
            &ModifiersState::default(),
            &[Keysym::new(keysyms::KEY_grave)]
        ));
        assert!(!shortcut.matches(&modifiers, &[Keysym::new(keysyms::KEY_F12)]));
    }

    #[test]
    fn process_lineage_match_accepts_exact_pid() {
        let pid = std::process::id();
        assert!(pid_matches_or_descends_from(pid, pid));
    }

    #[test]
    fn allocated_gbm_bo_export_policy_preserves_explicit_modifiers() {
        assert!(
            !GBM_BUFFER_FROM_BO_PRESERVE_EXPLICIT_MODIFIER,
            "modifier-aware GBM export must not force implicit modifier mode"
        );
    }

    #[test]
    fn gles_intermediate_render_targets_stay_on_stable_xrgb8888() {
        assert_eq!(GLES_INTERMEDIATE_RENDER_FORMAT, DrmFourcc::Xrgb8888);
    }

    fn test_overlay_region(
        region_id: &str,
        captures: Vec<OverlayCaptureCapability>,
    ) -> OverlayRegionStatus {
        OverlayRegionStatus {
            region_id: region_id.to_string(),
            pane_id: PaneId::new("pane-a"),
            pane_instance_id: "instance-a".to_string(),
            kind: CompositorOverlayKind::PaneBadge,
            rect: OverlayRect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            },
            z_index: Some(1),
            captures,
            clamped: false,
        }
    }

    #[test]
    fn overlay_region_capture_hit_test_is_data_only_per_input_class() {
        let regions = vec![test_overlay_region(
            "chrome",
            vec![
                OverlayCaptureCapability::PointerHover,
                OverlayCaptureCapability::PointerButton,
            ],
        )];
        let inside = (20.0, 30.0).into();
        let outside = (200.0, 30.0).into();

        assert!(super::overlay_region_capture_contains(
            &regions,
            inside,
            OverlayCaptureCapability::PointerHover
        ));
        assert!(super::overlay_region_capture_contains(
            &regions,
            inside,
            OverlayCaptureCapability::PointerButton
        ));
        assert!(!super::overlay_region_capture_contains(
            &regions,
            inside,
            OverlayCaptureCapability::PointerAxis
        ));
        assert!(!super::overlay_region_capture_contains(
            &regions,
            outside,
            OverlayCaptureCapability::PointerHover
        ));
    }

    #[test]
    fn native_pane_focus_target_preserves_pane_identity() {
        assert_eq!(
            super::runtime_focus_target_for_role(super::RuntimeSurfaceRole::NativePane(
                PaneId::new("pane-a")
            )),
            RuntimeFocusTarget::NativePane {
                pane_id: PaneId::new("pane-a")
            }
        );
    }

    #[test]
    fn overlay_region_debug_borders_are_two_pixel_clipped_rectangles() {
        let regions = vec![test_overlay_region(
            "chrome",
            vec![OverlayCaptureCapability::PointerHover],
        )];

        let rects = super::overlay_region_debug_border_rects(&regions, 160, 100);

        assert_eq!(rects.len(), 4);
        assert!(rects.contains(&Rectangle::new((10, 20).into(), (100, 2).into())));
        assert!(rects.contains(&Rectangle::new((10, 68).into(), (100, 2).into())));
        assert!(rects.contains(&Rectangle::new((10, 20).into(), (2, 50).into())));
        assert!(rects.contains(&Rectangle::new((108, 20).into(), (2, 50).into())));
    }

    #[test]
    fn overlay_region_debug_borders_use_rotated_logical_surface_bounds() {
        let mut region = test_overlay_region(
            "bottom-chrome",
            vec![OverlayCaptureCapability::PointerHover],
        );
        region.rect.x = 466.0;
        region.rect.y = 3743.0;
        region.rect.width = 148.0;
        region.rect.height = 48.0;

        assert!(
            super::overlay_region_debug_border_rects(&[region.clone()], 3840, 2160).is_empty(),
            "physical scanout bounds incorrectly clip bottom chrome on deg90"
        );
        let rects = super::overlay_region_debug_border_rects(&[region], 2160, 3840);

        assert!(rects.contains(&Rectangle::new((466, 3743).into(), (148, 2).into())));
        assert!(rects.contains(&Rectangle::new((466, 3789).into(), (148, 2).into())));
        assert!(rects.contains(&Rectangle::new((466, 3743).into(), (2, 48).into())));
        assert!(rects.contains(&Rectangle::new((612, 3743).into(), (2, 48).into())));
    }

    #[test]
    fn software_cursor_uses_non_magenta_visible_rectangles_at_pointer_location() {
        let rects = super::software_cursor_rects((10.0, 10.0).into(), 160, 100);

        assert_eq!(rects[0].1, super::SoftwareCursorColor::White);
        assert!(rects.iter().any(|(rect, color)| {
            *color == super::SoftwareCursorColor::White
                && *rect == Rectangle::new((10, 10).into(), (1, 1).into())
        }));
        assert!(rects.iter().any(|(rect, color)| {
            *color == super::SoftwareCursorColor::Black
                && *rect == Rectangle::new((9, 9).into(), (3, 1).into())
        }));
        assert!(
            rects
                .iter()
                .any(|(_, color)| *color == super::SoftwareCursorColor::White)
        );
        assert!(
            rects
                .iter()
                .any(|(_, color)| *color == super::SoftwareCursorColor::Black)
        );
    }

    #[test]
    fn software_cursor_mask_is_48_by_48_msb_first_rows() {
        assert_eq!(super::SOFTWARE_CURSOR_MASK_WIDTH, 48);
        assert_eq!(super::SOFTWARE_CURSOR_MASK_HEIGHT, 48);
        assert_eq!(super::SOFTWARE_CURSOR_MASK_STRIDE, 6);
        assert_eq!(super::SOFTWARE_CURSOR_HOTSPOT_X, 6);
        assert_eq!(super::SOFTWARE_CURSOR_HOTSPOT_Y, 4);
        assert_eq!(
            super::SOFTWARE_CURSOR_MASK.len(),
            (super::SOFTWARE_CURSOR_MASK_HEIGHT as usize) * super::SOFTWARE_CURSOR_MASK_STRIDE
        );

        assert!(super::software_cursor_mask_pixel(6, 4));
        assert!(!super::software_cursor_mask_pixel(5, 4));
        assert!(!super::software_cursor_mask_pixel(7, 4));
        assert!(super::software_cursor_mask_pixel(6, 24));
        assert!(super::software_cursor_mask_pixel(26, 24));
        assert!(!super::software_cursor_mask_pixel(27, 24));
        assert!(super::software_cursor_mask_pixel(11, 27));
        assert!(!super::software_cursor_mask_pixel(12, 27));
        assert!(super::software_cursor_mask_pixel(13, 27));
        assert!(super::software_cursor_mask_pixel(20, 42));
        assert!(!super::software_cursor_mask_pixel(21, 42));
        assert!(!super::software_cursor_mask_pixel(18, 43));
    }

    #[test]
    fn software_cursor_hotspot_places_arrow_tip_at_pointer_location() {
        let pointer = Point::<f64, Logical>::from((100.0, 80.0));
        assert_eq!(
            super::software_cursor_mask_origin(pointer),
            Point::<i32, Logical>::from((94, 76))
        );

        let rects = super::software_cursor_rects(pointer, 200, 160);

        assert!(rects.iter().any(|(rect, color)| {
            *color == super::SoftwareCursorColor::White
                && *rect == Rectangle::new((100, 80).into(), (1, 1).into())
        }));
    }

    #[test]
    fn software_cursor_rects_clip_to_output_bounds() {
        let rects = super::software_cursor_rects((-4.0, -4.0).into(), 12, 12);

        assert!(!rects.is_empty());
        assert!(rects.iter().all(|(rect, _)| {
            rect.loc.x >= 0
                && rect.loc.y >= 0
                && rect.loc.x + rect.size.w <= 12
                && rect.loc.y + rect.size.h <= 12
        }));
    }

    #[test]
    fn software_cursor_defaults_to_output_center_before_pointer_motion() {
        assert_eq!(
            super::software_cursor_default_location(Size::<i32, Logical>::from((3840, 2160))),
            Point::<f64, Logical>::from((1920.0, 1080.0))
        );
    }

    #[test]
    fn pointer_location_clamps_to_output_bounds() {
        assert_eq!(
            super::clamp_pointer_location(
                (-50.0, 2200.0).into(),
                Size::<i32, Logical>::from((3840, 2160))
            ),
            Point::<f64, Logical>::from((0.0, 2159.0))
        );
    }

    #[test]
    fn cursor_rects_map_to_output_scanout_after_rotation() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let scanout_size = Size::<i32, Physical>::from((3840, 2160));

        assert_eq!(
            super::cursor_rect_to_scanout(
                rect,
                Size::<i32, Physical>::from((3840, 2160)),
                2.0,
                OutputRotation::Deg0
            ),
            Some(Rectangle::<i32, Physical>::new(
                (20, 40).into(),
                (60, 80).into()
            ))
        );
        assert_eq!(
            super::cursor_rect_to_scanout(rect, scanout_size, 2.0, OutputRotation::Deg90),
            Some(Rectangle::<i32, Physical>::new(
                (3720, 20).into(),
                (80, 60).into()
            ))
        );
        assert_eq!(
            super::cursor_rect_to_scanout(rect, scanout_size, 2.0, OutputRotation::Deg270),
            Some(Rectangle::<i32, Physical>::new(
                (40, 2080).into(),
                (80, 60).into()
            ))
        );
    }

    #[test]
    fn deg90_cursor_mapping_is_inverse_of_capture_rotation() {
        let scanout_size = Size::<i32, Physical>::from((3840, 2160));
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 40).into());
        let mapped = super::cursor_rect_to_scanout(rect, scanout_size, 2.0, OutputRotation::Deg90)
            .expect("deg90 cursor rect should map into scanout bounds");

        assert_eq!(mapped.loc, (3720, 20).into());
        assert_eq!(mapped.size, (80, 60).into());
    }

    #[test]
    fn primary_plane_elements_keep_debug_borders_when_overlay_plane_is_split() {
        let mut capture = super::RenderElementCapture::default();
        capture.push_elements(
            super::RenderElementSource::Overlay,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "overlay",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        capture.push_elements(
            super::RenderElementSource::OverlayRegionDebug,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "chrome",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        let debug_count = capture.counts.overlay_region_debug;

        let primary = capture.primary_plane_elements();

        assert_eq!(primary.len(), debug_count);
        assert!(debug_count > 0);
    }

    #[test]
    fn primary_plane_elements_keep_lifted_main_overlay_regions() {
        let mut capture = super::RenderElementCapture::default();
        capture.push_elements(
            super::RenderElementSource::Overlay,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "dedicated-overlay",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        capture.push_elements(
            super::RenderElementSource::MainOverlayRegion,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "main-overlay",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        let lifted_count = capture.counts.main_overlay_regions;

        let primary = capture.primary_plane_elements();

        assert_eq!(primary.len(), lifted_count);
        assert!(lifted_count > 0);
    }

    #[test]
    fn primary_plane_elements_filter_dedicated_overlay_by_source_not_position() {
        let mut capture = super::RenderElementCapture::default();
        capture.push_elements(
            super::RenderElementSource::NativePane,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "native",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        let native_count = capture.counts.native_panes;
        capture.push_elements(
            super::RenderElementSource::Overlay,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "overlay",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );

        let primary = capture.primary_plane_elements();

        assert_eq!(primary.len(), native_count);
        assert!(native_count > 0);
    }

    #[test]
    fn overlay_region_chrome_orders_before_native_pane_content_for_smithay_z_order() {
        let mut capture = super::RenderElementCapture::default();
        capture.push_elements(
            super::RenderElementSource::OverlayRegionDebug,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "debug",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        capture.push_elements(
            super::RenderElementSource::MainOverlayRegion,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "chrome",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );
        capture.push_elements(
            super::RenderElementSource::NativePane,
            super::overlay_region_debug_border_elements(
                &[test_overlay_region(
                    "native",
                    vec![OverlayCaptureCapability::PointerHover],
                )],
                160,
                100,
            ),
        );

        let first_native = capture
            .sources
            .iter()
            .position(|source| *source == super::RenderElementSource::NativePane)
            .expect("native pane source should be recorded");
        let first_debug = capture
            .sources
            .iter()
            .position(|source| *source == super::RenderElementSource::OverlayRegionDebug)
            .expect("debug source should be recorded");
        let first_lifted = capture
            .sources
            .iter()
            .position(|source| *source == super::RenderElementSource::MainOverlayRegion)
            .expect("lifted source should be recorded");

        assert!(first_debug < first_native);
        assert!(first_lifted < first_native);
    }

    #[test]
    fn overlay_region_logical_rect_rounds_outward_for_visual_lift_mask() {
        let mut region =
            test_overlay_region("main-overlay", vec![OverlayCaptureCapability::PointerHover]);
        region.rect.x = 10.8;
        region.rect.y = 20.2;
        region.rect.width = 30.1;
        region.rect.height = 40.9;

        let rect = super::overlay_region_logical_rect(&region);

        assert_eq!(rect.loc, (10, 20).into());
        assert_eq!(rect.size, (31, 41).into());
    }

    #[test]
    fn overlay_region_crop_rect_tracks_mapped_target_space() {
        let mut region =
            test_overlay_region("main-overlay", vec![OverlayCaptureCapability::PointerHover]);
        region.rect.x = 944.4;
        region.rect.y = 1868.2;
        region.rect.width = 85.0;
        region.rect.height = 77.0;

        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((40, 20).into(), (80, 40).into()),
            Rectangle::<i32, Logical>::new((944, 16).into(), (960, 540).into()),
        );
        let mapped = mapping.map_rect(super::overlay_region_logical_rect(&region));
        let crop = Rectangle::<i32, Physical>::new(
            (mapped.loc.x, mapped.loc.y).into(),
            (mapped.size.w, mapped.size.h).into(),
        );

        assert_eq!(crop.loc, (11792, 24964).into());
        assert_eq!(crop.size, (1020, 1040).into());
    }

    #[test]
    fn select_primary_path_prefers_configured_path_when_present() {
        let paths = [
            PathBuf::from("/dev/dri/card1"),
            PathBuf::from("/dev/dri/card0"),
        ];
        let selected = select_primary_path(paths.iter(), Some(Path::new("/dev/dri/card1")));
        assert_eq!(selected.as_deref(), Some("/dev/dri/card1"));
    }

    #[test]
    fn select_primary_path_falls_back_to_lexicographic_order() {
        let paths = [
            PathBuf::from("/dev/dri/card9"),
            PathBuf::from("/dev/dri/card2"),
        ];
        let selected = select_primary_path(paths.iter(), Some(Path::new("/dev/dri/card0")));
        assert_eq!(selected.as_deref(), Some("/dev/dri/card2"));
    }

    #[test]
    fn overlay_plane_layout_maps_overlay_rect_to_atomic_coordinates() {
        let rect = Rectangle::<i32, Logical>::new((-20, 24).into(), (640, 360).into());
        let layout =
            AtomicPlaneLayout::from_overlay_rect(rect).expect("positive-size overlay maps");

        assert_eq!(layout.crtc_x, 0);
        assert_eq!(layout.crtc_y, 24);
        assert_eq!(layout.crtc_w, 640);
        assert_eq!(layout.crtc_h, 360);
        assert_eq!(layout.src_x, 0);
        assert_eq!(layout.src_y, 0);
        assert_eq!(layout.src_w, 640);
        assert_eq!(layout.src_h, 360);
    }

    #[test]
    fn overlay_plane_layout_rejects_zero_sized_overlay_rect() {
        let rect = Rectangle::<i32, Logical>::new((16, 16).into(), (0, 320).into());
        assert!(AtomicPlaneLayout::from_overlay_rect(rect).is_none());
    }

    #[test]
    fn runtime_overlay_policy_rect_maps_directly_to_atomic_overlay_layout() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(1920, 1080);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        let overlay_rect = wayland_state.overlay_rect();
        assert_eq!(overlay_rect.loc.x, 464);
        assert_eq!(overlay_rect.loc.y, 16);
        assert_eq!(overlay_rect.size.w, 480);
        assert_eq!(overlay_rect.size.h, 320);

        let layout = AtomicPlaneLayout::from_overlay_rect(overlay_rect)
            .expect("positive overlay rect should map to atomic plane layout");
        assert_eq!(layout.crtc_x, 464);
        assert_eq!(layout.crtc_y, 16);
        assert_eq!(layout.crtc_w, 480);
        assert_eq!(layout.crtc_h, 320);
        assert_eq!(layout.src_w, 480);
        assert_eq!(layout.src_h, 320);
    }

    #[test]
    fn runtime_overlay_policy_clamps_left_edge_on_small_outputs() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(320, 200);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        let overlay_rect = wayland_state.overlay_rect();
        assert_eq!(overlay_rect.loc.x, 0);
        assert_eq!(overlay_rect.loc.y, 16);
        assert_eq!(overlay_rect.size.w, 160);
        assert_eq!(overlay_rect.size.h, 84);

        let layout = AtomicPlaneLayout::from_overlay_rect(overlay_rect)
            .expect("overlay rect should still map to atomic plane layout");
        assert_eq!(layout.crtc_x, 0);
        assert_eq!(layout.crtc_y, 16);
        assert_eq!(layout.crtc_w, 160);
        assert_eq!(layout.crtc_h, 84);
    }

    #[test]
    fn runtime_overlay_policy_tiny_output_stays_non_empty_and_in_bounds() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(1, 1);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        let overlay_rect = wayland_state.overlay_rect();
        assert_eq!(overlay_rect.loc.x, 0);
        assert_eq!(overlay_rect.loc.y, 0);
        assert_eq!(overlay_rect.size.w, 1);
        assert_eq!(overlay_rect.size.h, 1);
        assert!(
            overlay_rect.loc.x + overlay_rect.size.w <= 1
                && overlay_rect.loc.y + overlay_rect.size.h <= 1
        );

        let layout = AtomicPlaneLayout::from_overlay_rect(overlay_rect)
            .expect("tiny overlay rect should still map to a valid atomic layout");
        assert_eq!(layout.crtc_x, 0);
        assert_eq!(layout.crtc_y, 0);
        assert_eq!(layout.crtc_w, 1);
        assert_eq!(layout.crtc_h, 1);
    }

    #[test]
    fn runtime_output_size_swaps_for_quarter_turn_rotation() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg90);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        assert_eq!(wayland_state.runtime_output_width(), 1080);
        assert_eq!(wayland_state.runtime_output_height(), 1920);
    }

    #[test]
    fn runtime_pointer_input_maps_physical_deg90_events_to_logical_surface() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg90);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        assert_eq!(
            wayland_state.runtime_physical_output_size(),
            Size::<i32, Physical>::from((3840, 2160))
        );
        assert_eq!(
            wayland_state.pointer_location,
            Point::<f64, Logical>::from((540.0, 960.0))
        );
        assert_eq!(
            wayland_state.map_physical_pointer_point_to_logical((3839.0, 2159.0).into()),
            Point::<f64, Logical>::from((1079.5, 0.5))
        );
    }

    #[test]
    fn runtime_output_size_keeps_upright_dimensions_without_quarter_turn_rotation() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg180);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        assert_eq!(wayland_state.runtime_output_width(), 1920);
        assert_eq!(wayland_state.runtime_output_height(), 1080);
    }

    #[test]
    fn quarter_turn_rotation_maps_to_counterclockwise_transform() {
        assert_eq!(
            transform_from_rotation(OutputRotation::Deg90),
            Transform::_270
        );
        assert_eq!(
            transform_from_rotation(OutputRotation::Deg270),
            Transform::_90
        );
    }

    #[test]
    fn direct_present_support_allows_compositor_rendered_quarter_turn_rotations() {
        assert!(direct_present_supported_for_rotation(OutputRotation::Deg0));
        assert!(direct_present_supported_for_rotation(OutputRotation::Deg90));
        assert!(direct_present_supported_for_rotation(
            OutputRotation::Deg180
        ));
        assert!(direct_present_supported_for_rotation(
            OutputRotation::Deg270
        ));
    }

    #[test]
    fn runtime_output_global_preserves_raw_physical_mode_across_root_rotation() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg90);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();

        assert_eq!(
            wayland_state.output.current_mode().map(|mode| mode.size),
            Some((3840, 2160).into())
        );
        assert_eq!(wayland_state.output.current_transform(), Transform::Normal);
    }

    #[test]
    fn sync_output_state_picks_up_rotation_changes_before_client_bind() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg0);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state =
            RuntimeWaylandState::new(display.handle(), shared_state.clone()).unwrap();
        assert_eq!(
            wayland_state.output.current_mode().map(|mode| mode.size),
            Some((3840, 2160).into())
        );

        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.set_output_rotation(OutputRotation::Deg90);
        }
        wayland_state.sync_output_state();

        assert_eq!(
            wayland_state.output.current_mode().map(|mode| mode.size),
            Some((3840, 2160).into())
        );
        assert_eq!(wayland_state.output.current_transform(), Transform::Normal);
    }

    #[test]
    fn sync_output_rotation_reconfigure_if_needed_preserves_physical_mode_size() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg90);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let mut wayland_state =
            RuntimeWaylandState::new(display.handle(), shared_state.clone()).unwrap();
        assert_eq!(
            wayland_state.output.current_mode().map(|mode| mode.size),
            Some((3840, 2160).into())
        );

        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.set_output_rotation(OutputRotation::Deg0);
        }
        wayland_state.sync_output_rotation_reconfigure_if_needed();

        assert_eq!(
            wayland_state.output.current_mode().map(|mode| mode.size),
            Some((3840, 2160).into())
        );
        assert_eq!(
            wayland_state.backend_output_size,
            Size::<i32, Physical>::from((3840, 2160))
        );
        assert_eq!(wayland_state.runtime_output_width(), 1920);
        assert_eq!(wayland_state.runtime_output_height(), 1080);
    }

    #[test]
    fn quarter_turn_render_output_size_uses_portrait_logical_dimensions_before_transform() {
        let shared_state = Arc::new(Mutex::new(CompositorState::new(
            true,
            Box::new(NoopProcessController),
        )));
        {
            let mut state = match shared_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.mark_runtime_resize(3840, 2160);
            state.set_output_rotation(OutputRotation::Deg90);
        }

        let display: Display<RuntimeWaylandState> =
            Display::new().expect("test wayland display should initialize");
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state).unwrap();
        let render_size = render_output_size_before_transform(&wayland_state);

        assert_eq!(render_size, Size::<i32, Physical>::from((2160, 3840)));
        assert_eq!(
            transform_from_rotation(OutputRotation::Deg90).transform_size(render_size),
            Size::<i32, Physical>::from((3840, 2160))
        );
    }

    #[test]
    fn scene_texture_transform_uses_physical_scanout_rotation_for_quarter_turn_composite() {
        assert_eq!(
            scene_texture_transform(OutputRotation::Deg90),
            Transform::_270
        );
        assert_eq!(
            scene_texture_transform(OutputRotation::Deg270),
            Transform::_90
        );
    }

    #[test]
    fn screen_capture_flip_policy_matches_verified_rotation_contract() {
        assert!(!screen_capture_src_flipped(true, OutputRotation::Deg0));
        assert!(!screen_capture_src_flipped(true, OutputRotation::Deg90));
        assert!(!screen_capture_src_flipped(true, OutputRotation::Deg180));
        assert!(!screen_capture_src_flipped(true, OutputRotation::Deg270));
        assert!(!screen_capture_src_flipped(false, OutputRotation::Deg0));
        assert!(!screen_capture_src_flipped(false, OutputRotation::Deg180));
        assert!(!screen_capture_src_flipped(false, OutputRotation::Deg90));
        assert!(!screen_capture_src_flipped(false, OutputRotation::Deg270));
    }

    #[test]
    fn quarter_turn_dumb_present_unflips_readback_rows_before_scanout() {
        let src = vec![
            0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        ];
        let mut dst = vec![0u8; 16];

        copy_renderer_pixels_to_dumb(&src, true, OutputRotation::Deg90, &mut dst, 8, 2, 2);

        assert_eq!(
            dst,
            vec![
                0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, //
                0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn role_surface_mapping_scales_source_bbox_to_target_rect() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((40, 20).into(), (80, 40).into()),
            Rectangle::<i32, Logical>::new((0, 0).into(), (3840, 2160).into()),
        );

        assert_eq!(
            mapping.map_rect(Rectangle::new((40, 20).into(), (80, 40).into())),
            Rectangle::<i32, Logical>::new((0, 0).into(), (3840, 2160).into())
        );
    }

    #[test]
    fn role_surface_mapping_exposes_global_surface_origin_for_pointer_focus() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (640, 480).into()),
            Rectangle::<i32, Logical>::new((0, 1920).into(), (2160, 1920).into()),
        );

        assert_eq!(mapping.origin, Point::<f64, Logical>::from((0.0, 1920.0)));
    }

    #[test]
    fn role_surface_mapping_preserves_fractional_native_pane_interval() {
        let mapping = RoleSurfaceMapping::new_native_materialization(
            Rectangle::<i32, Logical>::new((0, 0).into(), (131, 67).into()),
            crate::root_geometry::NativeBufferProjection {
                origin_x: 13,
                origin_y: 26,
                width_px: 131,
                height_px: 67,
                fractional_phase_x: 0.325,
                fractional_phase_y: 0.65,
                logical_clip: crate::root_geometry::LogicalRect {
                    x: 10.25,
                    y: 20.5,
                    width: 100.25,
                    height: 50.75,
                },
                scale_factor: 1.3,
            },
        );

        assert!((mapping.origin.x - 10.0).abs() < 1e-12);
        assert!((mapping.origin.y - 20.0).abs() < 1e-12);
        assert!((mapping.scale.x - 1.0 / 1.3).abs() < 1e-12);
        assert!((mapping.scale.y - 1.0 / 1.3).abs() < 1e-12);
    }

    #[test]
    fn production_software_sampler_uses_fractional_source_interval_for_all_rotations() {
        let projection = crate::root_geometry::NativeBufferProjection {
            origin_x: 13,
            origin_y: 26,
            width_px: 131,
            height_px: 67,
            fractional_phase_x: 0.325,
            fractional_phase_y: 0.65,
            logical_clip: crate::root_geometry::LogicalRect {
                x: 10.25,
                y: 20.5,
                width: 100.25,
                height: 50.75,
            },
            scale_factor: 1.3,
        };
        for (rotation, width, height) in [
            (OutputRotation::Deg0, 131, 67),
            (OutputRotation::Deg90, 67, 131),
            (OutputRotation::Deg180, 131, 67),
            (OutputRotation::Deg270, 67, 131),
        ] {
            for (rel_x, rel_y) in [(0, 0), (width - 1, height - 1)] {
                let (x, y) = native_source_sample(
                    Some(projection),
                    rotation,
                    rel_x,
                    rel_y,
                    width,
                    height,
                    131,
                    67,
                );
                assert!(x > projection.fractional_phase_x);
                assert!(x < projection.fractional_phase_x + 100.25 * 1.3);
                assert!(y > projection.fractional_phase_y);
                assert!(y < projection.fractional_phase_y + 50.75 * 1.3);
            }
        }
    }

    #[test]
    fn production_gles_native_materialization_keeps_one_source_texel_per_physical_pixel() {
        let projection = crate::root_geometry::NativeBufferProjection {
            origin_x: 13,
            origin_y: 26,
            width_px: 131,
            height_px: 67,
            fractional_phase_x: 0.325,
            fractional_phase_y: 0.65,
            logical_clip: crate::root_geometry::LogicalRect {
                x: 10.25,
                y: 20.5,
                width: 100.25,
                height: 50.75,
            },
            scale_factor: 1.3,
        };

        assert_eq!(
            native_materialized_source_rect(projection),
            Rectangle::<f64, BufferCoords>::new((0.0, 0.0).into(), (131.0, 67.0).into())
        );
        assert_eq!(
            native_materialized_destination_rect(projection),
            Rectangle::<i32, Physical>::new((13, 26).into(), (131, 67).into())
        );
    }

    #[test]
    fn native_materialization_remaps_real_damage_and_clips_fractional_coverage() {
        let projection = crate::root_geometry::NativeBufferProjection {
            origin_x: 13,
            origin_y: 26,
            width_px: 131,
            height_px: 67,
            fractional_phase_x: 0.325,
            fractional_phase_y: 0.65,
            logical_clip: crate::root_geometry::LogicalRect {
                x: 10.25,
                y: 20.5,
                width: 100.25,
                height: 50.75,
            },
            scale_factor: 1.3,
        };
        let remapped = remap_damage_to_materialized_destination(
            DamageSet::from_slice(&[Rectangle::new((10, 5).into(), (20, 10).into())]),
            (100, 50).into(),
            (131, 67).into(),
        );
        assert_eq!(
            remapped.iter().copied().collect::<Vec<_>>(),
            vec![Rectangle::new((13, 6).into(), (27, 15).into())]
        );

        assert_eq!(
            native_materialized_local_clip(
                projection,
                native_materialized_destination_rect(projection)
            ),
            Rectangle::new((0, 1).into(), (131, 66).into())
        );
        let child_destination = Rectangle::new((0, 0).into(), (3840, 2160).into());
        assert_eq!(
            native_materialized_local_clip(projection, child_destination),
            Rectangle::new((13, 27).into(), (131, 66).into())
        );
    }

    fn smithay_client_point(
        compositor_point: Point<f64, Logical>,
        mapping: RoleSurfaceMapping,
    ) -> Point<f64, Logical> {
        compositor_point - mapping.focus_origin()
    }

    fn physical_point_for_logical_deg90(logical_x: f64, logical_y: f64) -> Point<f64, Physical> {
        Point::<f64, Physical>::from((logical_y, 2159.0 - logical_x))
    }

    #[test]
    fn rotated_web_mouse_points_preserve_content_coordinates() {
        let web_mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (2160, 3840).into()),
            Rectangle::<i32, Logical>::new((0, 0).into(), (2160, 3840).into()),
        );
        let rotation = OutputRotationModel::new(OutputRotation::Deg90);
        let web_mouse_points = [
            (540.0, 1920.0),
            (543.0, 1918.0),
            (537.0, 1924.0),
            (541.0, 1919.0),
            (540.0, 1920.0),
        ];

        for (expected_x, expected_y) in web_mouse_points {
            let physical = physical_point_for_logical_deg90(expected_x, expected_y);
            let compositor_point =
                rotation.physical_point_to_logical(physical.x, physical.y, 3840, 2160);

            assert_eq!(
                smithay_client_point(compositor_point.into(), web_mapping),
                Point::<f64, Logical>::from((expected_x, expected_y))
            );
        }
    }

    #[test]
    fn rotated_web_native_mouse_surface_switch_preserves_local_coordinates() {
        let web_mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (1080, 3840).into()),
            Rectangle::<i32, Logical>::new((0, 0).into(), (1080, 3840).into()),
        );
        let native_mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (1080, 3840).into()),
            Rectangle::<i32, Logical>::new((1080, 0).into(), (1080, 3840).into()),
        );
        let rotation = OutputRotationModel::new(OutputRotation::Deg90);
        let cases = [
            (web_mapping, (540.0, 1920.0), (540.0, 1920.0)),
            (web_mapping, (542.0, 1921.0), (542.0, 1921.0)),
            (native_mapping, (1620.0, 1920.0), (540.0, 1920.0)),
            (native_mapping, (1617.0, 1923.0), (537.0, 1923.0)),
            (web_mapping, (539.0, 1918.0), (539.0, 1918.0)),
        ];

        for (mapping, (logical_x, logical_y), (expected_x, expected_y)) in cases {
            let physical = physical_point_for_logical_deg90(logical_x, logical_y);
            let compositor_point =
                rotation.physical_point_to_logical(physical.x, physical.y, 3840, 2160);

            assert_eq!(
                smithay_client_point(compositor_point.into(), mapping),
                Point::<f64, Logical>::from((expected_x, expected_y))
            );
        }
    }

    #[test]
    fn role_surface_mapping_focus_origin_yields_smithay_client_coordinates() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (2160, 1920).into()),
            Rectangle::<i32, Logical>::new((0, 1920).into(), (2160, 1920).into()),
        );
        let compositor_point = Point::<f64, Logical>::from((540.0, 2880.0));

        assert_eq!(
            mapping.focus_origin(),
            Point::<f64, Logical>::from((0.0, 1920.0))
        );
        assert_eq!(
            compositor_point - mapping.focus_origin(),
            Point::<f64, Logical>::from((540.0, 960.0))
        );
    }

    #[test]
    fn role_surface_mapping_offsets_global_surface_origin_for_source_geometry() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((40, 20).into(), (80, 40).into()),
            Rectangle::<i32, Logical>::new((0, 0).into(), (800, 400).into()),
        );

        assert_eq!(
            mapping.origin,
            Point::<f64, Logical>::from((-400.0, -200.0))
        );
    }

    #[test]
    fn role_surface_mapping_keeps_popup_relative_position_under_same_scale() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((0, 0).into(), (80, 40).into()),
            Rectangle::<i32, Logical>::new((0, 0).into(), (800, 400).into()),
        );

        assert_eq!(
            mapping.map_rect(Rectangle::new((60, 10).into(), (20, 10).into())),
            Rectangle::<i32, Logical>::new((600, 100).into(), (200, 100).into())
        );
    }

    #[test]
    fn role_surface_mapping_reports_render_element_transform_for_scaled_popup_tree() {
        let mapping = RoleSurfaceMapping::new(
            Rectangle::<i32, Logical>::new((40, 20).into(), (80, 40).into()),
            Rectangle::<i32, Logical>::new((944, 16).into(), (960, 540).into()),
        );

        assert_eq!(mapping.render_element_location(), (464, -254).into());
        assert_eq!(mapping.render_element_scale().x, 12.0);
        assert_eq!(mapping.render_element_scale().y, 13.5);
        assert_eq!(
            mapping.map_render_element_location((60, 10).into()),
            (1184, -119).into()
        );
    }

    #[test]
    fn source_rect_prefers_window_geometry_when_it_is_inside_bbox() {
        let bbox = Rectangle::<i32, Logical>::new((-32, -16).into(), (864, 632).into());
        let geometry = Rectangle::<i32, Logical>::new((0, 0).into(), (800, 600).into());

        assert_eq!(
            source_rect_from_bbox_and_geometry(bbox, Some(geometry)),
            geometry
        );
    }

    #[test]
    fn source_rect_prefers_committed_geometry_even_when_renderer_bbox_grows_beyond_it() {
        let bbox = Rectangle::<i32, Logical>::new((-32, -16).into(), (864, 1264).into());
        let geometry = Rectangle::<i32, Logical>::new((0, 0).into(), (800, 600).into());

        assert_eq!(
            source_rect_from_bbox_and_geometry(bbox, Some(geometry)),
            geometry
        );
    }

    #[test]
    fn source_rect_falls_back_to_bbox_when_geometry_is_missing() {
        let bbox = Rectangle::<i32, Logical>::new((-12, -8).into(), (824, 616).into());

        assert_eq!(source_rect_from_bbox_and_geometry(bbox, None), bbox);
    }

    #[test]
    fn source_rect_falls_back_to_bbox_when_geometry_is_non_positive() {
        let bbox = Rectangle::<i32, Logical>::new((-12, -8).into(), (824, 616).into());
        let geometry = Rectangle::<i32, Logical>::new((0, 0).into(), (0, 600).into());

        assert_eq!(
            source_rect_from_bbox_and_geometry(bbox, Some(geometry)),
            bbox
        );
    }

    #[test]
    fn atomic_plane_zpos_selection_prefers_deterministic_primary_below_overlay_values() {
        let selected = select_atomic_plane_zpos_values(0, 5, 0, 5)
            .expect("compatible zpos ranges should select deterministic ordering");
        assert_eq!(selected, (0, 1));
    }

    #[test]
    fn atomic_plane_zpos_selection_rejects_non_orderable_ranges() {
        let selected = select_atomic_plane_zpos_values(5, 8, 0, 5);
        assert!(
            selected.is_none(),
            "overlay zpos must be strictly greater than primary zpos"
        );
    }

    #[test]
    fn primary_scanout_format_prefers_xrgb_then_argb() {
        let formats = [DrmFourcc::Argb8888 as u32, DrmFourcc::Xrgb8888 as u32];
        let selected = select_preferred_scanout_format(&formats, PlaneSelection::Primary);
        assert_eq!(selected, Some(DrmFourcc::Xrgb8888));
    }

    #[test]
    fn primary_scanout_format_falls_back_to_argb_when_xrgb_missing() {
        let formats = [DrmFourcc::Argb8888 as u32];
        let selected = select_preferred_scanout_format(&formats, PlaneSelection::Primary);
        assert_eq!(selected, Some(DrmFourcc::Argb8888));
    }

    #[test]
    fn overlay_scanout_format_prefers_argb_for_alpha_truth() {
        let formats = [DrmFourcc::Argb8888 as u32, DrmFourcc::Xrgb8888 as u32];
        let selected = select_preferred_scanout_format(&formats, PlaneSelection::Overlay);
        assert_eq!(selected, Some(DrmFourcc::Argb8888));
    }

    #[test]
    fn overlay_scanout_format_falls_back_to_xrgb_when_argb_missing() {
        let formats = [DrmFourcc::Xrgb8888 as u32];
        let selected = select_preferred_scanout_format(&formats, PlaneSelection::Overlay);
        assert_eq!(selected, Some(DrmFourcc::Xrgb8888));
    }

    #[test]
    fn overlay_scanout_alpha_support_requires_argb_format() {
        assert!(overlay_scanout_format_supports_alpha(DrmFourcc::Argb8888));
        assert!(!overlay_scanout_format_supports_alpha(DrmFourcc::Xrgb8888));
    }
}
