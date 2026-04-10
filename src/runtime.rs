use crate::model::{
    OutputRotation, RuntimeBackend, RuntimeDmabufFormatStatus, RuntimeFocusTarget,
    RuntimeHostPresentOwnership, RuntimeHostQueuedPresentSource,
};
use crate::state::CompositorState;
use rustix::fs::OFlags;
use rustix::io::dup;
use smithay::backend::allocator::gbm::{GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{
    Buffer, Format, Fourcc, Modifier,
    dmabuf::{AsDmabuf, Dmabuf},
};
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_bo};
use smithay::backend::drm::{DrmDeviceFd, DrmNode};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::{
    draw_render_elements, import_surface_tree, on_commit_buffer_handler,
};
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, ImportDma, Offscreen, Renderer, TextureMapping,
};
use smithay::backend::session::Event as SessionSignal;
use smithay::backend::session::Session;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev::{UdevBackend, UdevEvent, primary_gpu};
use smithay::backend::winit::{self, WinitEvent};
use smithay::delegate_compositor;
use smithay::delegate_dmabuf;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_xdg_shell;
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
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
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{
    Buffer as BufferCoords, DeviceFd, Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Serial,
    Size, Transform,
};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState as SmithayCompositorState,
    SubsurfaceCachedState, SurfaceAttributes, TraversalAction, with_surface_tree_downward,
};
use smithay::wayland::dmabuf::{
    DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier, get_dmabuf,
};
use smithay::wayland::shell::xdg::{
    Configure, PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{BufferAccessError, ShmHandler, ShmState, with_buffer_contents};
use smithay::wayland::socket::ListeningSocketSource;
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;

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

    let mut wayland_state = RuntimeWaylandState::new(display_handle.clone(), shared_state.clone());
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
                data.wayland_state.prune_dead_surfaces();
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                let rotation = { lock_state(&data.shared_state).output_rotation() };
                let transform = transform_from_rotation(rotation);

                let render_result = (|| {
                    {
                        let (renderer, mut framebuffer) = backend
                            .bind()
                            .map_err(|err| format!("failed to bind winit frame: {err}"))?;

                        let capture = data
                            .wayland_state
                            .collect_render_elements(renderer, size.w, size.h);
                        let mut frame = renderer
                            .render(&mut framebuffer, size, transform)
                            .map_err(|err| format!("failed to start render pass: {err}"))?;
                        frame
                            .clear(Color32F::new(0.08, 0.08, 0.1, 1.0), &[damage])
                            .map_err(|err| format!("failed to clear frame: {err}"))?;
                        draw_render_elements(&mut frame, 1.0, &capture.elements, &[damage])
                            .map_err(|err| format!("failed to draw surface elements: {err}"))?;
                        let _ = frame
                            .finish()
                            .map_err(|err| format!("failed to finish render pass: {err}"))?;
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

pub fn run_host(shared_state: Arc<Mutex<CompositorState>>) -> Result<(), RuntimeError> {
    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_starting(RuntimeBackend::HostDrm);
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

    let display: Display<RuntimeWaylandState> =
        Display::new().map_err(|err| RuntimeError::WaylandDisplay(err.to_string()))?;
    let display_handle = display.handle();

    let mut wayland_state = RuntimeWaylandState::new(display_handle.clone(), shared_state.clone());
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

    let (session, session_notifier) =
        LibSeatSession::new().map_err(|err| RuntimeError::HostSession(err.to_string()))?;
    let seat_name = session.seat();
    let udev =
        UdevBackend::new(&seat_name).map_err(|err| RuntimeError::HostUdev(err.to_string()))?;
    let preferred_primary_path = primary_gpu(&seat_name).ok().flatten();

    let mut host_backend =
        HostBackendState::new(session, seat_name.clone(), preferred_primary_path.clone());
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

    let claimed_output = host_backend.claim_output_ownership(None)?;
    sync_runtime_host_present_capabilities(&shared_state, &host_backend);
    wayland_state.sync_dmabuf_protocol_formats(host_backend.claimed_dmabuf_formats());
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
            let _ = data
                .display_handle
                .insert_client(client_stream, Arc::new(RuntimeClientState::default()));
        })
        .map_err(|err| RuntimeError::RegisterSource(err.to_string()))?;
    wayland_state.reconfigure_roles(mode_w as i32, mode_h as i32);
    wayland_state.sync_runtime_status_with_roles();
    {
        let mut state = lock_state(&shared_state);
        state.mark_runtime_running(
            RuntimeBackend::HostDrm,
            Some(socket_name.clone()),
            mode_w as i32,
            mode_h as i32,
        );
        state.set_runtime_host_backend_snapshot(
            Some(host_backend.seat_name.clone()),
            host_backend.detected_count(),
            host_backend.opened_count(),
            host_backend.primary_opened_path(),
        );
        let (ownership, atomic_enabled, overlay_capable) = host_backend
            .claimed_present_capabilities()
            .unwrap_or((RuntimeHostPresentOwnership::None, false, false));
        state.set_runtime_host_present_capabilities(ownership, atomic_enabled, overlay_capable);
    }

    let drm_events_source_token = Rc::new(RefCell::new(None));
    bind_claimed_drm_event_source(
        &loop_handle,
        drm_events_fd,
        Rc::clone(&drm_events_source_token),
    )?;

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
                    let mut state = lock_state(&data.shared_state);
                    state.mark_runtime_failed(format!(
                        "host backend has no claimed output and reclaim failed: {err}"
                    ));
                    data.loop_signal.stop();
                    return TimeoutAction::Drop;
                }
                return TimeoutAction::ToDuration(Duration::from_millis(16));
            }
            match data
                .host_backend
                .queue_claimed_presentation_tick(&data.wayland_state)
            {
                Ok(_) => {}
                Err(failure) => {
                    if failure.is_reclaimable() {
                        data.host_backend.claimed_output = None;
                        sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
                        data.wayland_state.sync_dmabuf_protocol_formats(None);
                        if let Err(reclaim_err) = reclaim_host_output_in_process(
                            data,
                            &loop_handle_for_timer,
                            &drm_events_source_token_for_timer,
                            reclaim_required_ownership,
                        ) {
                            let mut state = lock_state(&data.shared_state);
                            state.mark_runtime_failed(format!(
                                "host present/commit recovery failed after queue error ({}): {reclaim_err}",
                                failure.error_ref()
                            ));
                            data.loop_signal.stop();
                            return TimeoutAction::Drop;
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
                    let mut state = lock_state(&data.shared_state);
                    state.set_runtime_host_backend_snapshot(
                        Some(data.host_backend.seat_name.clone()),
                        data.host_backend.detected_count(),
                        data.host_backend.opened_count(),
                        data.host_backend.primary_opened_path(),
                    );
                    state.mark_runtime_failed(format!(
                        "host backend lost claimed output and reclaim failed: {err}"
                    ));
                    data.loop_signal.stop();
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPresentFailureClass {
    Reclaimable,
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

    fn is_reclaimable(&self) -> bool {
        matches!(self.class, HostPresentFailureClass::Reclaimable)
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
    if completed > 0 {
        data.wayland_state.prune_dead_surfaces();
        data.wayland_state.send_frame_callbacks();
        let _ = data.display_handle.flush_clients();
        let mut state = lock_state(&data.shared_state);
        for _ in 0..completed {
            state.mark_runtime_redraw();
        }
        state.poll_processes();
    }
    Ok(())
}

fn sync_runtime_host_present_capabilities(
    shared_state: &Arc<Mutex<CompositorState>>,
    host_backend: &HostBackendState,
) {
    let (ownership, atomic_enabled, overlay_capable) = host_backend
        .claimed_present_capabilities()
        .unwrap_or((RuntimeHostPresentOwnership::None, false, false));
    let mut state = lock_state(shared_state);
    state.set_runtime_host_present_capabilities(ownership, atomic_enabled, overlay_capable);
}

fn reclaim_host_output_in_process(
    data: &mut HostRuntimeLoopData,
    loop_handle: &LoopHandle<'_, HostRuntimeLoopData>,
    drm_events_source_token: &Rc<RefCell<Option<RegistrationToken>>>,
    reclaim_required_ownership: Option<StartupPresentOwnership>,
) -> Result<(), RuntimeError> {
    let claimed_output = data
        .host_backend
        .claim_output_ownership(reclaim_required_ownership)?;
    sync_runtime_host_present_capabilities(&data.shared_state, &data.host_backend);
    data.wayland_state
        .sync_dmabuf_protocol_formats(data.host_backend.claimed_dmabuf_formats());
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
    data.wayland_state
        .reconfigure_roles(mode_w as i32, mode_h as i32);
    data.wayland_state.sync_runtime_status_with_roles();
    let mut state = lock_state(&data.shared_state);
    state.mark_runtime_resize(mode_w as i32, mode_h as i32);
    state.set_runtime_host_backend_snapshot(
        Some(data.host_backend.seat_name.clone()),
        data.host_backend.detected_count(),
        data.host_backend.opened_count(),
        data.host_backend.primary_opened_path(),
    );
    Ok(())
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
                        data.host_backend.claimed_output = None;
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
    session: LibSeatSession,
    seat_name: String,
    preferred_primary_path: Option<PathBuf>,
    detected_devices: HashMap<u64, PathBuf>,
    opened_devices: HashMap<u64, OpenedHostDevice>,
    claimed_output: Option<ClaimedHostOutput>,
}

struct OpenedHostDevice {
    path: PathBuf,
    _node: DrmNode,
    fd: OwnedFd,
    claimed_pipeline: Option<ClaimedPresentationPipeline>,
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
    flip_pending: bool,
    pending_flip_source: Option<QueuedFlipSource>,
    gles_renderer: Option<HostGlesRendererState>,
}

struct HostGlesRendererState {
    _gbm_device: GbmDevice<DeviceFd>,
    _drm_device_fd: DrmDeviceFd,
    _egl_display: EGLDisplay,
    renderer: GlesRenderer,
    target_texture: GlesTexture,
    size: Size<i32, BufferCoords>,
    primary_scanout_format: DrmFourcc,
    overlay_scanout_format: Option<DrmFourcc>,
    direct_scanout: Option<HostDirectScanoutState>,
    overlay_scanout: Option<HostOverlayScanoutState>,
}

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

#[derive(Clone, Copy)]
struct ClaimedHostOutput {
    device_id: u64,
    mode: DrmMode,
    startup_present_ownership: StartupPresentOwnership,
}

struct OutputClaimPlan {
    connector: drm_connector::Handle,
    crtc: drm_crtc::Handle,
    mode: DrmMode,
    atomic: Option<AtomicClaimPlan>,
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
}

#[derive(Clone)]
struct AtomicPlaneZposProperty {
    handle: drm_property::Handle,
    min: u64,
    max: u64,
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
    fn new(
        session: LibSeatSession,
        seat_name: String,
        preferred_primary_path: Option<PathBuf>,
    ) -> Self {
        Self {
            session,
            seat_name,
            preferred_primary_path,
            detected_devices: HashMap::new(),
            opened_devices: HashMap::new(),
            claimed_output: None,
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
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY)
            .map_err(|err| RuntimeError::HostDeviceOpen {
                path: path.display().to_string(),
                error: err.to_string(),
            })?;
        self.opened_devices.insert(
            device_id,
            OpenedHostDevice {
                path: path.to_path_buf(),
                _node: node,
                fd,
                claimed_pipeline: None,
            },
        );
        Ok(())
    }

    fn claim_output_ownership(
        &mut self,
        required_startup_ownership: Option<StartupPresentOwnership>,
    ) -> Result<ClaimedHostOutput, RuntimeError> {
        let mut device_ids: Vec<u64> = self.opened_devices.keys().copied().collect();
        let mut last_error: Option<RuntimeError> = None;
        device_ids.sort_by(|left, right| {
            let left_path = &self.opened_devices.get(left).expect("device exists").path;
            let right_path = &self.opened_devices.get(right).expect("device exists").path;
            host_device_sort_key(left_path, self.preferred_primary_path.as_deref()).cmp(
                &host_device_sort_key(right_path, self.preferred_primary_path.as_deref()),
            )
        });

        for device_id in device_ids {
            let Some(opened) = self.opened_devices.get_mut(&device_id) else {
                continue;
            };
            let plan = match build_output_claim_plan(opened) {
                Ok(Some(plan)) => plan,
                Ok(None) => continue,
                Err(err) => {
                    eprintln!(
                        "host backend failed to inspect output claim plan on {}: {err}",
                        opened.path.display()
                    );
                    last_error = Some(err);
                    continue;
                }
            };
            match claim_output_on_device(opened, plan, required_startup_ownership) {
                Ok(claimed) => {
                    let claimed_output = ClaimedHostOutput {
                        device_id,
                        mode: claimed.mode,
                        startup_present_ownership: claimed.startup_present_ownership,
                    };
                    self.claimed_output = Some(claimed_output);
                    return Ok(claimed_output);
                }
                Err(err) => {
                    eprintln!(
                        "host backend failed to claim output ownership on {}: {err}",
                        opened.path.display()
                    );
                    last_error = Some(err);
                    continue;
                }
            }
        }

        match last_error {
            Some(err) => Err(err),
            None => Err(RuntimeError::HostNoConnectedOutputRoute),
        }
    }

    fn claimed_device_event_fd(&self) -> Option<OwnedFd> {
        let claimed = self.claimed_output?;
        let opened = self.opened_devices.get(&claimed.device_id)?;
        dup(opened.fd.as_fd()).ok()
    }

    fn claimed_dmabuf_formats(&self) -> Option<Vec<Format>> {
        let claimed = self.claimed_output?;
        let opened = self.opened_devices.get(&claimed.device_id)?;
        let pipeline = opened.claimed_pipeline.as_ref()?;
        let gles = pipeline.gles_renderer.as_ref()?;
        Some(gles.renderer.dmabuf_formats().iter().copied().collect())
    }

    fn claimed_present_capabilities(&self) -> Option<(RuntimeHostPresentOwnership, bool, bool)> {
        let claimed = self.claimed_output?;
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
            .unwrap_or(false);
        Some((ownership, atomic_enabled, overlay_capable))
    }

    fn queue_claimed_presentation_tick(
        &mut self,
        wayland_state: &RuntimeWaylandState,
    ) -> Result<bool, HostPresentFailure> {
        let claimed = match self.claimed_output {
            Some(claimed) => claimed,
            None => return Ok(false),
        };
        let Some(opened) = self.opened_devices.get_mut(&claimed.device_id) else {
            return Ok(false);
        };
        let Some(pipeline) = opened.claimed_pipeline.as_mut() else {
            return Ok(false);
        };
        if pipeline.flip_pending {
            return Ok(false);
        }
        let requires_direct_present = matches!(
            claimed.startup_present_ownership,
            StartupPresentOwnership::DirectGbm
        );

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
        let overlay_plane_rotation_supported = matches!(
            lock_state(&wayland_state.shared_state).output_rotation(),
            OutputRotation::Deg0
        );
        let overlay_plane_alpha_format_supported = pipeline
            .atomic_commit_state
            .as_ref()
            .and_then(|atomic| atomic.overlay_scanout_format)
            .map(overlay_scanout_format_supports_alpha)
            .unwrap_or(false);
        let prefer_overlay_plane_split =
            overlay_plane_rotation_supported && overlay_plane_alpha_format_supported;
        if let Some(gles_renderer) = pipeline.gles_renderer.as_mut() {
            match render_host_scene_with_gles_direct(
                gles_renderer,
                wayland_state,
                &opened.path,
                mode_w as i32,
                mode_h as i32,
                prefer_overlay_plane_split,
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
                pipeline.gles_renderer = None;
            }
            if !rendered_with_gles_readback {
                let _ = wayland_state.compose_host_scene(
                    &mut mapping,
                    stride,
                    mode_w as i32,
                    mode_h as i32,
                );
            }
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
            if let Err(err) = queue_atomic_frame_commit(
                &card,
                &opened.path,
                atomic,
                Some(queued_framebuffer),
                overlay_framebuffer,
                wayland_state,
            ) {
                return Err(HostPresentFailure::reclaimable(err));
            }
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
        pipeline.flip_pending = true;
        pipeline.pending_flip_source = Some(queued_source);
        Ok(true)
    }

    fn process_claimed_presentation_events(&mut self) -> Result<u64, HostPresentFailure> {
        let claimed = match self.claimed_output {
            Some(claimed) => claimed,
            None => return Ok(0),
        };
        let Some(opened) = self.opened_devices.get_mut(&claimed.device_id) else {
            return Ok(0);
        };
        let Some(pipeline) = opened.claimed_pipeline.as_mut() else {
            return Ok(0);
        };

        let card = HostKmsCard::new(&opened.fd);
        let events = card.receive_events().map_err(|err| {
            HostPresentFailure::reclaimable(RuntimeError::HostOutputClaim {
                path: opened.path.display().to_string(),
                error: format!("failed to receive drm events: {err}"),
            })
        })?;

        let mut completed = 0u64;
        for event in events {
            if let drm_api::control::Event::PageFlip(flip) = event {
                if flip.crtc == pipeline.crtc && pipeline.flip_pending {
                    match pipeline.pending_flip_source {
                        Some(QueuedFlipSource::Dumb) => {
                            std::mem::swap(
                                &mut pipeline.dumb_front_buffer,
                                &mut pipeline.dumb_back_buffer,
                            );
                        }
                        Some(QueuedFlipSource::DirectGbm) => {
                            if let Some(gles_renderer) = pipeline.gles_renderer.as_mut() {
                                if let Some(direct_scanout) = gles_renderer.direct_scanout.as_mut()
                                {
                                    std::mem::swap(
                                        &mut direct_scanout.front_buffer,
                                        &mut direct_scanout.back_buffer,
                                    );
                                }
                            }
                        }
                        None => {}
                    }
                    pipeline.flip_pending = false;
                    pipeline.pending_flip_source = None;
                    completed = completed.saturating_add(1);
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
        let Some(opened) = self.opened_devices.remove(&device_id) else {
            return Ok(());
        };
        if let Some(pipeline) = opened.claimed_pipeline {
            let card = HostKmsCard::new(&opened.fd);
            if let Some(dumb_buffers) = pipeline.dumb_buffers {
                for buffer in dumb_buffers {
                    let _ = card.destroy_framebuffer(buffer.fb);
                    let _ = card.destroy_dumb_buffer(buffer.dumb);
                }
            }
        }
        self.session
            .close(opened.fd)
            .map_err(|err| RuntimeError::HostDeviceClose {
                path: opened.path.display().to_string(),
                error: err.to_string(),
            })?;
        Ok(())
    }

    fn remove_device(&mut self, device_id: u64) -> Result<(), RuntimeError> {
        self.detected_devices.remove(&device_id);
        self.close_device(device_id)
    }
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
}

fn build_output_claim_plan(
    opened: &OpenedHostDevice,
) -> Result<Option<OutputClaimPlan>, RuntimeError> {
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
            if let Some(current_crtc) = encoder_info.crtc() {
                if crtcs.contains(&current_crtc) {
                    let atomic = if atomic_client_enabled {
                        match build_atomic_claim_plan(
                            &card,
                            &opened.path,
                            &resources,
                            connector_info.handle(),
                            current_crtc,
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
                    return Ok(Some(OutputClaimPlan {
                        connector: connector_info.handle(),
                        crtc: current_crtc,
                        mode,
                        atomic,
                    }));
                }
            }
            if let Some(crtc) = crtcs.first().copied() {
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
                return Ok(Some(OutputClaimPlan {
                    connector: connector_info.handle(),
                    crtc,
                    mode,
                    atomic,
                }));
            }
        }
    }

    Ok(None)
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
    };

    let mut overlay_state = if let Some(overlay_plane) =
        select_atomic_plane(card, device_path, resources, crtc, PlaneSelection::Overlay)?
    {
        plane_property_handles(card, device_path, overlay_plane.handle)?.map(|props| {
            AtomicPlaneState {
                role: AtomicPlaneRole::Overlay,
                handle: overlay_plane.handle,
                props,
                scanout_format: overlay_plane.scanout_format,
                zpos: None,
            }
        })
    } else {
        None
    };

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
    }))
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
        lock_state(&wayland_state.shared_state).output_rotation(),
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

fn claim_output_on_device(
    opened: &mut OpenedHostDevice,
    plan: OutputClaimPlan,
    required_startup_ownership: Option<StartupPresentOwnership>,
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
        .and_then(|atomic| atomic.overlay_scanout_format)
        .filter(|format| overlay_scanout_format_supports_alpha(*format));
    let requires_direct_startup = matches!(
        required_startup_ownership,
        Some(StartupPresentOwnership::DirectGbm)
    );

    let gles_renderer = match build_host_gles_renderer_state(
        &opened.fd,
        &opened.path,
        plan.mode.size(),
        primary_scanout_format,
        overlay_scanout_format,
    ) {
        Ok(mut renderer) => {
            match prime_direct_startup_frame(&mut renderer, &opened.path, plan.mode.size()) {
                Ok(Some(framebuffer)) => {
                    if let Some(atomic) = atomic_candidate.as_ref() {
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
                    if !used_direct_startup {
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
        if let Some(atomic) = atomic_candidate.as_ref() {
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
        if !claimed_with_atomic {
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
    opened.claimed_pipeline = Some(ClaimedPresentationPipeline {
        crtc: plan.crtc,
        dumb_buffers,
        dumb_front_buffer: 0,
        dumb_back_buffer: 1,
        atomic_commit_state,
        flip_pending: false,
        pending_flip_source: None,
        gles_renderer,
    });
    Ok(ClaimedOutput {
        mode: plan.mode,
        startup_present_ownership: if used_direct_startup {
            StartupPresentOwnership::DirectGbm
        } else {
            StartupPresentOwnership::Dumb
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
    let render_fd = dup(fd.as_fd()).map_err(|err| RuntimeError::HostOutputClaim {
        path: device_path.display().to_string(),
        error: format!("failed to duplicate drm fd for gbm/egl renderer: {err}"),
    })?;
    let gbm_device =
        GbmDevice::new(DeviceFd::from(render_fd)).map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gbm device: {err}"),
        })?;
    let egl_display = unsafe { EGLDisplay::new(gbm_device.clone()) }.map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create egl display for gbm device: {err}"),
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
        .create_buffer(primary_scanout_format, size)
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to create gles offscreen render target: {err}"),
        })?;
    let direct_scanout = match build_host_direct_scanout_state(
        &drm_device_fd,
        &gbm_device,
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
        _gbm_device: gbm_device,
        _drm_device_fd: drm_device_fd,
        _egl_display: egl_display,
        renderer,
        target_texture,
        size,
        primary_scanout_format,
        overlay_scanout_format,
        direct_scanout,
        overlay_scanout: None,
    })
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
    let gbm_buffer = GbmBuffer::from_bo(bo, true);
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
        &gles_state._gbm_device,
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
        ._gbm_device
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
    let gbm_buffer = GbmBuffer::from_bo(gbm_buffer, true);
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
    wayland_state: &RuntimeWaylandState,
    device_path: &Path,
    output_w: i32,
    output_h: i32,
    prefer_overlay_plane_split: bool,
) -> Result<DirectRenderTargets, RuntimeError> {
    let size = Size::<i32, BufferCoords>::from((output_w.max(1), output_h.max(1)));
    if gles_state.size != size {
        gles_state.target_texture = gles_state
            .renderer
            .create_buffer(DrmFourcc::Xrgb8888, size)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to resize gles offscreen render target: {err}"),
            })?;
        gles_state.direct_scanout = None;
        gles_state.size = size;
    }

    ensure_direct_scanout_state(gles_state, device_path, size)?;

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

    let rotation = { lock_state(&wayland_state.shared_state).output_rotation() };
    let transform = transform_from_rotation(rotation);
    let render_size = Size::<i32, Physical>::from((output_w.max(1), output_h.max(1)));
    let damage = Rectangle::from_size(render_size);
    let capture =
        wayland_state.collect_render_elements(&mut gles_state.renderer, output_w, output_h);
    let overlay_framebuffer = if prefer_overlay_plane_split {
        render_overlay_plane_framebuffer(gles_state, wayland_state, device_path)?
    } else {
        gles_state.overlay_scanout = None;
        None
    };
    let primary_elements = if overlay_framebuffer.is_some() {
        capture.primary_plane_slice()
    } else {
        &capture.elements
    };
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
    draw_render_elements(&mut frame, 1.0, primary_elements, &[damage]).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw scene elements into direct scanout buffer: {err}"),
        }
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish direct gles render pass: {err}"),
        })?;

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
    draw_render_elements(&mut frame, 1.0, &overlay_elements, &[damage]).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw overlay elements: {err}"),
        }
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
    wayland_state: &RuntimeWaylandState,
    device_path: &Path,
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) -> Result<(), RuntimeError> {
    let size = Size::<i32, BufferCoords>::from((output_w.max(1), output_h.max(1)));
    if gles_state.size != size {
        gles_state.target_texture = gles_state
            .renderer
            .create_buffer(DrmFourcc::Xrgb8888, size)
            .map_err(|err| RuntimeError::HostOutputClaim {
                path: device_path.display().to_string(),
                error: format!("failed to resize gles host render target: {err}"),
            })?;
        gles_state.size = size;
    }

    let rotation = { lock_state(&wayland_state.shared_state).output_rotation() };
    let transform = transform_from_rotation(rotation);
    let render_size = Size::<i32, Physical>::from((output_w.max(1), output_h.max(1)));
    let damage = Rectangle::from_size(render_size);
    let capture =
        wayland_state.collect_render_elements(&mut gles_state.renderer, output_w, output_h);

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
    draw_render_elements(&mut frame, 1.0, &capture.elements, &[damage]).map_err(|err| {
        RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to draw scene elements with gles: {err}"),
        }
    })?;
    let _ = frame
        .finish()
        .map_err(|err| RuntimeError::HostOutputClaim {
            path: device_path.display().to_string(),
            error: format!("failed to finish gles render pass: {err}"),
        })?;

    let readback_region = Rectangle::from_size(size);
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
        target,
        target_stride,
        output_w.max(1) as usize,
        output_h.max(1) as usize,
    );
    Ok(())
}

fn copy_renderer_pixels_to_dumb(
    src_pixels: &[u8],
    src_flipped: bool,
    dst_pixels: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    let src_stride = width.saturating_mul(4);
    if src_stride == 0 || dst_stride == 0 {
        return;
    }
    for y in 0..height {
        let src_y = if src_flipped {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeSurfaceRole {
    MainApp,
    OverlayNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceClassification {
    MainApp,
    OverlayCandidate,
    PendingIdentity,
}

struct RuntimeWaylandState {
    shared_state: Arc<Mutex<CompositorState>>,
    display_handle: DisplayHandle,
    compositor_state: SmithayCompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    dmabuf_state: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    dmabuf_formats: Vec<Format>,
    seat_state: SeatState<Self>,
    seat: Seat<Self>,
    main_toplevel: Option<ToplevelSurface>,
    overlay_toplevel: Option<ToplevelSurface>,
    pending_toplevels: Vec<ToplevelSurface>,
    popups: Vec<ManagedPopup>,
    pointer_location: Point<f64, Logical>,
    start_time: std::time::Instant,
    host_surface_buffers: HashMap<u32, SurfaceBufferSnapshot>,
}

const DMABUF_PROTOCOL_FORMATS: [Format; 4] = [
    Format {
        code: Fourcc::Argb8888,
        modifier: Modifier::Invalid,
    },
    Format {
        code: Fourcc::Argb8888,
        modifier: Modifier::Linear,
    },
    Format {
        code: Fourcc::Xrgb8888,
        modifier: Modifier::Invalid,
    },
    Format {
        code: Fourcc::Xrgb8888,
        modifier: Modifier::Linear,
    },
];

#[derive(Debug, Clone, Copy)]
enum RenderElementSource {
    Main,
    MainPopup,
    Overlay,
    OverlayPopup,
}

#[derive(Default)]
struct RenderElementCapture {
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    counts: RenderElementCounts,
}

impl RenderElementCapture {
    fn push(
        &mut self,
        source: RenderElementSource,
        new_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    ) {
        let added = new_elements.len();
        if added == 0 {
            return;
        }
        self.elements.extend(new_elements);
        match source {
            RenderElementSource::Main => self.counts.main += added,
            RenderElementSource::MainPopup => self.counts.main_popups += added,
            RenderElementSource::Overlay => self.counts.overlay += added,
            RenderElementSource::OverlayPopup => self.counts.overlay_popups += added,
        }
    }

    fn primary_plane_slice(&self) -> &[WaylandSurfaceRenderElement<GlesRenderer>] {
        let end = self.counts.main + self.counts.main_popups;
        &self.elements[..end]
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct RenderElementCounts {
    main: usize,
    main_popups: usize,
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
}

struct HostSceneSurface {
    buffer: wl_buffer::WlBuffer,
    kind: SurfaceBufferKind,
    target: Rectangle<i32, Logical>,
    dmabuf: Option<SurfaceDmabufInfo>,
}

#[derive(Default, Debug, Clone, Copy)]
struct HostSceneComposeStats {
    attempted_surfaces: u32,
    composed_surfaces: u32,
}

impl RuntimeWaylandState {
    fn new(display_handle: DisplayHandle, shared_state: Arc<Mutex<CompositorState>>) -> Self {
        let compositor_state = SmithayCompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let dmabuf_formats = DMABUF_PROTOCOL_FORMATS.to_vec();
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = if dmabuf_formats.is_empty() {
            None
        } else {
            Some(
                dmabuf_state.create_global::<Self>(&display_handle, dmabuf_formats.iter().copied()),
            )
        };
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "winit");
        let _ = seat.add_keyboard(Default::default(), 200, 25);
        seat.add_pointer();

        let state = Self {
            shared_state,
            display_handle,
            compositor_state,
            xdg_shell_state,
            shm_state,
            dmabuf_state,
            dmabuf_global,
            dmabuf_formats,
            seat_state,
            seat,
            main_toplevel: None,
            overlay_toplevel: None,
            pending_toplevels: Vec::new(),
            popups: Vec::new(),
            pointer_location: (0.0, 0.0).into(),
            start_time: std::time::Instant::now(),
            host_surface_buffers: HashMap::new(),
        };
        state.sync_runtime_dmabuf_protocol_status();
        state
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

    fn sync_dmabuf_protocol_formats(&mut self, formats: Option<Vec<Format>>) {
        let formats = formats.unwrap_or_default();
        if formats == self.dmabuf_formats {
            self.sync_runtime_dmabuf_protocol_status();
            return;
        }
        if let Some(global) = self.dmabuf_global.take() {
            self.dmabuf_state
                .disable_global::<Self>(&self.display_handle, &global);
            self.dmabuf_state
                .destroy_global::<Self>(&self.display_handle, global);
        }
        if formats.is_empty() {
            self.dmabuf_formats = formats;
            self.sync_runtime_dmabuf_protocol_status();
            return;
        }
        self.dmabuf_global = Some(
            self.dmabuf_state
                .create_global::<Self>(&self.display_handle, formats.iter().copied()),
        );
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
                    keyboard.input::<(), _>(
                        self,
                        event.key_code(),
                        event.state(),
                        serial,
                        event.time_msec(),
                        |_, _, _| FilterResult::Forward,
                    );
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output_w = self.runtime_output_width();
                let output_h = self.runtime_output_height();
                let pos = event.position_transformed((output_w, output_h).into());
                self.pointer_location = pos;
                let serial = SERIAL_COUNTER.next_serial();

                let under = self.surface_under_point(pos);
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
                        let surface_under = self.surface_under_point(self.pointer_location);
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
        let requested_target = {
            lock_state(&self.shared_state)
                .status_snapshot()
                .runtime
                .active_focus_target
        };
        let resolved = match requested_target {
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
            None => None,
        }
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
        if requested_target != resolved_target {
            let mut state = lock_state(&self.shared_state);
            state.set_runtime_focus_target(resolved_target);
        }
    }

    fn assign_toplevel_role(&mut self, surface: ToplevelSurface) {
        match self.classify_toplevel(&surface) {
            SurfaceClassification::MainApp => self.assign_main_role(surface),
            SurfaceClassification::OverlayCandidate => self.assign_overlay_role_or_queue(surface),
            SurfaceClassification::PendingIdentity => self.pending_toplevels.push(surface),
        }
    }

    fn assign_main_role(&mut self, surface: ToplevelSurface) {
        if self.main_toplevel.is_none() {
            self.configure_toplevel_for_role(&surface, RuntimeSurfaceRole::MainApp);
            self.main_toplevel = Some(surface);
            self.promote_pending_toplevels();
            self.sync_runtime_status_with_roles();
            self.apply_focus_route();
        } else {
            if self
                .main_toplevel
                .as_ref()
                .map(|main| surface_id(main.wl_surface()) != surface_id(surface.wl_surface()))
                .unwrap_or(true)
            {
                surface.send_close();
                let mut state = lock_state(&self.shared_state);
                state.increment_runtime_denied_toplevel();
            }
        }
    }

    fn assign_overlay_role_or_queue(&mut self, surface: ToplevelSurface) {
        if self.main_toplevel.is_none() {
            self.pending_toplevels.push(surface);
            return;
        }
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
                SurfaceClassification::OverlayCandidate => {
                    self.assign_overlay_role_or_queue(surface)
                }
                SurfaceClassification::PendingIdentity => self.pending_toplevels.push(surface),
            }
        }
    }

    fn classify_toplevel(&self, surface: &ToplevelSurface) -> SurfaceClassification {
        let (app_id, title) =
            smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                let attrs = states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().ok());
                let app_id = attrs.as_ref().and_then(|attrs| attrs.app_id.clone());
                let title = attrs.as_ref().and_then(|attrs| attrs.title.clone());
                (app_id, title)
            });

        if app_id.is_none() && title.is_none() {
            return SurfaceClassification::PendingIdentity;
        }

        let hint = {
            let state = lock_state(&self.shared_state);
            state.runtime_main_app_match_hint().to_lowercase()
        };
        let app_id_l = app_id.unwrap_or_default().to_lowercase();
        let title_l = title.unwrap_or_default().to_lowercase();
        if !hint.is_empty() && (app_id_l.contains(&hint) || title_l.contains(&hint)) {
            SurfaceClassification::MainApp
        } else {
            SurfaceClassification::OverlayCandidate
        }
    }

    fn runtime_output_width(&self) -> i32 {
        lock_state(&self.shared_state)
            .status_snapshot()
            .runtime
            .window_width
            .unwrap_or(1280)
            .max(1)
    }

    fn runtime_output_height(&self) -> i32 {
        lock_state(&self.shared_state)
            .status_snapshot()
            .runtime
            .window_height
            .unwrap_or(800)
            .max(1)
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

    fn surface_under_point(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        for popup in self.popups.iter().rev() {
            let popup_geometry = self.popup_absolute_geometry(popup);
            let hit = pos.x >= popup_geometry.loc.x as f64
                && pos.x < (popup_geometry.loc.x + popup_geometry.size.w) as f64
                && pos.y >= popup_geometry.loc.y as f64
                && pos.y < (popup_geometry.loc.y + popup_geometry.size.h) as f64;
            if hit {
                let local_pos = (
                    pos.x - popup_geometry.loc.x as f64,
                    pos.y - popup_geometry.loc.y as f64,
                )
                    .into();
                return Some((popup.surface.wl_surface().clone(), local_pos));
            }
        }

        let overlay_rect = self.overlay_rect();
        let overlay_hit = pos.x >= overlay_rect.loc.x as f64
            && pos.x < (overlay_rect.loc.x + overlay_rect.size.w) as f64
            && pos.y >= overlay_rect.loc.y as f64
            && pos.y < (overlay_rect.loc.y + overlay_rect.size.h) as f64;
        if overlay_hit {
            if let Some(overlay) = &self.overlay_toplevel {
                let local_pos = (
                    pos.x - overlay_rect.loc.x as f64,
                    pos.y - overlay_rect.loc.y as f64,
                )
                    .into();
                return Some((overlay.wl_surface().clone(), local_pos));
            }
        }
        self.main_toplevel
            .as_ref()
            .map(|main| (main.wl_surface().clone(), pos))
    }

    fn popup_owner_role(&self, popup: &PopupSurface) -> Option<RuntimeSurfaceRole> {
        let parent_id = popup.get_parent_surface().as_ref().map(surface_id)?;
        if self
            .main_toplevel
            .as_ref()
            .map(|main| surface_id(main.wl_surface()) == parent_id)
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::MainApp);
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|overlay| surface_id(overlay.wl_surface()) == parent_id)
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::OverlayNative);
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

    fn popup_absolute_geometry(&self, popup: &ManagedPopup) -> Rectangle<i32, Logical> {
        let local = self.popup_geometry_local(&popup.surface);
        let base = match popup.owner_role {
            RuntimeSurfaceRole::MainApp => Point::from((0, 0)),
            RuntimeSurfaceRole::OverlayNative => self.overlay_rect().loc,
        };
        Rectangle::new(
            (base.x + local.loc.x, base.y + local.loc.y).into(),
            local.size,
        )
    }

    fn handle_surface_identity_update(&mut self, surface: &ToplevelSurface) {
        let updated_id = surface_id(surface.wl_surface());
        if let Some(idx) = self
            .pending_toplevels
            .iter()
            .position(|pending| surface_id(pending.wl_surface()) == updated_id)
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
    }

    fn role_for_surface_id(&self, id: u32) -> Option<RuntimeSurfaceRole> {
        if self
            .main_toplevel
            .as_ref()
            .map(|main| surface_id(main.wl_surface()) == id)
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::MainApp);
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|overlay| surface_id(overlay.wl_surface()) == id)
            .unwrap_or(false)
        {
            return Some(RuntimeSurfaceRole::OverlayNative);
        }
        for popup in &self.popups {
            if surface_id(popup.surface.wl_surface()) == id {
                return Some(popup.owner_role);
            }
        }
        None
    }

    fn configure_toplevel_for_role(&self, surface: &ToplevelSurface, role: RuntimeSurfaceRole) {
        let status = lock_state(&self.shared_state).status_snapshot();
        let output_w = status.runtime.window_width.unwrap_or(1280).max(1);
        let output_h = status.runtime.window_height.unwrap_or(800).max(1);

        surface.with_pending_state(|pending| {
            pending.states.set(xdg_toplevel::State::Activated);
            match role {
                RuntimeSurfaceRole::MainApp => {
                    pending.states.set(xdg_toplevel::State::Fullscreen);
                    pending.size = Some((output_w, output_h).into());
                }
                RuntimeSurfaceRole::OverlayNative => {
                    let overlay_rect = self.overlay_rect();
                    pending.size = Some((overlay_rect.size.w, overlay_rect.size.h).into());
                }
            }
        });
        let _ = surface.send_pending_configure();
    }

    fn reconfigure_roles(&mut self, width: i32, height: i32) {
        if let Some(main) = &self.main_toplevel {
            self.configure_toplevel_for_role(main, RuntimeSurfaceRole::MainApp);
        }
        if let Some(overlay) = &self.overlay_toplevel {
            self.configure_toplevel_for_role(overlay, RuntimeSurfaceRole::OverlayNative);
        }
        let mut state = lock_state(&self.shared_state);
        state.mark_runtime_resize(width, height);
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
        self.enforce_overlay_binding_policy();
        if self
            .main_toplevel
            .as_ref()
            .map(|surface| !surface.alive())
            .unwrap_or(false)
        {
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
        self.popups.retain(|popup| popup.surface.alive());
        self.promote_pending_toplevels();
        self.sync_runtime_status_with_roles();
    }

    fn collect_render_elements(
        &self,
        renderer: &mut GlesRenderer,
        _output_width: i32,
        _output_height: i32,
    ) -> RenderElementCapture {
        let mut capture = RenderElementCapture::default();

        if let Some(main) = &self.main_toplevel {
            if let Err(err) = import_surface_tree(renderer, main.wl_surface()) {
                eprintln!(
                    "host renderer could not import main surface tree: {err:#?}",
                    err = err
                );
            }
            let elements = render_elements_from_surface_tree(
                renderer,
                main.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            capture.push(RenderElementSource::Main, elements);
        }

        for popup in &self.popups {
            if popup.owner_role != RuntimeSurfaceRole::MainApp {
                continue;
            }
            let popup_geo = self.popup_absolute_geometry(popup);
            if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                eprintln!(
                    "host renderer could not import main popup surface tree: {err:#?}",
                    err = err
                );
            }
            let elements = render_elements_from_surface_tree(
                renderer,
                popup.surface.wl_surface(),
                (popup_geo.loc.x, popup_geo.loc.y),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            capture.push(RenderElementSource::MainPopup, elements);
        }

        if let Some(overlay) = &self.overlay_toplevel {
            let overlay_rect = self.overlay_rect();
            if let Err(err) = import_surface_tree(renderer, overlay.wl_surface()) {
                eprintln!(
                    "host renderer could not import overlay surface tree: {err:#?}",
                    err = err
                );
            }
            let elements = render_elements_from_surface_tree(
                renderer,
                overlay.wl_surface(),
                (overlay_rect.loc.x, overlay_rect.loc.y),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            capture.push(RenderElementSource::Overlay, elements);
        }

        for popup in &self.popups {
            if popup.owner_role != RuntimeSurfaceRole::OverlayNative {
                continue;
            }
            let popup_geo = self.popup_absolute_geometry(popup);
            if let Err(err) = import_surface_tree(renderer, popup.surface.wl_surface()) {
                eprintln!(
                    "host renderer could not import overlay popup surface tree: {err:#?}",
                    err = err
                );
            }
            let elements = render_elements_from_surface_tree(
                renderer,
                popup.surface.wl_surface(),
                (popup_geo.loc.x, popup_geo.loc.y),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            capture.push(RenderElementSource::OverlayPopup, elements);
        }

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
        let mut elements = render_elements_from_surface_tree(
            renderer,
            overlay.wl_surface(),
            (0, 0),
            1.0,
            1.0,
            Kind::Unspecified,
        );

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
        for popup in &self.popups {
            send_frames_surface_tree(popup.surface.wl_surface(), elapsed_ms);
        }
    }

    fn overlay_binding_expected(&self) -> bool {
        lock_state(&self.shared_state).runtime_overlay_binding_expected()
    }

    fn expected_overlay_client_pid(&self) -> Option<u32> {
        lock_state(&self.shared_state)
            .runtime_expected_overlay_binding()
            .map(|(_pane_id, pid)| pid)
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

    fn bridge_overlay_surface_detached(&self, client_pid: u32) {
        let mut state = lock_state(&self.shared_state);
        let _ = state.runtime_mark_overlay_surface_detached_for_pid(client_pid);
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

        let id = surface_id(surface);
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
                self.host_surface_buffers.insert(
                    id,
                    SurfaceBufferSnapshot {
                        buffer,
                        kind,
                        size,
                        dmabuf,
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
        self.host_surface_buffers.remove(&surface_id(surface));
    }

    fn host_scene_surfaces(&self, output_w: i32, output_h: i32) -> Vec<HostSceneSurface> {
        let mut surfaces = Vec::new();
        if let Some(main) = &self.main_toplevel {
            let main_rect = Rectangle::new((0, 0).into(), (output_w, output_h).into());
            self.collect_surface_tree_surfaces(main.wl_surface(), main_rect, &mut surfaces);
        }

        if let Some(overlay) = &self.overlay_toplevel {
            let overlay_rect = self.overlay_rect();
            self.collect_surface_tree_surfaces(overlay.wl_surface(), overlay_rect, &mut surfaces);
        }

        for popup in &self.popups {
            if let Some(snapshot) = self
                .host_surface_buffers
                .get(&surface_id(popup.surface.wl_surface()))
            {
                surfaces.push(HostSceneSurface {
                    buffer: snapshot.buffer.clone(),
                    kind: snapshot.kind,
                    target: self.popup_absolute_geometry(popup),
                    dmabuf: snapshot.dmabuf,
                });
            }
        }

        surfaces
    }

    fn collect_surface_tree_surfaces(
        &self,
        surface: &WlSurface,
        base_rect: Rectangle<i32, Logical>,
        surfaces: &mut Vec<HostSceneSurface>,
    ) {
        with_surface_tree_downward(
            surface,
            base_rect.loc,
            |_surface, data, &offset| {
                let location = data
                    .cached_state
                    .get::<SubsurfaceCachedState>()
                    .current()
                    .location;
                let next_offset = Point::new(offset.x + location.x, offset.y + location.y);
                TraversalAction::DoChildren(next_offset)
            },
            |_surface, _, &offset| {
                if let Some(snapshot) = self.host_surface_buffers.get(&surface_id(surface)) {
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
                            target: Rectangle::new(offset, size),
                            dmabuf: snapshot.dmabuf,
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
        for surface in self.host_scene_surfaces(output_w, output_h) {
            stats.attempted_surfaces = stats.attempted_surfaces.saturating_add(1);
            match surface.kind {
                SurfaceBufferKind::Shm => {
                    if blit_shm_surface(
                        &surface.buffer,
                        surface.target,
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
        self.capture_surface_buffer_commit(surface);
        on_commit_buffer_handler::<Self>(surface);
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
            let target = self.popup_target_rect(owner_role);
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
        let destroyed_id = surface_id(surface.wl_surface());
        self.drop_surface_buffer(surface.wl_surface());
        self.pending_toplevels
            .retain(|pending| surface_id(pending.wl_surface()) != destroyed_id);
        if self
            .main_toplevel
            .as_ref()
            .map(|item| surface_id(item.wl_surface()) == destroyed_id)
            .unwrap_or(false)
        {
            self.main_toplevel = None;
        }
        if self
            .overlay_toplevel
            .as_ref()
            .map(|item| surface_id(item.wl_surface()) == destroyed_id)
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
        let mut removed_popup_ids = Vec::new();
        self.popups.retain(|popup| {
            let keep =
                popup.surface.get_parent_surface().as_ref().map(surface_id) != Some(destroyed_id);
            if !keep {
                removed_popup_ids.push(surface_id(popup.surface.wl_surface()));
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
        let destroyed_id = surface_id(surface.wl_surface());
        self.drop_surface_buffer(surface.wl_surface());
        self.popups
            .retain(|popup| surface_id(popup.surface.wl_surface()) != destroyed_id);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.handle_surface_identity_update(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.handle_surface_identity_update(&surface);
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
        let target = focused
            .map(surface_id)
            .and_then(|id| self.role_for_surface_id(id))
            .map(|role| match role {
                RuntimeSurfaceRole::MainApp => RuntimeFocusTarget::MainApp,
                RuntimeSurfaceRole::OverlayNative => RuntimeFocusTarget::OverlayNative,
            });
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
    target: &mut [u8],
    target_stride: usize,
    output_w: i32,
    output_h: i32,
) -> bool {
    if target_rect.size.w <= 0 || target_rect.size.h <= 0 || output_w <= 0 || output_h <= 0 {
        return false;
    }

    let clipped =
        target_rect.intersection(Rectangle::new((0, 0).into(), (output_w, output_h).into()));
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
            let src_y = rel_y.saturating_mul(src_h) / dst_h;
            let dst_y_i32 = target_rect.loc.y + rel_y as i32;
            if !(0..output_h).contains(&dst_y_i32) {
                continue;
            }
            let dst_row = (dst_y_i32 as usize).saturating_mul(target_stride);
            for rel_x in clipped_left..clipped_right {
                let src_x = rel_x.saturating_mul(src_w) / dst_w;
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

fn lock_state(
    shared_state: &Arc<Mutex<CompositorState>>,
) -> std::sync::MutexGuard<'_, CompositorState> {
    match shared_state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn surface_id(surface: &WlSurface) -> u32 {
    surface.id().protocol_id()
}

fn transform_from_rotation(rotation: OutputRotation) -> Transform {
    match rotation {
        OutputRotation::Deg0 => Transform::Normal,
        OutputRotation::Deg90 => Transform::_90,
        OutputRotation::Deg180 => Transform::_180,
        OutputRotation::Deg270 => Transform::_270,
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

delegate_xdg_shell!(RuntimeWaylandState);
delegate_compositor!(RuntimeWaylandState);
delegate_shm!(RuntimeWaylandState);
delegate_dmabuf!(RuntimeWaylandState);
delegate_seat!(RuntimeWaylandState);

#[cfg(test)]
mod tests {
    use super::{
        AtomicPlaneLayout, DrmFourcc, PlaneSelection, RuntimeWaylandState,
        overlay_scanout_format_supports_alpha, select_atomic_plane_zpos_values,
        select_preferred_scanout_format, select_primary_path,
    };
    use crate::model::ProcessSpec;
    use crate::process_manager::{ProcessController, ProcessExit};
    use crate::state::CompositorState;
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::{Logical, Rectangle};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

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
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state);

        let overlay_rect = wayland_state.overlay_rect();
        assert_eq!(overlay_rect.loc.x, 944);
        assert_eq!(overlay_rect.loc.y, 16);
        assert_eq!(overlay_rect.size.w, 960);
        assert_eq!(overlay_rect.size.h, 540);

        let layout = AtomicPlaneLayout::from_overlay_rect(overlay_rect)
            .expect("positive overlay rect should map to atomic plane layout");
        assert_eq!(layout.crtc_x, 944);
        assert_eq!(layout.crtc_y, 16);
        assert_eq!(layout.crtc_w, 960);
        assert_eq!(layout.crtc_h, 540);
        assert_eq!(layout.src_w, 960);
        assert_eq!(layout.src_h, 540);
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
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state);

        let overlay_rect = wayland_state.overlay_rect();
        assert_eq!(overlay_rect.loc.x, 0);
        assert_eq!(overlay_rect.loc.y, 16);
        assert_eq!(overlay_rect.size.w, 320);
        assert_eq!(overlay_rect.size.h, 184);

        let layout = AtomicPlaneLayout::from_overlay_rect(overlay_rect)
            .expect("overlay rect should still map to atomic plane layout");
        assert_eq!(layout.crtc_x, 0);
        assert_eq!(layout.crtc_y, 16);
        assert_eq!(layout.crtc_w, 320);
        assert_eq!(layout.crtc_h, 184);
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
        let wayland_state = RuntimeWaylandState::new(display.handle(), shared_state);

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
