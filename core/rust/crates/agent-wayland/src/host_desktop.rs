//! A complete X11 desktop hosted as one Wayland surface. The host's window
//! manager owns application placement, decorations, fonts, themes and shortcuts.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::compositor::{AgentCompositorState, CompositorError};

pub(crate) struct HostDesktop {
    child: Child,
    terminal_flow_control: Option<crate::terminal_flow_control::TerminalFlowControl>,
    ready: Receiver<Result<u32, String>>,
    started: Instant,
    cookie: [u8; 16],
    pub(crate) clipboard: Option<crate::host_clipboard::HostClipboard>,
    pub(crate) authority: PathBuf,
    pub(crate) monitors: Option<(
        u32,
        u32,
        Vec<ad_stream_protocol::Rectangle>,
        Vec<ad_stream_protocol::MonitorProperties>,
    )>,
}

impl Drop for HostDesktop {
    fn drop(&mut self) {
        // Stop PTY inspection before retiring this desktop's private identity.
        drop(self.terminal_flow_control.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.authority);
    }
}

fn write_authority(file: &mut File, display: &str, cookie: &[u8; 16]) -> std::io::Result<()> {
    // FamilyWild allows local Unix socket clients regardless of hostname. The
    // cookie remains mandatory and the authority file is private to this session.
    file.write_all(&65535u16.to_be_bytes())?;
    for field in [
        b"".as_slice(),
        display.as_bytes(),
        b"MIT-MAGIC-COOKIE-1",
        cookie,
    ] {
        file.write_all(&(field.len() as u16).to_be_bytes())?;
        file.write_all(field)?;
    }
    file.flush()
}

impl AgentCompositorState {
    pub(crate) fn start_host_desktop(&mut self) -> Result<(), CompositorError> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or(CompositorError::MissingRuntimeDirectory)?;
        let authority = runtime.join(format!(
            "{}-{}.Xauthority",
            self.socket_name.to_string_lossy(),
            std::process::id()
        ));
        let mut cookie = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut cookie)?;
        let mut auth = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&authority)?;
        write_authority(&mut auth, "", &cookie)?;
        let spawned = Command::new("Xwayland")
            .args([
                "-nolisten",
                "tcp",
                "-noreset",
                "-displayfd",
                "1",
                "-geometry",
            ])
            .arg(format!("{}x{}", self.width, self.height))
            .arg("-auth")
            .arg(&authority)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(authority);
                return Err(error.into());
            }
        };
        let stdout = child.stdout.take().expect("Xwayland stdout is piped");
        let (sender, ready) = mpsc::channel();
        self.host_desktop = Some(HostDesktop {
            child,
            terminal_flow_control: None,
            ready,
            started: Instant::now(),
            cookie,
            clipboard: None,
            authority,
            monitors: None,
        });
        // Readiness must not block dispatch of Wayland events needed by Xwayland.
        std::thread::Builder::new()
            .name("aetherdesk-x11-ready".into())
            .spawn(move || {
                let mut line = String::new();
                let result = BufReader::new(stdout)
                    .take(64)
                    .read_line(&mut line)
                    .map_err(|error| error.to_string())
                    .and_then(|_| {
                        line.trim()
                            .parse::<u32>()
                            .map_err(|_| "invalid Xwayland display number".into())
                    });
                let _ = sender.send(result);
            })?;
        Ok(())
    }

    pub(crate) fn poll_host_desktop(&mut self) -> Result<(), CompositorError> {
        let Some(desktop) = &mut self.host_desktop else {
            return Ok(());
        };
        if let Some(status) = desktop.child.try_wait()? {
            return Err(CompositorError::XWayland(format!(
                "host desktop server exited: {status}"
            )));
        }
        if self.xdisplay.is_none() {
            match desktop.ready.try_recv() {
                Ok(Ok(display_number)) => {
                    let mut auth = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&desktop.authority)?;
                    write_authority(&mut auth, &display_number.to_string(), &desktop.cookie)?;
                    desktop.terminal_flow_control =
                        Some(crate::terminal_flow_control::TerminalFlowControl::start(
                            display_number,
                            desktop.authority.clone(),
                        )?);
                    desktop.clipboard = Some(crate::host_clipboard::HostClipboard::start(
                        display_number,
                        desktop.cookie,
                        self.clipboard_tx.clone(),
                    )?);
                    if let (Some(clipboard), Some(selection)) =
                        (&desktop.clipboard, &self.clipboard_selection)
                    {
                        clipboard.set(selection.clone());
                    }
                    if let (Some(bridge), Some((width, height, monitors, properties))) =
                        (&desktop.clipboard, &desktop.monitors)
                    {
                        bridge.set_monitors(*width, *height, monitors.clone(), properties.clone());
                    }
                    self.xdisplay = Some(display_number);
                    tracing::info!(
                        display_number,
                        "X11 desktop ready for the host window manager"
                    );
                }
                Ok(Err(error)) => return Err(CompositorError::XWayland(error)),
                Err(TryRecvError::Disconnected) => {
                    return Err(CompositorError::XWayland(
                        "host desktop startup channel closed".into(),
                    ))
                }
                Err(TryRecvError::Empty) if desktop.started.elapsed() > Duration::from_secs(15) => {
                    return Err(CompositorError::XWayland(
                        "host desktop server startup timed out".into(),
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        Ok(())
    }

    pub(crate) fn configure_host_surface(&self, surface: &ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.size = Some((self.width as i32, self.height as i32).into());
            state.states.set(xdg_toplevel::State::Activated);
            state.states.set(xdg_toplevel::State::Fullscreen);
        });
        surface.send_configure();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{
        ApplicationSpec, CompositorConfig, CompositorEvent, HeadlessCompositor,
    };
    use ad_stream_protocol::InputMessage;
    use smithay::reexports::x11rb::{
        connection::Connection,
        protocol::xproto::*,
        rust_connection::{DefaultStream, RustConnection},
        wrapper::ConnectionExt as _,
    };
    use std::os::unix::{fs::PermissionsExt, net::UnixStream};

    #[test]
    fn host_xfwm_draws_decorations_and_handles_input_and_resize() {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let display_file = runtime.path().join("display");
        let authority_file = runtime.path().join("authority-path");
        let mut compositor = HeadlessCompositor::spawn(CompositorConfig {
            width: 640,
            height: 480,
            refresh_millihz: 60_000,
            runtime_dir: runtime.path().to_path_buf(),
            enable_xwayland: true,
            host_window_manager: true,
        })
        .unwrap();
        let mut application = ApplicationSpec::new("/bin/sh");
        application.arguments = vec!["-c".into(), "printf '%s' \"$DISPLAY\" > \"$1\"; printf '%s' \"$XAUTHORITY\" > \"$2\"; exec env -u WAYLAND_DISPLAY GDK_BACKEND=x11 dbus-run-session -- xfwm4".into(), "host-desktop-test".into(), display_file.as_os_str().into(), authority_file.as_os_str().into()];
        compositor.launch(application).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !authority_file.exists() {
            assert!(
                Instant::now() < deadline,
                "host desktop did not become ready"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let display = std::fs::read_to_string(display_file).unwrap();
        let auth_path = std::fs::read_to_string(authority_file).unwrap();
        let auth = std::fs::read(auth_path).unwrap();
        let cookie = auth[auth.len() - 16..].to_vec();
        let socket = format!("/tmp/.X11-unix/X{}", display.trim_start_matches(':'));
        let connect = || {
            DefaultStream::from_unix_stream(UnixStream::connect(&socket).unwrap())
                .unwrap()
                .0
        };
        assert!(
            RustConnection::connect_to_stream(connect(), 0).is_err(),
            "X11 display must require authentication"
        );
        let connection = RustConnection::connect_to_stream_with_auth_info(
            connect(),
            0,
            b"MIT-MAGIC-COOKIE-1".to_vec(),
            cookie.clone(),
        )
        .unwrap();
        let screen = &connection.setup().roots[0];
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let wm_check = atom(b"_NET_SUPPORTING_WM_CHECK");
        loop {
            let wm = connection
                .get_property(false, screen.root, wm_check, AtomEnum::WINDOW, 0, 1)
                .unwrap()
                .reply()
                .unwrap();
            if wm
                .value32()
                .is_some_and(|mut values| values.next().is_some())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host xfwm4 failed to become the window manager"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                screen.root_depth,
                window,
                screen.root,
                80,
                90,
                300,
                160,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(0xff0000)
                    .event_mask(
                        EventMask::KEY_PRESS | EventMask::KEY_RELEASE | EventMask::STRUCTURE_NOTIFY,
                    ),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Host theme - original title bar",
            )
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
        connection.flush().unwrap();
        let frame_extents = atom(b"_NET_FRAME_EXTENTS");
        loop {
            let extents = connection
                .get_property(false, window, frame_extents, AtomEnum::CARDINAL, 0, 4)
                .unwrap()
                .reply()
                .unwrap();
            let borders: Vec<_> = extents.value32().into_iter().flatten().collect();
            if borders.len() == 4 && borders[2] > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "xfwm4 did not decorate the application"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let parent = connection
            .query_tree(window)
            .unwrap()
            .reply()
            .unwrap()
            .parent;
        // The independent view keeps this same client, display and WM frame.
        // Exercise real Composite capture while another window covers it.
        crate::window_mirror::verify_existing_window(
            RustConnection::connect_to_stream_with_auth_info(
                connect(),
                0,
                b"MIT-MAGIC-COOKIE-1".to_vec(),
                cookie.clone(),
            )
            .unwrap(),
            &connection,
            window,
        );
        assert_ne!(
            parent, screen.root,
            "application must be reparented inside the host's decoration frame"
        );
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        executor.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let CompositorEvent::Frame(frame) =
                    tokio::time::timeout_at(deadline, compositor.next_event())
                        .await
                        .unwrap()
                        .unwrap()
                {
                    if frame
                        .pixels_bgra
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[2] > 200 && p[1] < 30 && p[0] < 30)
                    {
                        // Optional visual inspection artifact, outside production/session data.
                        if let Some(path) = std::env::var_os("AETHERDESK_TEST_SCREENSHOT") {
                            let mut output = File::create(path).unwrap();
                            write!(output, "P6\n{} {}\n255\n", frame.width, frame.height).unwrap();
                            for row in frame
                                .pixels_bgra
                                .chunks(frame.stride)
                                .take(frame.height as usize)
                            {
                                for p in row.as_chunks::<4>().0.iter().take(frame.width as usize) {
                                    output.write_all(&[p[2], p[1], p[0]]).unwrap();
                                }
                            }
                        }
                        break;
                    }
                }
            }
        });
        let location = connection
            .translate_coordinates(window, screen.root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        for pressed in [true, false] {
            compositor
                .inject_input(InputMessage::PointerButton {
                    button: 1,
                    pressed,
                    x: i32::from(location.dst_x) + 20,
                    y: i32::from(location.dst_y) + 30,
                })
                .unwrap();
        }
        for pressed in [true, false] {
            compositor
                .inject_input(InputMessage::Keyboard {
                    scan_code: 30,
                    pressed,
                    modifiers: 0,
                    repeat: false,
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(smithay::reexports::x11rb::protocol::Event::KeyPress(event)) =
                connection.poll_for_event().unwrap()
            {
                assert_eq!(event.detail, 38);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "keyboard did not reach the host-managed application"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // The actual host WM receives title-bar drags, rather than AetherDesk
        // drawing a header and interpreting its buttons.
        let start = connection
            .translate_coordinates(window, screen.root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        let x = i32::from(start.dst_x) + 80;
        let y = i32::from(start.dst_y) - 10;
        compositor
            .inject_input(InputMessage::PointerButton {
                button: 1,
                pressed: true,
                x,
                y,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        compositor
            .inject_input(InputMessage::PointerMove {
                x: x + 20,
                y: y + 10,
                monitor: 0,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        compositor
            .inject_input(InputMessage::PointerButton {
                button: 1,
                pressed: false,
                x: x + 20,
                y: y + 10,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let moved = connection
                .translate_coordinates(window, screen.root, 0, 0)
                .unwrap()
                .reply()
                .unwrap();
            if moved.dst_x == start.dst_x + 20 && moved.dst_y == start.dst_y + 10 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host title-bar drag did not move the application: start={start:?}, moved={moved:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Exercise both directions, including INCR transfers larger than an X11 request.
        let (peer_tx, mut peer_rx) = tokio::sync::mpsc::channel(8);
        let peer = crate::host_clipboard::HostClipboard::start(
            display.trim_start_matches(':').parse().unwrap(),
            cookie.try_into().unwrap(),
            peer_tx,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        for size in [32, 100_000, 2_000_000] {
            let payload = vec![b'a'; size];
            compositor
                .set_clipboard(
                    crate::compositor::ClipboardSelection::new(
                        "text/plain;charset=utf-8".into(),
                        payload.clone(),
                    )
                    .unwrap(),
                )
                .unwrap();
            let received = executor.block_on(async {
                tokio::time::timeout(Duration::from_secs(8), peer_rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
            });
            assert_eq!(received.data, payload);
            let payload = vec![b'b'; size + 1];
            peer.set(
                crate::compositor::ClipboardSelection::new(
                    "text/plain;charset=utf-8".into(),
                    payload.clone(),
                )
                .unwrap(),
            );
            executor.block_on(async {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                loop {
                    if let CompositorEvent::Clipboard(received) =
                        tokio::time::timeout_at(deadline, compositor.next_event())
                            .await
                            .unwrap()
                            .unwrap()
                    {
                        assert_eq!(received.data, payload);
                        break;
                    }
                }
            });
        }
        // The Windows input layout changes the X11 keymap, not just the UI language.
        compositor
            .set_keyboard_layout("es".into(), "".into())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let mapping = connection
                .get_keyboard_mapping(47, 1)
                .unwrap()
                .reply()
                .unwrap();
            if mapping.keysyms.contains(&0x00f1) {
                break;
            } // Spanish ñ on the physical semicolon key.
            assert!(
                Instant::now() < deadline,
                "Spanish keyboard map did not reach Xwayland"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let accent = connection
            .get_keyboard_mapping(48, 1)
            .unwrap()
            .reply()
            .unwrap();
        assert!(
            accent.keysyms.contains(&0xfe51),
            "Spanish dead acute key is missing"
        );

        use smithay::reexports::x11rb::protocol::{randr, Event};
        // Match GTK's subscription: SetMonitor's resource-change event alone
        // does not reach these clients. Require the compatible notification.
        randr::select_input(&connection, screen.root, randr::NotifyMask::OUTPUT_PROPERTY)
            .unwrap()
            .check()
            .unwrap();
        let layout_property = atom(b"_AETHERDESK_MONITOR_LAYOUT");
        let wait_for_layout_notification = || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(Event::RandrNotify(event)) = connection.poll_for_event().unwrap() {
                    if event.sub_code == randr::Notify::OUTPUT_PROPERTY
                        && event.u.as_op().atom == layout_property
                    {
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "monitor change was not delivered to GTK-compatible listeners"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // Mixed monitor sizes and offsets must be real outputs/CRTCs as well
        // as logical rectangles, so GTK, Qt and the host WM all agree.
        let regions = vec![
            ad_stream_protocol::Rectangle {
                x: 0,
                y: 0,
                width: 637,
                height: 600,
            },
            ad_stream_protocol::Rectangle {
                x: 637,
                y: 100,
                width: 803,
                height: 497,
            },
            ad_stream_protocol::Rectangle {
                x: 1440,
                y: 0,
                width: 480,
                height: 600,
            },
        ];
        let properties: Vec<_> = (0..3)
            .map(|i| ad_stream_protocol::MonitorProperties {
                name: format!("Client display {}", i + 1),
                dpi: 120,
                width_mm: 400 + i * 10,
                height_mm: 250,
                refresh_millihz: 60000,
                primary: i == 0,
            })
            .collect();
        compositor
            .set_monitor_configuration(1920, 600, regions.clone(), properties.clone())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let monitors = smithay::reexports::x11rb::protocol::randr::get_monitors(
                &connection,
                screen.root,
                false,
            )
            .unwrap()
            .reply()
            .unwrap();
            if monitors.monitors.len() == 3
                && regions.iter().all(|r| {
                    monitors.monitors.iter().any(|m| {
                        m.x == r.x as i16
                            && m.y == r.y as i16
                            && m.width == r.width as u16
                            && m.height == r.height as u16
                            && m.outputs.len() == 1
                    })
                })
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host did not expose three monitor areas: {monitors:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        wait_for_layout_notification();
        let resources = randr::get_screen_resources_current(&connection, screen.root)
            .unwrap()
            .reply()
            .unwrap();
        let mut actual = Vec::new();
        for output in resources.outputs {
            let info = randr::get_output_info(&connection, output, 0)
                .unwrap()
                .reply()
                .unwrap();
            if info.connection == randr::Connection::CONNECTED && info.crtc != 0 {
                let crtc = randr::get_crtc_info(&connection, info.crtc, 0)
                    .unwrap()
                    .reply()
                    .unwrap();
                let index = regions
                    .iter()
                    .position(|r| r.x == crtc.x as u32 && r.y == crtc.y as u32)
                    .unwrap();
                assert_eq!(
                    info.mm_width, properties[index].width_mm,
                    "client physical width was not exposed through RandR"
                );
                assert_eq!(
                    info.mm_height, properties[index].height_mm,
                    "client physical height was not exposed through RandR"
                );
                actual.push((crtc.x, crtc.y, crtc.width, crtc.height));
            }
        }
        assert_eq!(
            actual.len(),
            3,
            "Each monitor needs a distinct connected output"
        );
        for region in &regions {
            assert!(actual.contains(&(
                region.x as i16,
                region.y as i16,
                region.width as u16,
                region.height as u16
            )));
        }
        // The host WM should maximize within a monitor rather than across all three.
        let state_atom = atom(b"_NET_WM_STATE");
        connection
            .send_event(
                false,
                screen.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(
                    32,
                    window,
                    state_atom,
                    [
                        1,
                        atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
                        atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
                        1,
                        0,
                    ],
                ),
            )
            .unwrap();
        connection.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            if geometry.width > 500
                && geometry.width <= 640
                && geometry.height > 500
                && geometry.height <= 600
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host WM did not maximize inside one monitor: {}x{} at {},{}",
                geometry.width,
                geometry.height,
                geometry.x,
                geometry.y
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // Rearrange an unchanged canvas too: the already-maximized window
        // must react without needing another screen-size notification.
        compositor
            .set_monitors(
                1920,
                600,
                (0..2)
                    .map(|i| ad_stream_protocol::Rectangle {
                        x: i * 960,
                        y: 0,
                        width: 960,
                        height: 600,
                    })
                    .collect(),
            )
            .unwrap();
        wait_for_layout_notification();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            if geometry.width > 900
                && geometry.width <= 960
                && geometry.height > 500
                && geometry.height <= 600
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "WM kept stale geometry after a same-size canvas rearrangement: {}x{}",
                geometry.width,
                geometry.height
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // Exact reported hardware: 2880x1800 at 200%, 1920x1080 at 100%.
        // Host outputs use physical pixels independently of Windows UI scaling.
        let mixed = vec![
            ad_stream_protocol::Rectangle {
                x: 0,
                y: 0,
                width: 2880,
                height: 1800,
            },
            ad_stream_protocol::Rectangle {
                x: 2880,
                y: 720,
                width: 1920,
                height: 1080,
            },
        ];
        compositor.set_monitors(4800, 1800, mixed.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let monitors = randr::get_monitors(&connection, screen.root, true)
                .unwrap()
                .reply()
                .unwrap();
            if monitors.monitors.len() == 2
                && mixed.iter().all(|r| {
                    monitors.monitors.iter().any(|m| {
                        (m.x, m.y, m.width, m.height)
                            == (r.x as i16, r.y as i16, r.width as u16, r.height as u16)
                            && m.outputs.len() == 1
                    })
                })
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mixed-DPI physical outputs were lost: {monitors:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        for monitor in randr::get_monitors(&connection, screen.root, true)
            .unwrap()
            .reply()
            .unwrap()
            .monitors
        {
            let info = randr::get_output_info(&connection, monitor.outputs[0], 0)
                .unwrap()
                .reply()
                .unwrap();
            let crtc = randr::get_crtc_info(&connection, info.crtc, 0)
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(
                (crtc.x, crtc.y, crtc.width, crtc.height),
                (monitor.x, monitor.y, monitor.width, monitor.height)
            );
        }
        // Put a normal application on the monitor that will disappear.
        connection
            .send_event(
                false,
                screen.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(
                    32,
                    window,
                    state_atom,
                    [
                        0,
                        atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
                        atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
                        1,
                        0,
                    ],
                ),
            )
            .unwrap();
        connection.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        connection
            .configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x(3300)
                    .y(900)
                    .width(1100)
                    .height(700),
            )
            .unwrap()
            .check()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pos = connection
                .translate_coordinates(window, screen.root, 0, 0)
                .unwrap()
                .reply()
                .unwrap();
            if pos.dst_x >= 2880 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "test window did not reach the second monitor: {pos:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        compositor
            .set_monitors(
                800,
                600,
                vec![ad_stream_protocol::Rectangle {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 600,
                }],
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let geometry = connection
                .get_geometry(screen.root)
                .unwrap()
                .reply()
                .unwrap();
            if (geometry.width, geometry.height) == (800, 600) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host desktop did not follow remote display resize: {geometry:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let monitors = randr::get_monitors(&connection, screen.root, true)
                .unwrap()
                .reply()
                .unwrap();
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            let pos = connection
                .translate_coordinates(window, screen.root, 0, 0)
                .unwrap()
                .reply()
                .unwrap();
            let extents = connection
                .get_property(
                    false,
                    window,
                    atom(b"_NET_FRAME_EXTENTS"),
                    AtomEnum::CARDINAL,
                    0,
                    4,
                )
                .unwrap()
                .reply()
                .unwrap()
                .value32()
                .unwrap()
                .collect::<Vec<_>>();
            let left = i32::from(pos.dst_x) - extents[0] as i32;
            let top = i32::from(pos.dst_y) - extents[2] as i32;
            let right = i32::from(pos.dst_x) + i32::from(geometry.width) + extents[1] as i32;
            let bottom = i32::from(pos.dst_y) + i32::from(geometry.height) + extents[3] as i32;
            if monitors.monitors.len() == 1
                && monitors.monitors[0].width == 800
                && monitors.monitors[0].height == 600
                && left >= 0
                && top >= 0
                && right <= 800
                && bottom <= 600
            {
                break;
            }
            assert!(Instant::now() < deadline,
                "return to tab left an application outside the active monitor: frame=({left},{top})-({right},{bottom}), monitors={monitors:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        compositor.shutdown();
    }
}
