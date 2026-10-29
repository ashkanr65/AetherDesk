//! Clipboard bridge for a host-managed X11 desktop, without claiming WM ownership.
use crate::compositor::{CapturedClipboard, ClipboardSelection};
use smithay::reexports::x11rb::{
    connection::Connection,
    protocol::{xfixes, xproto::*, Event},
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
};
use std::{
    collections::HashMap,
    os::unix::net::UnixStream,
    sync::mpsc,
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const LIMIT: usize = 10 * 1024 * 1024;
const CHUNK: usize = 32 * 1024;
const FORMATS: [&str; 6] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "text/html",
    "image/png",
    "image/bmp",
];

enum DesktopCommand {
    Clipboard(ClipboardSelection),
    Wheel(i32, i32),
    Monitors(
        u32,
        u32,
        Vec<ad_stream_protocol::Rectangle>,
        Vec<ad_stream_protocol::MonitorProperties>,
    ),
}

pub(crate) struct HostClipboard(mpsc::Sender<DesktopCommand>);

impl HostClipboard {
    pub(crate) fn start(
        display: u32,
        cookie: [u8; 16],
        output: tokio::sync::mpsc::Sender<CapturedClipboard>,
        cursors: tokio::sync::watch::Sender<
            Option<std::sync::Arc<crate::host_cursor::CursorImage>>,
        >,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("aetherdesk-x11-clipboard".into())
            .spawn(move || {
                if let Err(error) = Bridge::run(display, cookie, receiver, output, cursors) {
                    tracing::warn!(%error, "host desktop clipboard bridge stopped");
                }
            })?;
        Ok(Self(sender))
    }

    pub(crate) fn set_monitors(
        &self,
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
        properties: Vec<ad_stream_protocol::MonitorProperties>,
    ) {
        let _ = self.0.send(DesktopCommand::Monitors(
            width, height, monitors, properties,
        ));
    }

    pub(crate) fn set(&self, selection: ClipboardSelection) {
        let _ = self.0.send(DesktopCommand::Clipboard(selection));
    }
    pub(crate) fn wheel(&self, horizontal: i32, vertical: i32) -> bool {
        self.0
            .send(DesktopCommand::Wheel(horizontal, vertical))
            .is_ok()
    }
}

struct Transfer {
    target: Atom,
    data: Vec<u8>,
    offset: usize,
    updated: Instant,
}

struct Bridge {
    connection: RustConnection,
    window: Window,
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    incr: Atom,
    property: Atom,
    formats: Vec<(Atom, &'static str)>,
    selection: Option<ClipboardSelection>,
    owner_time: u32,
    incoming_time: u32,
    incoming_target: Atom,
    incoming: Option<Transfer>,
    outgoing: HashMap<(Window, Atom), Transfer>,
    monitors: Vec<Atom>,
    monitor_properties: Vec<ad_stream_protocol::MonitorProperties>,
    pending_monitors: Option<(u32, u32, Vec<ad_stream_protocol::Rectangle>, Instant)>,
    pending_windows: Option<(Vec<ad_stream_protocol::Rectangle>, Instant)>,
    output: tokio::sync::mpsc::Sender<CapturedClipboard>,
    wheel: [i32; 2],
}

impl Bridge {
    fn run(
        display: u32,
        cookie: [u8; 16],
        commands: mpsc::Receiver<DesktopCommand>,
        output: tokio::sync::mpsc::Sender<CapturedClipboard>,
        cursors: tokio::sync::watch::Sender<
            Option<std::sync::Arc<crate::host_cursor::CursorImage>>,
        >,
    ) -> Result<(), Error> {
        let stream = DefaultStream::from_unix_stream(UnixStream::connect(format!(
            "/tmp/.X11-unix/X{display}"
        ))?)?
        .0;
        let connection = RustConnection::connect_to_stream_with_auth_info(
            stream,
            0,
            b"MIT-MAGIC-COOKIE-1".to_vec(),
            cookie.to_vec(),
        )?;
        let atom = |name: &str| -> Result<Atom, Error> {
            Ok(connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom)
        };
        let window = connection.generate_id()?;
        let root = connection.setup().roots[0].root;
        connection
            .create_window(
                0,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let clipboard = atom("CLIPBOARD")?;
        xfixes::query_version(&connection, 5, 0)?.reply()?;
        x11rb::protocol::xtest::get_version(&connection, 2, 2)?.reply()?;
        xfixes::select_selection_input(
            &connection,
            window,
            clipboard,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER
                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?
        .check()?;
        // Cursor shape notifications are independent of desktop damage/video.
        let cursor_supported = xfixes::select_cursor_input(
            &connection,
            window,
            xfixes::CursorNotifyMask::DISPLAY_CURSOR,
        )?
        .check()
        .is_ok();
        let mut cursor_dirty = cursor_supported;
        let mut cursor_at = Instant::now() - Duration::from_secs(1);
        let mut bridge = Self {
            window,
            clipboard,
            targets: atom("TARGETS")?,
            timestamp: atom("TIMESTAMP")?,
            incr: atom("INCR")?,
            property: atom("_AETHERDESK_CLIPBOARD")?,
            formats: FORMATS
                .iter()
                .map(|name| Ok((atom(name)?, *name)))
                .collect::<Result<_, Error>>()?,
            connection,
            selection: None,
            owner_time: 0,
            incoming_time: 0,
            incoming_target: 0,
            incoming: None,
            outgoing: HashMap::new(),
            monitors: Vec::new(),
            monitor_properties: Vec::new(),
            pending_monitors: None,
            pending_windows: None,
            output,
            wheel: [0; 2],
        };
        bridge.connection.flush()?;
        loop {
            match commands.recv_timeout(Duration::from_millis(10)) {
                Ok(DesktopCommand::Clipboard(selection)) => {
                    bridge.selection = Some(selection);
                    bridge.incoming = None;
                    bridge
                        .connection
                        .set_selection_owner(bridge.window, bridge.clipboard, 0u32)?;
                }
                Ok(DesktopCommand::Wheel(horizontal, vertical)) => {
                    // Rootful Xwayland switches between its absolute and relative
                    // pointer devices for Wayland motion/axis events. GTK resets
                    // its smooth-scroll baseline on that switch. Send real X11
                    // wheel buttons once, as the independent-window path does.
                    use x11rb::protocol::xtest;
                    for (index, value, positive, negative) in
                        [(0, horizontal, 7, 6), (1, vertical, 4, 5)]
                    {
                        let steps =
                            crate::window_mirror::wheel_steps(&mut bridge.wheel[index], value);
                        let button = if steps > 0 { positive } else { negative };
                        for _ in 0..steps.unsigned_abs() {
                            xtest::fake_input(
                                &bridge.connection,
                                BUTTON_PRESS_EVENT,
                                button,
                                0,
                                root,
                                0,
                                0,
                                0,
                            )?;
                            xtest::fake_input(
                                &bridge.connection,
                                BUTTON_RELEASE_EVENT,
                                button,
                                0,
                                root,
                                0,
                                0,
                                0,
                            )?;
                        }
                    }
                    bridge.connection.flush()?;
                }
                Ok(DesktopCommand::Monitors(width, height, monitors, properties)) => {
                    bridge.monitor_properties = properties;
                    bridge.pending_windows = None;
                    bridge.pending_monitors = Some((width, height, monitors, Instant::now()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Err(error) = bridge.update_monitors() {
                // RandR resources can change while the desktop settings daemon
                // starts. Retain the complete layout until all outputs verify.
                if bridge
                    .pending_monitors
                    .as_ref()
                    .is_some_and(|(_, _, _, since)| since.elapsed() < Duration::from_secs(5))
                {
                    tracing::debug!(%error, "retrying host monitor arrangement");
                } else {
                    tracing::warn!(%error, "host monitor arrangement could not be applied");
                    bridge.pending_monitors = None;
                }
            }
            for _ in 0..128 {
                let Some(event) = bridge.connection.poll_for_event()? else {
                    break;
                };
                if matches!(event, Event::XfixesCursorNotify(_)) {
                    cursor_dirty = true;
                    continue;
                }
                // A disappearing application must not tear down the bridge.
                if let Err(error) = bridge.event(event) {
                    tracing::debug!(%error, "X11 clipboard transfer failed");
                }
            }
            if cursor_dirty && cursor_at.elapsed() >= Duration::from_millis(20) {
                cursor_dirty = false;
                cursor_at = Instant::now();
                match crate::host_cursor::capture(&bridge.connection) {
                    Ok(image) => {
                        let changed = cursors.borrow().as_deref() != Some(&image);
                        if changed {
                            cursors.send_replace(Some(std::sync::Arc::new(image)));
                        }
                    }
                    Err(error) => tracing::debug!(%error, "Host cursor shape unavailable"),
                }
            }
            // Let the window manager consume RandR/work-area notifications
            // before fitting normal windows. New layouts supersede this work.
            if bridge
                .pending_windows
                .as_ref()
                .is_some_and(|(_, since)| since.elapsed() >= Duration::from_millis(150))
            {
                let (monitors, _) = bridge.pending_windows.take().unwrap();
                if let Err(error) = crate::host_windows::fit_windows(&bridge.connection, &monitors)
                {
                    tracing::warn!(%error, "could not fit windows after monitor change");
                }
            }
            bridge
                .outgoing
                .retain(|_, transfer| transfer.updated.elapsed() < Duration::from_secs(15));
            if bridge
                .incoming
                .as_ref()
                .is_some_and(|t| t.updated.elapsed() > Duration::from_secs(15))
            {
                bridge.incoming = None;
            }
            bridge.connection.flush()?;
        }
    }

    fn update_monitors(&mut self) -> Result<(), Error> {
        use smithay::reexports::x11rb::protocol::randr;
        let Some((width, height, _, since)) = &self.pending_monitors else {
            return Ok(());
        };
        let root = self.connection.setup().roots[0].root;
        let geometry = self.connection.get_geometry(root)?.reply()?;
        if u32::from(geometry.width) != *width || u32::from(geometry.height) != *height {
            if since.elapsed() > Duration::from_secs(5) {
                return Err("desktop resize timed out before monitor configuration".into());
            }
            return Ok(());
        }
        let monitors = self.pending_monitors.as_ref().unwrap().2.clone();
        let version = randr::query_version(&self.connection, 1, 5)?.reply()?;
        if (version.major_version, version.minor_version) < (1, 5) {
            return Err("X11 server does not support monitor configuration".into());
        }
        for name in self.monitors.drain(..) {
            let _ = randr::delete_monitor(&self.connection, root, name)?.check();
        }
        let outputs = randr::get_screen_resources_current(&self.connection, root)?
            .reply()?
            .outputs;
        let support = self
            .connection
            .intern_atom(false, b"_AETHERDESK_OUTPUT_RECTS_VERSION")?
            .reply()?
            .atom;
        let layout_property = self
            .connection
            .intern_atom(false, b"_AETHERDESK_OUTPUT_RECTS")?
            .reply()?
            .atom;
        let mut controller = None;
        for &output in &outputs {
            let version = randr::get_output_property(
                &self.connection,
                output,
                support,
                AtomEnum::CARDINAL,
                0,
                1,
                false,
                false,
            )?
            .reply()?;
            if version.format == 32 && version.data.len() == 4 && version.data == 1u32.to_ne_bytes()
            {
                controller = Some(output);
                break;
            }
        }
        if let Some(controller) = controller {
            let details_atom = self
                .connection
                .intern_atom(false, b"_AETHERDESK_OUTPUT_DETAILS")?
                .reply()?
                .atom;
            let details: Vec<u8> = monitors
                .iter()
                .enumerate()
                .flat_map(|(index, r)| {
                    self.monitor_properties
                        .get(index)
                        .map(|p| [p.width_mm, p.height_mm, p.refresh_millihz])
                        .unwrap_or([r.width * 254 / 960, r.height * 254 / 960, 60000])
                })
                .flat_map(u32::to_ne_bytes)
                .collect();
            randr::change_output_property(
                &self.connection,
                controller,
                details_atom,
                AtomEnum::CARDINAL.into(),
                32,
                PropMode::REPLACE,
                (details.len() / 4) as u32,
                &details,
            )?
            .check()?;
            let metadata_atom = self
                .connection
                .intern_atom(false, b"_AETHERDESK_DISPLAY_PROPERTIES")?
                .reply()?
                .atom;
            let mut metadata = Vec::new();
            for properties in &self.monitor_properties {
                let name = self
                    .connection
                    .intern_atom(false, properties.name.as_bytes())?
                    .reply()?
                    .atom;
                metadata.extend_from_slice(&[
                    name,
                    u32::from(properties.dpi),
                    properties.width_mm,
                    properties.height_mm,
                    properties.refresh_millihz,
                    u32::from(properties.primary),
                ]);
            }
            self.connection.change_property32(
                PropMode::REPLACE,
                root,
                metadata_atom,
                AtomEnum::CARDINAL,
                &metadata,
            )?;
            let data: Vec<_> = monitors
                .iter()
                .flat_map(|r| [r.x, r.y, r.width, r.height])
                .flat_map(u32::to_ne_bytes)
                .collect();
            randr::change_output_property(
                &self.connection,
                controller,
                layout_property,
                AtomEnum::CARDINAL.into(),
                32,
                PropMode::REPLACE,
                (data.len() / 4) as u32,
                &data,
            )?
            .check()?;
            // Query the real output set after Xwayland creates the CRTCs.
            let resources = randr::get_screen_resources_current(&self.connection, root)?.reply()?;
            for (index, region) in monitors.iter().enumerate() {
                let expected_name = format!("AetherDesk-{}", index + 1);
                let mut matching_output = None;
                for &output in &resources.outputs {
                    let info = randr::get_output_info(&self.connection, output, 0)?.reply()?;
                    if info.name == expected_name.as_bytes() && info.crtc != 0 {
                        let crtc = randr::get_crtc_info(&self.connection, info.crtc, 0)?.reply()?;
                        if crtc.x == region.x as i16
                            && crtc.y == region.y as i16
                            && crtc.width == region.width as u16
                            && crtc.height == region.height as u16
                        {
                            matching_output = Some(output);
                        }
                    }
                }
                let output = matching_output
                    .ok_or("Xwayland did not apply the requested monitor geometry")?;
                let name = self
                    .connection
                    .intern_atom(
                        false,
                        format!("AetherDesk-Monitor-{}", index + 1).as_bytes(),
                    )?
                    .reply()?
                    .atom;
                randr::set_monitor(
                    &self.connection,
                    root,
                    randr::MonitorInfo {
                        name,
                        primary: monitors.len() == 1
                            || self
                                .monitor_properties
                                .get(index)
                                .map_or(index == 0, |p| p.primary),
                        automatic: false,
                        x: region.x as i16,
                        y: region.y as i16,
                        width: region.width as u16,
                        height: region.height as u16,
                        width_in_millimeters: self
                            .monitor_properties
                            .get(index)
                            .map_or(region.width * 254 / 960, |p| p.width_mm),
                        height_in_millimeters: self
                            .monitor_properties
                            .get(index)
                            .map_or(region.height * 254 / 960, |p| p.height_mm),
                        outputs: vec![output],
                    },
                )?
                .check()?;
                self.monitors.push(name);
            }
            let notification = self
                .connection
                .intern_atom(false, b"_AETHERDESK_MONITOR_LAYOUT")?
                .reply()?
                .atom;
            let data: Vec<_> = self
                .monitors
                .iter()
                .flat_map(|atom| atom.to_ne_bytes())
                .collect();
            for output in resources.outputs {
                randr::change_output_property(
                    &self.connection,
                    output,
                    notification,
                    AtomEnum::ATOM.into(),
                    32,
                    PropMode::REPLACE,
                    self.monitors.len() as u32,
                    &data,
                )?
                .check()?;
            }
        } else if monitors.len() > 1 {
            return Err("Multiple monitors require the bundled AetherDesk Xwayland server".into());
        }
        self.pending_windows = Some((monitors, Instant::now()));
        self.pending_monitors = None;
        Ok(())
    }

    fn event(&mut self, event: Event) -> Result<(), Error> {
        match event {
            Event::XfixesSelectionNotify(event) if event.selection == self.clipboard => {
                if event.owner == self.window {
                    self.owner_time = event.selection_timestamp;
                } else {
                    self.selection = None;
                    self.incoming = None;
                    self.incoming_time = event.timestamp;
                    self.incoming_target = self.targets;
                    self.connection
                        .delete_property(self.window, self.property)?;
                    if event.owner != 0 {
                        self.connection.convert_selection(
                            self.window,
                            self.clipboard,
                            self.targets,
                            self.property,
                            self.incoming_time,
                        )?;
                    }
                }
            }
            Event::SelectionRequest(event) if event.selection == self.clipboard => {
                self.send(event)?
            }
            Event::SelectionNotify(event)
                if event.requestor == self.window
                    && event.selection == self.clipboard
                    && event.time == self.incoming_time
                    && event.property == self.property =>
            {
                let reply = self
                    .connection
                    .get_property(
                        true,
                        self.window,
                        self.property,
                        AtomEnum::ANY,
                        0,
                        (LIMIT / 4 + 1) as u32,
                    )?
                    .reply()?;
                if reply.bytes_after != 0 || reply.value.len() > LIMIT {
                    return Ok(());
                }
                if event.target == self.targets && reply.type_ == u32::from(AtomEnum::ATOM) {
                    let offered: Vec<_> = reply.value32().into_iter().flatten().collect();
                    if let Some((target, _)) = self
                        .formats
                        .iter()
                        .find(|(target, _)| offered.contains(target))
                    {
                        self.incoming_target = *target;
                        self.connection.convert_selection(
                            self.window,
                            self.clipboard,
                            *target,
                            self.property,
                            self.incoming_time,
                        )?;
                    }
                } else if reply.type_ == self.incr {
                    self.incoming = Some(Transfer {
                        target: event.target,
                        data: Vec::new(),
                        offset: 0,
                        updated: Instant::now(),
                    });
                } else if reply.format == 8 && event.target == self.incoming_target {
                    self.deliver(event.target, reply.value);
                }
            }
            Event::PropertyNotify(event) => {
                if event.window == self.window
                    && event.atom == self.property
                    && event.state == Property::NEW_VALUE
                    && self.incoming.is_some()
                {
                    let reply = self
                        .connection
                        .get_property(
                            true,
                            self.window,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            (LIMIT / 4 + 1) as u32,
                        )?
                        .reply()?;
                    let mut transfer = self.incoming.take().unwrap();
                    if reply.bytes_after != 0
                        || reply.format != 8
                        || transfer.data.len() + reply.value.len() > LIMIT
                    {
                        return Ok(());
                    }
                    if reply.value.is_empty() {
                        self.deliver(transfer.target, transfer.data);
                    } else {
                        transfer.data.extend(reply.value);
                        transfer.updated = Instant::now();
                        self.incoming = Some(transfer);
                    }
                }
                if event.state == Property::DELETE {
                    if let Some(mut transfer) = self.outgoing.remove(&(event.window, event.atom)) {
                        let end = (transfer.offset + CHUNK).min(transfer.data.len());
                        self.connection.change_property8(
                            PropMode::REPLACE,
                            event.window,
                            event.atom,
                            transfer.target,
                            &transfer.data[transfer.offset..end],
                        )?;
                        if transfer.offset < transfer.data.len() {
                            transfer.offset = end;
                            transfer.updated = Instant::now();
                            self.outgoing.insert((event.window, event.atom), transfer);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn data(&self, target: Atom) -> Option<Vec<u8>> {
        let selection = self.selection.as_ref()?;
        let name = self.formats.iter().find(|(atom, _)| *atom == target)?.1;
        if matches!(
            name,
            "UTF8_STRING" | "text/plain;charset=utf-8" | "text/plain"
        ) {
            selection
                .data("text/plain;charset=utf-8")
                .or_else(|| selection.data("text/plain"))
        } else {
            selection.data(name)
        }
    }

    fn send(&mut self, event: SelectionRequestEvent) -> Result<(), Error> {
        let property = if event.property == 0 {
            event.target
        } else {
            event.property
        };
        let mut accepted = self.selection.is_some();
        if accepted && event.target == self.targets {
            let mut targets = vec![self.targets, self.timestamp];
            targets.extend(
                self.formats
                    .iter()
                    .filter(|(atom, _)| self.data(*atom).is_some())
                    .map(|(atom, _)| *atom),
            );
            self.connection.change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            )?;
        } else if accepted && event.target == self.timestamp {
            self.connection.change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                AtomEnum::INTEGER,
                &[self.owner_time],
            )?;
        } else if let Some(data) = self.data(event.target) {
            if data.len() <= CHUNK {
                self.connection.change_property8(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    event.target,
                    &data,
                )?;
            } else if self.outgoing.len() < 8 {
                self.connection.change_window_attributes(
                    event.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?;
                self.connection.change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    self.incr,
                    &[data.len() as u32],
                )?;
                self.outgoing.insert(
                    (event.requestor, property),
                    Transfer {
                        target: event.target,
                        data,
                        offset: 0,
                        updated: Instant::now(),
                    },
                );
            } else {
                accepted = false;
            }
        } else {
            accepted = false;
        }
        let reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: event.time,
            requestor: event.requestor,
            selection: event.selection,
            target: event.target,
            property: if accepted { property } else { 0 },
        };
        self.connection
            .send_event(false, event.requestor, EventMask::NO_EVENT, reply)?;
        Ok(())
    }

    fn deliver(&self, target: Atom, data: Vec<u8>) {
        if let Some((_, name)) = self.formats.iter().find(|(atom, _)| *atom == target) {
            let mime_type = if *name == "UTF8_STRING" {
                "text/plain;charset=utf-8"
            } else {
                name
            };
            if ClipboardSelection::new(mime_type.to_string(), data.clone()).is_ok() {
                let _ = self.output.try_send(CapturedClipboard {
                    mime_type: mime_type.to_string(),
                    data,
                });
            }
        }
    }
}
