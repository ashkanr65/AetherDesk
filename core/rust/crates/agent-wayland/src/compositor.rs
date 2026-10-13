use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc as std_mpsc, Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use ad_stream_protocol::{
    InputMessage, TouchPhase, MODIFIER_CAPS_LOCK, MODIFIER_LOCK_STATE_VALID, MODIFIER_NUM_LOCK,
};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::input::{Axis, ButtonState, KeyState, TouchSlot};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::backend::renderer::{Bind, ImportDma};
use smithay::desktop::{PopupKind, PopupManager, Space, Window};
use smithay::input::keyboard::{FilterResult, KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent as PointerMotionEvent};
use smithay::input::touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{
    generic::Generic, EventLoop, Interest, Mode as CalloopMode, PostAction,
};
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{IsAlive, Logical, Point, Serial, Transform};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, with_states, BufferAssignment, CompositorClientState,
    CompositorHandler, CompositorState as SmithayCompositorState, SurfaceAttributes,
};
use smithay::wayland::dmabuf::{
    get_dmabuf, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    request_data_device_client_selection, set_data_device_focus, set_data_device_selection,
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{with_buffer_contents, ShmHandler, ShmState};
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::X11Surface;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_shell,
};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{error, warn};

use crate::damage::{CapturedFrame, SurfaceBufferKind, SurfaceRegistry, SurfaceSnapshot};

#[derive(Clone, Debug)]
pub struct CompositorConfig {
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: u32,
    pub runtime_dir: PathBuf,
    pub enable_xwayland: bool,
    /// Let the selected host desktop own X11 window management and decorations.
    pub host_window_manager: bool,
}

impl CompositorConfig {
    pub fn from_environment(width: u32, height: u32) -> Result<Self, CompositorError> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or(CompositorError::MissingRuntimeDirectory)?;
        Ok(Self {
            width,
            height,
            refresh_millihz: 60_000,
            runtime_dir,
            enable_xwayland: true,
            host_window_manager: true,
        })
    }

    fn validate(&self) -> Result<(), CompositorError> {
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 {
            return Err(CompositorError::InvalidConfiguration(
                "headless output dimensions are invalid",
            ));
        }
        if !(1_000..=240_000).contains(&self.refresh_millihz) {
            return Err(CompositorError::InvalidConfiguration(
                "refresh rate must be between 1 Hz and 240 Hz",
            ));
        }
        let metadata = std::fs::metadata(&self.runtime_dir)?;
        if !metadata.is_dir() {
            return Err(CompositorError::InvalidRuntimeDirectory(
                "XDG_RUNTIME_DIR is not a directory",
            ));
        }
        if metadata.uid() != smithay::reexports::rustix::process::getuid().as_raw() {
            return Err(CompositorError::InvalidRuntimeDirectory(
                "XDG_RUNTIME_DIR is owned by another user",
            ));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(CompositorError::InvalidRuntimeDirectory(
                "XDG_RUNTIME_DIR must not be accessible by group or other users",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ApplicationSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub working_directory: Option<PathBuf>,
}

impl ApplicationSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            environment: Vec::new(),
            working_directory: None,
        }
    }

    fn validate(&self) -> Result<(), CompositorError> {
        if self.program.as_os_str().is_empty() {
            return Err(CompositorError::InvalidApplication("empty program path"));
        }
        if self
            .environment
            .iter()
            .any(|(name, _)| name.as_encoded_bytes().contains(&b'='))
        {
            return Err(CompositorError::InvalidApplication(
                "environment variable names may not contain '='",
            ));
        }
        Ok(())
    }
}

enum CompositorCommand {
    Launch(ApplicationSpec),
    Resize {
        width: u32,
        height: u32,
    },
    Input(InputMessage),
    Clipboard(ClipboardSelection),
    KeyboardLayout {
        layout: String,
        variant: String,
    },
    MonitorLayout {
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
    },
    Stop,
}

#[derive(Clone, Debug)]
pub struct ClipboardSelection {
    values: Arc<HashMap<String, Vec<u8>>>,
}

impl ClipboardSelection {
    pub fn new(mime_type: String, data: Vec<u8>) -> Result<Self, CompositorError> {
        if !matches!(
            mime_type.as_str(),
            "text/plain" | "text/plain;charset=utf-8" | "text/html" | "image/png" | "image/bmp"
        ) || data.len() > 10 * 1024 * 1024
            || (mime_type == "image/png"
                && !data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]))
            || (mime_type == "image/bmp" && !data.starts_with(b"BM"))
        {
            return Err(CompositorError::InvalidClipboard);
        }
        Ok(Self {
            values: Arc::new(HashMap::from([(mime_type, data)])),
        })
    }

    pub(crate) fn data(&self, mime_type: &str) -> Option<Vec<u8>> {
        self.values.get(mime_type).cloned()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedClipboard {
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum CompositorEvent {
    Frame(Arc<CapturedFrame>),
    Clipboard(CapturedClipboard),
}

pub struct HeadlessCompositor {
    socket_name: OsString,
    commands: std_mpsc::Sender<CompositorCommand>,
    frames: watch::Receiver<Option<Arc<CapturedFrame>>>,
    clipboards: mpsc::Receiver<CapturedClipboard>,
    surfaces: Arc<RwLock<Vec<SurfaceSnapshot>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HeadlessCompositor {
    pub fn spawn(config: CompositorConfig) -> Result<Self, CompositorError> {
        config.validate()?;
        let (commands, command_rx) = std_mpsc::channel();
        let (frame_tx, frames) = watch::channel(None);
        let (clipboard_tx, clipboards) = mpsc::channel(8);
        let surfaces = Arc::new(RwLock::new(Vec::new()));
        let surface_output = surfaces.clone();
        let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);

        let thread = thread::Builder::new()
            .name("aetherdesk-wayland".into())
            .spawn(move || {
                if let Err(error) = run_compositor(
                    config,
                    command_rx,
                    frame_tx,
                    clipboard_tx,
                    surface_output,
                    &ready_tx,
                ) {
                    let message = error.to_string();
                    let _ = ready_tx.send(Err(error));
                    tracing::error!(error = %message, "headless compositor stopped");
                }
            })?;
        let socket_name = match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result?,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                return Err(CompositorError::StartupTimeout);
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CompositorError::Stopped);
            }
        };
        Ok(Self {
            socket_name,
            commands,
            frames,
            clipboards,
            surfaces,
            thread: Some(thread),
        })
    }

    pub fn socket_name(&self) -> &OsStr {
        &self.socket_name
    }

    pub fn launch(&self, application: ApplicationSpec) -> Result<(), CompositorError> {
        application.validate()?;
        self.commands
            .send(CompositorCommand::Launch(application))
            .map_err(|_| CompositorError::Stopped)
    }

    pub fn resize(&self, width: u32, height: u32) -> Result<(), CompositorError> {
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            return Err(CompositorError::InvalidConfiguration(
                "headless output dimensions are invalid",
            ));
        }
        self.commands
            .send(CompositorCommand::Resize { width, height })
            .map_err(|_| CompositorError::Stopped)
    }

    pub fn inject_input(&self, input: InputMessage) -> Result<(), CompositorError> {
        self.commands
            .send(CompositorCommand::Input(input))
            .map_err(|_| CompositorError::Stopped)
    }

    pub fn set_keyboard_layout(
        &self,
        layout: String,
        variant: String,
    ) -> Result<(), CompositorError> {
        if !ad_stream_protocol::valid_keyboard_layout(&layout, &variant) {
            return Err(CompositorError::Input("invalid keyboard layout"));
        }
        self.commands
            .send(CompositorCommand::KeyboardLayout { layout, variant })
            .map_err(|_| CompositorError::Stopped)
    }

    pub fn set_monitors(
        &self,
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
    ) -> Result<(), CompositorError> {
        if !ad_stream_protocol::valid_monitor_layout(width, height, &monitors) {
            return Err(CompositorError::Input("invalid monitor layout"));
        }
        self.commands
            .send(CompositorCommand::MonitorLayout {
                width,
                height,
                monitors,
            })
            .map_err(|_| CompositorError::Stopped)
    }

    pub fn set_clipboard(&self, selection: ClipboardSelection) -> Result<(), CompositorError> {
        self.commands
            .send(CompositorCommand::Clipboard(selection))
            .map_err(|_| CompositorError::Stopped)
    }

    pub async fn next_frame(&mut self) -> Result<Arc<CapturedFrame>, CompositorError> {
        self.frames
            .changed()
            .await
            .map_err(|_| CompositorError::Stopped)?;
        self.frames
            .borrow_and_update()
            .clone()
            .ok_or(CompositorError::Stopped)
    }

    pub async fn next_clipboard(&mut self) -> Result<CapturedClipboard, CompositorError> {
        self.clipboards.recv().await.ok_or(CompositorError::Stopped)
    }

    pub async fn next_event(&mut self) -> Result<CompositorEvent, CompositorError> {
        tokio::select! {
            changed = self.frames.changed() => {
                changed.map_err(|_| CompositorError::Stopped)?;
                self.frames.borrow_and_update().clone().map(CompositorEvent::Frame)
            },
            clipboard = self.clipboards.recv() => clipboard.map(CompositorEvent::Clipboard),
        }
        .ok_or(CompositorError::Stopped)
    }

    pub fn surfaces(&self) -> Vec<SurfaceSnapshot> {
        self.surfaces
            .read()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub fn shutdown(&mut self) {
        let _ = self.commands.send(CompositorCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for HeadlessCompositor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) struct AgentCompositorState {
    pub(crate) display_handle: DisplayHandle,
    pub(crate) loop_handle: smithay::reexports::calloop::LoopHandle<'static, Self>,
    pub(crate) compositor_state: SmithayCompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) xwayland_shell_state: XWaylandShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) dmabuf_state: DmabufState,
    pub(crate) _dmabuf_global: DmabufGlobal,
    pub(crate) _output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) popups: PopupManager,
    pub(crate) _seat: Seat<Self>,
    pub(crate) space: Space<Window>,
    pub(crate) renderer: PixmanRenderer,
    pub(crate) output: Output,
    pub(crate) target: Image<'static, 'static>,
    pub(crate) output_damage: OutputDamageTracker,
    pub(crate) start_time: Instant,
    pub(crate) socket_name: OsString,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) refresh_millihz: u32,
    pub(crate) registry: SurfaceRegistry,
    pub(crate) surface_output: Arc<RwLock<Vec<SurfaceSnapshot>>>,
    pub(crate) frame_tx: watch::Sender<Option<Arc<CapturedFrame>>>,
    pub(crate) clipboard_tx: mpsc::Sender<CapturedClipboard>,
    pub(crate) frame_id: u64,
    target_valid: bool,
    command_rx: std_mpsc::Receiver<CompositorCommand>,
    pub(crate) applications: Vec<Child>,
    pub(crate) pending_applications: Vec<ApplicationSpec>,
    pub(crate) xdisplay: Option<u32>,
    pub(crate) xwm: Option<smithay::xwayland::X11Wm>,
    pub(crate) running: bool,
    pub(crate) enable_xwayland: bool,
    pub(crate) host_window_manager: bool,
    pub(crate) host_desktop: Option<crate::host_desktop::HostDesktop>,
    pub(crate) window_offset: i32,
    remote_modifiers: u16,
    pub(crate) clipboard_selection: Option<ClipboardSelection>,
}

fn run_compositor(
    config: CompositorConfig,
    command_rx: std_mpsc::Receiver<CompositorCommand>,
    frame_tx: watch::Sender<Option<Arc<CapturedFrame>>>,
    clipboard_tx: mpsc::Sender<CapturedClipboard>,
    surface_output: Arc<RwLock<Vec<SurfaceSnapshot>>>,
    ready_tx: &std_mpsc::SyncSender<Result<OsString, CompositorError>>,
) -> Result<(), CompositorError> {
    std::env::set_var("XDG_RUNTIME_DIR", &config.runtime_dir);
    let mut event_loop: EventLoop<'static, AgentCompositorState> = EventLoop::try_new()?;
    let display: Display<AgentCompositorState> =
        Display::new().map_err(|error| CompositorError::WaylandInit(error.to_string()))?;
    let mut state = AgentCompositorState::new(
        &mut event_loop,
        display,
        config,
        command_rx,
        frame_tx,
        clipboard_tx,
        surface_output,
    )?;
    let socket_name = state.socket_name.clone();
    if state.xwayland_enabled() {
        if state.host_window_manager {
            state.start_host_desktop()?;
        } else {
            state.start_xwayland()?;
        }
    }
    let _ = ready_tx.send(Ok(socket_name));

    let frame_interval = Duration::from_secs_f64(1000.0 / f64::from(state.refresh_millihz));
    let mut next_frame = Instant::now();
    while state.running {
        event_loop.dispatch(Some(Duration::from_millis(4)), &mut state)?;
        state.process_commands()?;
        if Instant::now() >= next_frame {
            state.render_frame()?;
            // Never catch up by rendering a burst of outdated frames.
            next_frame = Instant::now() + frame_interval;
        }
        state.reap_applications();
        state.space.refresh();
        state.popups.cleanup();
        state.display_handle.flush_clients()?;
    }
    state.terminate_applications();
    Ok(())
}

impl AgentCompositorState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        event_loop: &mut EventLoop<'static, AgentCompositorState>,
        display: Display<Self>,
        config: CompositorConfig,
        command_rx: std_mpsc::Receiver<CompositorCommand>,
        frame_tx: watch::Sender<Option<Arc<CapturedFrame>>>,
        clipboard_tx: mpsc::Sender<CapturedClipboard>,
        surface_output: Arc<RwLock<Vec<SurfaceSnapshot>>>,
    ) -> Result<Self, CompositorError> {
        let display_handle = display.handle();
        let compositor_state = SmithayCompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let mut seat = seat_state.new_wl_seat(&display_handle, "aetherdesk");
        seat.add_keyboard(Default::default(), 250, 30)?;
        seat.add_pointer();
        seat.add_touch();

        let renderer = PixmanRenderer::new()?;
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global =
            dmabuf_state.create_global::<Self>(&display_handle, renderer.dmabuf_formats());
        let output = create_output(
            &display_handle,
            config.width,
            config.height,
            config.refresh_millihz,
        );
        let mut space = Space::default();
        space.map_output(&output, (0, 0));
        let output_damage = OutputDamageTracker::from_output(&output);
        let target = create_target(config.width, config.height)?;

        let listening_socket = ListeningSocketSource::new_auto()
            .map_err(|error| CompositorError::Socket(error.to_string()))?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();
        loop_handle
            .insert_source(listening_socket, |client_stream, _, data| {
                if let Err(error) = data
                    .display_handle
                    .insert_client(client_stream, Arc::new(WaylandClientState::default()))
                {
                    error!(%error, "failed to insert Wayland client");
                }
            })
            .map_err(|error| CompositorError::EventSource(error.to_string()))?;
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, CalloopMode::Level),
                |_, display, data| {
                    // The display remains owned by this calloop source for the
                    // entire compositor thread lifetime.
                    unsafe { display.get_mut().dispatch_clients(data)? };
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| CompositorError::EventSource(error.to_string()))?;

        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
        Ok(Self {
            display_handle,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            xwayland_shell_state,
            shm_state,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            _output_manager_state: output_manager_state,
            seat_state,
            data_device_state,
            popups: PopupManager::default(),
            _seat: seat,
            space,
            renderer,
            output,
            target,
            output_damage,
            start_time: Instant::now(),
            socket_name,
            width: config.width,
            height: config.height,
            refresh_millihz: config.refresh_millihz,
            registry: SurfaceRegistry::default(),
            surface_output,
            frame_tx,
            clipboard_tx,
            frame_id: 0,
            target_valid: false,
            command_rx,
            applications: Vec::new(),
            pending_applications: Vec::new(),
            xdisplay: None,
            xwm: None,
            running: true,
            enable_xwayland: config.enable_xwayland,
            host_window_manager: config.host_window_manager,
            host_desktop: None,
            window_offset: 0,
            remote_modifiers: 0,
            clipboard_selection: None,
        })
    }

    fn xwayland_enabled(&self) -> bool {
        self.enable_xwayland
    }

    fn process_commands(&mut self) -> Result<(), CompositorError> {
        self.poll_host_desktop()?;
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                CompositorCommand::Launch(application) => {
                    self.pending_applications.push(application)
                }
                CompositorCommand::Resize { width, height } => self.resize_output(width, height)?,
                CompositorCommand::Input(input) => self.inject_arpp_input(input)?,
                CompositorCommand::Clipboard(selection) => {
                    let mime_types: Vec<String> = selection.values.keys().cloned().collect();
                    set_data_device_selection(
                        &self.display_handle,
                        &self._seat,
                        mime_types.clone(),
                        selection.clone(),
                    );
                    if let Some(clipboard) = self
                        .host_desktop
                        .as_ref()
                        .and_then(|desktop| desktop.clipboard.as_ref())
                    {
                        clipboard.set(selection.clone());
                    }
                    self.clipboard_selection = Some(selection);
                    if let Some(xwm) = self.xwm.as_mut() {
                        xwm.new_selection(SelectionTarget::Clipboard, Some(mime_types))
                            .map_err(|error| CompositorError::XWayland(error.to_string()))?;
                    }
                }
                CompositorCommand::KeyboardLayout { layout, variant } => {
                    if let Some(keyboard) = self._seat.get_keyboard() {
                        if let Err(error) = keyboard.set_xkb_config(
                            self,
                            smithay::input::keyboard::XkbConfig {
                                layout: &layout,
                                variant: &variant,
                                ..Default::default()
                            },
                        ) {
                            warn!(%error, %layout, %variant, "host keyboard layout could not be loaded");
                        }
                    }
                }
                CompositorCommand::MonitorLayout {
                    width,
                    height,
                    monitors,
                } => {
                    self.resize_output(width, height)?;
                    if let Some(desktop) = &mut self.host_desktop {
                        desktop.monitors = Some((width, height, monitors.clone()));
                        if let Some(bridge) = &desktop.clipboard {
                            bridge.set_monitors(width, height, monitors);
                        }
                    }
                }
                CompositorCommand::Stop => self.running = false,
            }
        }
        if !self.enable_xwayland || self.xdisplay.is_some() {
            for application in std::mem::take(&mut self.pending_applications) {
                self.applications.push(self.spawn_application(application)?);
            }
        }
        Ok(())
    }

    fn spawn_application(&self, application: ApplicationSpec) -> Result<Child, CompositorError> {
        let mut command = Command::new(&application.program);
        command.args(&application.arguments);
        // Keep the desktop and its child calculations in a group owned by
        // this session, so explicit termination reaches more than its launcher.
        command.process_group(0);
        command.env_clear();
        for name in [
            "HOME",
            "USER",
            "LOGNAME",
            "PATH",
            "LANG",
            "LC_ALL",
            "DBUS_SESSION_BUS_ADDRESS",
            "XAUTHORITY",
            "XDG_CONFIG_HOME",
            "XDG_CONFIG_DIRS",
            "XDG_DATA_HOME",
            "XDG_DATA_DIRS",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env(
            "XDG_RUNTIME_DIR",
            std::env::var_os("XDG_RUNTIME_DIR").unwrap(),
        );
        command.env("WAYLAND_DISPLAY", &self.socket_name);
        if let Some(desktop) = &self.host_desktop {
            command.env("XAUTHORITY", &desktop.authority);
        }
        if let Some(display) = self.xdisplay {
            command.env("DISPLAY", format!(":{display}"));
        }
        for (name, value) in application.environment {
            command.env(name, value);
        }
        if let Some(directory) = application.working_directory {
            command.current_dir(directory);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Let a user service manager capture application diagnostics. A
            // piped stream without a dedicated drain task can fill and block
            // the child indefinitely.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(CompositorError::ApplicationSpawn)
    }

    fn inject_arpp_input(&mut self, input: InputMessage) -> Result<(), CompositorError> {
        let time = self.start_time.elapsed().as_millis() as u32;
        match input {
            InputMessage::Keyboard {
                scan_code,
                pressed,
                modifiers,
                repeat: _,
            } => {
                self.sync_wayland_modifiers(modifiers, time)?;
                // Lock synchronization already performed the requested toggle.
                if modifiers & MODIFIER_LOCK_STATE_VALID != 0 && matches!(scan_code, 58 | 69) {
                    return Ok(());
                }
                let keyboard = self
                    ._seat
                    .get_keyboard()
                    .ok_or(CompositorError::Input("Wayland keyboard is unavailable"))?;
                let state = if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                keyboard.input::<(), _>(
                    self,
                    // ARPP uses evdev codes; XKB reserves the first eight codes.
                    scan_code
                        .checked_add(8)
                        .ok_or(CompositorError::Input("keyboard scan code overflow"))?
                        .into(),
                    state,
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
            InputMessage::PointerMove { x, y, .. } => self.pointer_motion(x, y, time),
            InputMessage::PointerButton {
                button,
                pressed,
                x,
                y,
            } => {
                self.pointer_motion(x, y, time);
                let pointer = self
                    ._seat
                    .get_pointer()
                    .ok_or(CompositorError::Input("Wayland pointer is unavailable"))?;
                let code = match button {
                    1 => 0x110,
                    2 => 0x111,
                    3 => 0x112,
                    4 => 0x113,
                    5 => 0x114,
                    _ => return Err(CompositorError::Input("invalid pointer button")),
                };
                if pressed && !pointer.is_grabbed() {
                    let current = pointer.current_location();
                    let target = self
                        .space
                        .element_under(current)
                        .map(|(window, _)| window.clone());
                    if let Some(window) = target {
                        self.space.raise_element(&window, true);
                        let focus = window
                            .x11_surface()
                            .cloned()
                            .map(KeyboardFocus::X11)
                            .or_else(|| {
                                window
                                    .wl_surface()
                                    .map(|surface| KeyboardFocus::Wayland(surface.into_owned()))
                            });
                        if let (Some(keyboard), Some(surface)) = (self._seat.get_keyboard(), focus)
                        {
                            keyboard.set_focus(
                                self,
                                Some(surface),
                                smithay::utils::SERIAL_COUNTER.next_serial(),
                            );
                        }
                    }
                }
                pointer.button(
                    self,
                    &ButtonEvent {
                        button: code,
                        state: if pressed {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                        serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                        time,
                    },
                );
                pointer.frame(self);
            }
            InputMessage::PointerWheel {
                horizontal,
                vertical,
            } => {
                let pointer = self
                    ._seat
                    .get_pointer()
                    .ok_or(CompositorError::Input("Wayland pointer is unavailable"))?;
                let mut frame = AxisFrame::new(time);
                if horizontal != 0 {
                    frame = frame
                        .value(Axis::Horizontal, f64::from(horizontal) / 8.0)
                        .v120(Axis::Horizontal, horizontal);
                }
                if vertical != 0 {
                    frame = frame
                        .value(Axis::Vertical, f64::from(vertical) / 8.0)
                        .v120(Axis::Vertical, vertical);
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputMessage::Touch {
                contact_id,
                phase,
                x,
                y,
                ..
            } => {
                let touch = self._seat.get_touch().ok_or(CompositorError::Input(
                    "Wayland touch device is unavailable",
                ))?;
                let position = Point::from((
                    f64::from(x.clamp(0, self.width as i32 - 1)),
                    f64::from(y.clamp(0, self.height as i32 - 1)),
                ));
                let slot = TouchSlot::from(Some(contact_id));
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                match phase {
                    TouchPhase::Begin => touch.down(
                        self,
                        self.surface_under(position),
                        &DownEvent {
                            slot,
                            location: position,
                            serial,
                            time,
                        },
                    ),
                    TouchPhase::Update => touch.motion(
                        self,
                        self.surface_under(position),
                        &TouchMotionEvent {
                            slot,
                            location: position,
                            time,
                        },
                    ),
                    TouchPhase::End | TouchPhase::Cancel => {
                        touch.up(self, &UpEvent { slot, serial, time })
                    }
                }
                touch.frame(self);
            }
            InputMessage::UnicodeText { .. } => {
                return Err(CompositorError::Input(
                    "Unicode text requires the Wayland text-input protocol",
                ));
            }
        }
        Ok(())
    }

    fn sync_wayland_modifiers(&mut self, requested: u16, time: u32) -> Result<(), CompositorError> {
        let keyboard = self
            ._seat
            .get_keyboard()
            .ok_or(CompositorError::Input("Wayland keyboard is unavailable"))?;
        for (mask, code) in [(1_u16, 42_u32), (2, 29), (4, 56), (8, 125)] {
            let previous = self.remote_modifiers & mask != 0;
            let next = requested & mask != 0;
            if previous != next {
                keyboard.input::<(), _>(
                    self,
                    (code + 8).into(),
                    if next {
                        KeyState::Pressed
                    } else {
                        KeyState::Released
                    },
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
        }
        if requested & MODIFIER_LOCK_STATE_VALID != 0 {
            let current = keyboard.modifier_state();
            for (mask, code, enabled) in [
                (MODIFIER_NUM_LOCK, 69_u32, current.num_lock),
                (MODIFIER_CAPS_LOCK, 58_u32, current.caps_lock),
            ] {
                if enabled != (requested & mask != 0) {
                    for state in [KeyState::Pressed, KeyState::Released] {
                        keyboard.input::<(), _>(
                            self,
                            (code + 8).into(),
                            state,
                            smithay::utils::SERIAL_COUNTER.next_serial(),
                            time,
                            |_, _, _| FilterResult::Forward,
                        );
                    }
                }
            }
        }
        self.remote_modifiers = requested & 0x0f;
        Ok(())
    }

    fn pointer_motion(&mut self, x: i32, y: i32, time: u32) {
        let position = Point::from((
            f64::from(x.clamp(0, self.width as i32 - 1)),
            f64::from(y.clamp(0, self.height as i32 - 1)),
        ));
        if let Some(pointer) = self._seat.get_pointer() {
            pointer.motion(
                self,
                self.surface_under(position),
                &PointerMotionEvent {
                    location: position,
                    serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    fn resize_output(&mut self, width: u32, height: u32) -> Result<(), CompositorError> {
        self.width = width;
        self.height = height;
        let mode = Mode {
            size: (width as i32, height as i32).into(),
            refresh: self.refresh_millihz as i32,
        };
        self.output
            .change_current_state(Some(mode), None, None, None);
        self.output.set_preferred(mode);
        self.target = create_target(width, height)?;
        self.target_valid = false;
        self.output_damage = OutputDamageTracker::from_output(&self.output);
        if self.host_window_manager {
            for window in self.space.elements() {
                if let Some(surface) = window.toplevel() {
                    self.configure_host_surface(surface);
                }
            }
        }
        Ok(())
    }

    fn render_frame(&mut self) -> Result<(), CompositorError> {
        let mut framebuffer = self.renderer.bind(&mut self.target)?;
        let result = smithay::desktop::space::render_output::<
            _,
            WaylandSurfaceRenderElement<PixmanRenderer>,
            _,
            _,
        >(
            &self.output,
            &mut self.renderer,
            &mut framebuffer,
            1.0,
            usize::from(self.target_valid),
            [&self.space],
            &[],
            &mut self.output_damage,
            [0.035, 0.035, 0.045, 1.0],
        )?;
        self.target_valid = true;
        // Frame callbacks must continue even when no pixels changed.
        self.space.elements().for_each(|window| {
            window.send_frame(
                &self.output,
                self.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            )
        });
        let Some(damage) = result.damage else {
            return Ok(());
        };
        if damage.is_empty() {
            return Ok(());
        }
        let damage = damage
            .iter()
            .filter_map(|region| {
                let x = u32::try_from(region.loc.x).ok()?;
                let y = u32::try_from(region.loc.y).ok()?;
                let width = u32::try_from(region.size.w).ok()?;
                let height = u32::try_from(region.size.h).ok()?;
                Some(ad_stream_protocol::Rectangle {
                    x,
                    y,
                    width,
                    height,
                })
            })
            .collect();
        let stride = self.target.stride();
        let length = stride.saturating_mul(self.target.height());
        // Pixman owns this image on the compositor thread. No client or other
        // thread can mutate the target while this copy is performed.
        let pixels =
            unsafe { std::slice::from_raw_parts(self.target.data().cast::<u8>(), length).to_vec() };
        self.frame_id = self.frame_id.wrapping_add(1);
        let frame = CapturedFrame {
            frame_id: self.frame_id,
            presentation_time_us: self.start_time.elapsed().as_micros() as u64,
            width: self.width,
            height: self.height,
            stride,
            pixels_bgra: pixels,
            damage,
        };
        // A slow encoder takes the newest complete raw image, never old queued images.
        self.frame_tx.send_replace(Some(Arc::new(frame)));
        Ok(())
    }

    fn reap_applications(&mut self) {
        self.applications
            .retain_mut(|child| match child.try_wait() {
                Ok(Some(status)) => {
                    warn!(pid = child.id(), %status, "session application exited");
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    warn!(pid = child.id(), %error, "failed to query session application");
                    false
                }
            });
    }

    fn terminate_applications(&mut self) {
        for child in &mut self.applications {
            // SAFETY: the child's PID is also its private process-group ID,
            // created above. It cannot refer to the agent's or caller's group.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.applications.clear();
    }

    pub(crate) fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(position)
            .and_then(|(window, location)| {
                window
                    .surface_under(
                        position - location.to_f64(),
                        smithay::desktop::WindowSurfaceType::ALL,
                    )
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }
}

fn create_output(
    display_handle: &DisplayHandle,
    width: u32,
    height: u32,
    refresh_millihz: u32,
) -> Output {
    let output = Output::new(
        "AetherDesk-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "AetherDesk".into(),
            model: "ARPP Headless Output".into(),
        },
    );
    let mode = Mode {
        size: (width as i32, height as i32).into(),
        refresh: refresh_millihz as i32,
    };
    output.create_global::<AgentCompositorState>(display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    output
}

fn create_target(width: u32, height: u32) -> Result<Image<'static, 'static>, CompositorError> {
    Image::new(FormatCode::A8R8G8B8, width as usize, height as usize, false)
        .map_err(|_| CompositorError::FramebufferAllocation)
}

fn pending_buffer_kind(surface: &WlSurface) -> Option<SurfaceBufferKind> {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .pending()
            .buffer
            .as_ref()
            .and_then(|assignment| match assignment {
                BufferAssignment::NewBuffer(buffer) => {
                    if get_dmabuf(buffer).is_ok() {
                        Some(SurfaceBufferKind::DmaBuf)
                    } else if with_buffer_contents(buffer, |_, _, _| ()).is_ok() {
                        Some(SurfaceBufferKind::Shm)
                    } else {
                        Some(SurfaceBufferKind::Unknown)
                    }
                }
                BufferAssignment::Removed => None,
            })
    })
}

#[derive(Default)]
pub(crate) struct WaylandClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for WaylandClientState {
    fn initialized(&self, _client_id: ClientId) {}

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl CompositorHandler for AgentCompositorState {
    fn compositor_state(&mut self) -> &mut SmithayCompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Smithay installs its own client data when spawning XWayland.
        if let Some(data) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            &data.compositor_state
        } else {
            &client
                .get_data::<WaylandClientState>()
                .expect("native Wayland clients use WaylandClientState")
                .compositor_state
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        let buffer_kind = pending_buffer_kind(surface);
        on_commit_buffer_handler::<Self>(surface);
        self.registry
            .committed(surface.id().protocol_id(), buffer_kind);
        if let Ok(mut output) = self.surface_output.write() {
            *output = self.registry.snapshots();
        }

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.space.elements().find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == &root)
            }) {
                window.on_commit();
            }
        }
        self.popups.commit(surface);
        if let Some(window) = self.space.elements().find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface)
        }) {
            let configured = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
            });
            if !configured {
                window.toplevel().unwrap().send_configure();
            }
        }
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
        }
    }
}

impl BufferHandler for AgentCompositorState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for AgentCompositorState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for AgentCompositorState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}

impl XdgShellHandler for AgentCompositorState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        if self.host_window_manager {
            self.configure_host_surface(&surface);
            let focus = surface.wl_surface().clone();
            self.space
                .map_element(Window::new_wayland_window(surface), (0, 0), true);
            if let Some(keyboard) = self._seat.get_keyboard() {
                keyboard.set_focus(
                    self,
                    Some(KeyboardFocus::Wayland(focus)),
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                );
            }
            return;
        }
        let window = Window::new_wayland_window(surface);
        let max_x = self.width.saturating_sub(640) as i32;
        let max_y = self.height.saturating_sub(480) as i32;
        let location = (self.window_offset.min(max_x), self.window_offset.min(max_y));
        self.window_offset = (self.window_offset + 32) % 256;
        self.space.map_element(window, location, true);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if self.host_window_manager {
            self.configure_host_surface(&surface);
        } else {
            surface.send_configure();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn move_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
        // Headless sessions use deterministic server-side placement; clients
        // may still position their own subsurfaces and popups.
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
        _edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        surface.send_pending_configure();
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
    }
}

// Preserve the X11 window when assigning keyboard focus: its handler also
// sends SetInputFocus/WM_TAKE_FOCUS before forwarding Wayland key events.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum KeyboardFocus {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::X11(surface) => surface.alive(),
        }
    }
}

impl WaylandFocus for KeyboardFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            Self::X11(surface) => surface.wl_surface().map(Cow::Owned),
        }
    }
}

impl KeyboardTarget<AgentCompositorState> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<AgentCompositorState>,
        data: &mut AgentCompositorState,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
            Self::X11(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
        }
    }

    fn leave(
        &self,
        seat: &Seat<AgentCompositorState>,
        data: &mut AgentCompositorState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::leave(surface, seat, data, serial),
            Self::X11(surface) => KeyboardTarget::leave(surface, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<AgentCompositorState>,
        data: &mut AgentCompositorState,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
            Self::X11(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<AgentCompositorState>,
        data: &mut AgentCompositorState,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::modifiers(surface, seat, data, modifiers, serial)
            }
            Self::X11(surface) => KeyboardTarget::modifiers(surface, seat, data, modifiers, serial),
        }
    }
}

impl SeatHandler for AgentCompositorState {
    type KeyboardFocus = KeyboardFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocus>) {
        let client = focused
            .and_then(WaylandFocus::wl_surface)
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

impl SelectionHandler for AgentCompositorState {
    type SelectionUserData = ClipboardSelection;

    fn new_selection(
        &mut self,
        target: SelectionTarget,
        source: Option<SelectionSource>,
        seat: Seat<Self>,
    ) {
        if target != SelectionTarget::Clipboard {
            return;
        }
        let Some(source) = source else {
            return;
        };
        let Some(mime_type) = preferred_clipboard_mime(&source.mime_types()) else {
            return;
        };
        let Ok((read_fd, write_fd)) = smithay::reexports::rustix::pipe::pipe() else {
            warn!("failed to allocate Wayland clipboard transfer pipe");
            return;
        };
        if let Err(error) = request_data_device_client_selection(&seat, mime_type.clone(), write_fd)
        {
            warn!(%error, "failed to request Wayland clipboard contents");
            return;
        }
        capture_clipboard(read_fd, mime_type, self.clipboard_tx.clone());
    }

    fn send_selection(
        &mut self,
        target: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &Self::SelectionUserData,
    ) {
        if target != SelectionTarget::Clipboard {
            return;
        }
        let Some(data) = user_data.values.get(&mime_type).cloned() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("aetherdesk-clipboard-write".into())
            .spawn(move || {
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(&data);
                let _ = file.flush();
            });
    }
}

pub(crate) fn preferred_clipboard_mime(mime_types: &[String]) -> Option<String> {
    [
        "text/plain;charset=utf-8",
        "text/plain",
        "text/html",
        "image/png",
        "image/bmp",
    ]
    .into_iter()
    .find(|candidate| mime_types.iter().any(|value| value == candidate))
    .map(str::to_owned)
}

pub(crate) fn capture_clipboard(
    read_fd: OwnedFd,
    mime_type: String,
    output: mpsc::Sender<CapturedClipboard>,
) {
    let _ = thread::Builder::new()
        .name("aetherdesk-clipboard-read".into())
        .spawn(move || {
            let mut file = std::fs::File::from(read_fd);
            let mut data = Vec::new();
            if std::io::Read::by_ref(&mut file)
                .take(10 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .is_ok()
                && !data.is_empty()
                && data.len() <= 10 * 1024 * 1024
            {
                let _ = output.blocking_send(CapturedClipboard { mime_type, data });
            }
        });
}

impl DataDeviceHandler for AgentCompositorState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for AgentCompositorState {}
impl ServerDndGrabHandler for AgentCompositorState {}
impl OutputHandler for AgentCompositorState {}

delegate_compositor!(AgentCompositorState);
delegate_shm!(AgentCompositorState);
delegate_dmabuf!(AgentCompositorState);
delegate_xdg_shell!(AgentCompositorState);
delegate_seat!(AgentCompositorState);
delegate_data_device!(AgentCompositorState);
delegate_output!(AgentCompositorState);

#[derive(Debug, Error)]
pub enum CompositorError {
    #[error("invalid compositor configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("XDG_RUNTIME_DIR is not configured")]
    MissingRuntimeDirectory,
    #[error("invalid XDG_RUNTIME_DIR: {0}")]
    InvalidRuntimeDirectory(&'static str),
    #[error("invalid application: {0}")]
    InvalidApplication(&'static str),
    #[error("the compositor did not initialize within ten seconds")]
    StartupTimeout,
    #[error("the compositor has stopped")]
    Stopped,
    #[error("failed to allocate the headless framebuffer")]
    FramebufferAllocation,
    #[error("failed to spawn session application: {0}")]
    ApplicationSpawn(std::io::Error),
    #[error("compositor I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("calloop failed: {0}")]
    Calloop(#[from] smithay::reexports::calloop::Error),
    #[error("Wayland initialization failed: {0}")]
    WaylandInit(String),
    #[error("Wayland socket creation failed: {0}")]
    Socket(String),
    #[error("event source registration failed: {0}")]
    EventSource(String),
    #[error("Wayland dispatch failed: {0}")]
    Wayland(#[from] smithay::reexports::wayland_server::DispatchError),
    #[error("Smithay Pixman rendering failed: {0}")]
    Pixman(#[from] smithay::backend::renderer::pixman::PixmanError),
    #[error("Smithay output rendering failed: {0}")]
    Render(
        #[from]
        smithay::backend::renderer::damage::Error<smithay::backend::renderer::pixman::PixmanError>,
    ),
    #[error("keyboard initialization failed: {0}")]
    Keyboard(#[from] smithay::input::keyboard::Error),
    #[error("input injection failed: {0}")]
    Input(&'static str),
    #[error("clipboard MIME type or payload is invalid")]
    InvalidClipboard,
    #[error("XWayland failed: {0}")]
    XWayland(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn validates_output_limits_before_starting_native_services() {
        let config = CompositorConfig {
            width: 0,
            height: 1080,
            refresh_millihz: 60_000,
            runtime_dir: PathBuf::from("/does/not/matter"),
            enable_xwayland: false,
            host_window_manager: false,
        };
        assert!(matches!(
            config.validate(),
            Err(CompositorError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn validates_remote_clipboard_payloads() {
        assert!(
            ClipboardSelection::new("text/plain;charset=utf-8".into(), b"hello".to_vec()).is_ok()
        );
        assert!(matches!(
            ClipboardSelection::new("application/x-executable".into(), vec![0]),
            Err(CompositorError::InvalidClipboard)
        ));
    }

    #[test]
    fn starts_and_stops_a_user_space_headless_compositor() {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
            width: 320,
            height: 200,
            refresh_millihz: 30_000,
            runtime_dir: runtime.path().to_path_buf(),
            enable_xwayland: false,
            host_window_manager: false,
        })
        .unwrap();
        assert!(compositor
            .socket_name()
            .to_string_lossy()
            .starts_with("wayland-"));
        compositor.shutdown();
    }

    #[tokio::test]
    async fn idle_desktop_does_not_copy_frames_and_slow_consumer_gets_latest_resize() {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
            width: 320,
            height: 200,
            refresh_millihz: 30_000,
            runtime_dir: runtime.path().to_path_buf(),
            enable_xwayland: false,
            host_window_manager: false,
        })
        .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), compositor.next_frame())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.frame_id, 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), compositor.next_frame())
                .await
                .is_err()
        );
        for width in [400, 500, 600] {
            compositor.resize(width, 300).unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        let latest = compositor.next_frame().await.unwrap();
        assert_eq!((latest.width, latest.height), (600, 300));
        assert_eq!(latest.frame_id, 4);
        compositor.shutdown();
    }

    #[test]
    fn starts_xwayland_before_launching_x11_applications() {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let display_file = runtime.path().join("application-display");
        let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
            width: 320,
            height: 200,
            refresh_millihz: 30_000,
            runtime_dir: runtime.path().to_path_buf(),
            enable_xwayland: true,
            host_window_manager: false,
        })
        .unwrap();
        let mut application = ApplicationSpec::new("/bin/sh");
        application.arguments = vec![
            "-c".into(),
            r#"printf '%s' "$DISPLAY" > "$1""#.into(),
            "aetherdesk-xwayland-test".into(),
            display_file.as_os_str().to_owned(),
        ];
        compositor.launch(application).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !display_file.exists()
            && Instant::now() < deadline
            && !compositor.thread.as_ref().unwrap().is_finished()
        {
            thread::sleep(Duration::from_millis(20));
        }
        let display = std::fs::read_to_string(&display_file).unwrap_or_default();
        compositor.shutdown();
        // DISPLAY is assigned only after the real XWayland client and XWM
        // have initialized. The old client-state downcast panicked first.
        assert!(display.starts_with(':'), "XWayland did not become ready");
    }

    #[test]
    fn captures_pixels_from_a_real_x11_window() {
        use smithay::reexports::x11rb::{connection::Connection, protocol::xproto::*};
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let display_file = runtime.path().join("application-display");
        let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
            width: 320,
            height: 200,
            refresh_millihz: 30_000,
            runtime_dir: runtime.path().to_path_buf(),
            enable_xwayland: true,
            host_window_manager: false,
        })
        .unwrap();
        let mut application = ApplicationSpec::new("/bin/sh");
        application.arguments = vec![
            "-c".into(),
            r#"printf '%s' "$DISPLAY" > "$1""#.into(),
            "aetherdesk-capture-test".into(),
            display_file.as_os_str().to_owned(),
        ];
        compositor.launch(application).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !display_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let display = std::fs::read_to_string(display_file).unwrap();
        let (connection, screen_number) =
            smithay::reexports::x11rb::connect(Some(&display)).unwrap();
        let screen = &connection.setup().roots[screen_number];
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                screen.root_depth,
                window,
                screen.root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(0x00ff0000)
                    .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
        connection.flush().unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let captured = executor.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let frame = compositor.next_frame().await.unwrap();
                    if frame
                        .pixels_bgra
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|pixel| pixel[2] > 200 && pixel[1] < 30 && pixel[0] < 30)
                    {
                        break;
                    }
                }
            })
            .await
            .is_ok()
        });
        assert!(
            captured,
            "mapped red X11 window never reached the captured framebuffer"
        );
        for pressed in [true, false] {
            compositor
                .inject_input(InputMessage::PointerButton {
                    button: 1,
                    pressed,
                    x: 10,
                    y: 10,
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while connection.get_input_focus().unwrap().reply().unwrap().focus != window
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            connection.get_input_focus().unwrap().reply().unwrap().focus,
            window,
            "click did not focus the X11 window"
        );
        for (pressed, modifiers) in [(true, 0), (false, 0), (true, 1), (false, 0)] {
            compositor
                .inject_input(InputMessage::Keyboard {
                    scan_code: 30,
                    pressed,
                    modifiers,
                    repeat: false,
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut received = Vec::new();
        while received.len() < 6 && Instant::now() < deadline {
            match connection.poll_for_event().unwrap() {
                Some(smithay::reexports::x11rb::protocol::Event::KeyPress(event)) => {
                    received.push((true, event.detail))
                }
                Some(smithay::reexports::x11rb::protocol::Event::KeyRelease(event)) => {
                    received.push((false, event.detail))
                }
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
        let key_state = |scan_code, modifiers| {
            for pressed in [true, false] {
                compositor
                    .inject_input(InputMessage::Keyboard {
                        scan_code,
                        pressed,
                        modifiers,
                        repeat: false,
                    })
                    .unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(smithay::reexports::x11rb::protocol::Event::KeyPress(event)) =
                    connection.poll_for_event().unwrap()
                {
                    if u32::from(event.detail) == scan_code + 8 {
                        break event.state;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "lock-state key event was not delivered"
                );
                thread::sleep(Duration::from_millis(5));
            }
        };
        let locks = MODIFIER_LOCK_STATE_VALID;
        assert!(key_state(79, locks | MODIFIER_NUM_LOCK).contains(KeyButMask::MOD2));
        assert!(!key_state(79, locks).contains(KeyButMask::MOD2));
        // A physical Num Lock event must not double-toggle the synchronized state.
        for pressed in [true, false] {
            compositor
                .inject_input(InputMessage::Keyboard {
                    scan_code: 69,
                    pressed,
                    modifiers: locks | MODIFIER_NUM_LOCK,
                    repeat: false,
                })
                .unwrap();
        }
        assert!(key_state(79, locks | MODIFIER_NUM_LOCK).contains(KeyButMask::MOD2));
        assert!(key_state(30, locks | MODIFIER_CAPS_LOCK).contains(KeyButMask::LOCK));
        assert!(!key_state(30, locks).contains(KeyButMask::LOCK));
        compositor.shutdown();
        assert_eq!(
            received,
            vec![
                (true, 38),
                (false, 38),
                (true, 50),
                (true, 38),
                (false, 50),
                (false, 38)
            ],
            "evdev KEY_A and Shift must reach X11 as keycodes 38 and 50"
        );
    }
}
