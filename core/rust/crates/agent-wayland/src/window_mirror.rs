//! Opt-in X11 window mirror. Never changes the desktop compositing preference,
//! replaces the WM or closes an application. An explicitly mirrored client
//! minimized on the desktop can live in a private, crash-safe capture holder.
use ad_stream_protocol::{lossless::LosslessTile, window_stream as wire, Rectangle};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Read,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::unix::AsyncFd;
use x11rb::{
    connection::Connection,
    protocol::{
        composite::{self, Redirect},
        damage::{self, ReportLevel},
        xfixes,
        xproto::*,
        xtest, Event,
    },
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
};

#[path = "window_popups.rs"]
mod popups;
use popups::PopupCapture;
#[path = "window_capture.rs"]
mod capture;
#[path = "window_detach.rs"]
mod detach;
#[path = "window_updates.rs"]
mod updates;

type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Debug)]
struct FocusChanged;
impl std::fmt::Display for FocusChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Application focus changed")
    }
}
impl std::error::Error for FocusChanged {}

fn input_failure_reply(error: &Error) -> wire::Reply {
    // Focus can move through a toolkit menu while a queued input is in flight.
    // Keep rejecting stale input, but recover silently rather than painting a warning.
    if error.is::<FocusChanged>() {
        // The client hides this condition and resets its held-input snapshot.
        // An ordinary Ack would leave Ctrl/Shift out of sync after cleanup.
        wire::Reply::Error("Application focus changed".into())
    } else {
        wire::Reply::Error(error.to_string().chars().take(240).collect())
    }
}
fn x_button(button: u8) -> Result<u8, Error> {
    match button {
        1 => Ok(1),
        2 => Ok(3),
        3 => Ok(2),
        4 => Ok(8),
        5 => Ok(9),
        _ => Err("Unsupported application button".into()),
    }
}
struct Selected {
    id: u32,
    damage: u32,
    pixmap: u32,
    size: (u32, u32),
    pixels: Vec<u8>,
    sequence: u64,
    dirty: bool,
    destroyed: bool,
    rebind: bool,
    popups: Vec<wire::Popup>,
    client_range: Option<(u32, u32)>,
    info: wire::WindowInfo,
    metadata_dirty: bool,
    region: u32,
}
pub struct Mirror {
    connection: Arc<RustConnection>,
    root: u32,
    application_runtime: Option<std::path::PathBuf>,
    selected: Option<Selected>,
    keys: BTreeSet<u8>,
    buttons: BTreeSet<u8>,
    wheel: [i32; 2],
    popups: std::collections::BTreeMap<u32, PopupCapture>,
    menu_cache: std::cell::RefCell<Option<(Instant, Vec<(u32, i32, i32, u32, u32)>)>>,
    atoms: std::cell::RefCell<std::collections::BTreeMap<Vec<u8>, u32>>,
    last_read_pixels: u64,
    detached: Option<detach::Detached>,
}

pub(crate) fn wheel_steps(remainder: &mut i32, value: i32) -> i32 {
    let total = i64::from(*remainder) + i64::from(value);
    *remainder = (total % 120) as i32;
    // X11 exposes whole wheel buttons. Preserve small movements, retain the
    // existing eight-click per-request bound, and never amplify every fraction.
    (total / 120).clamp(-8, 8) as i32
}

fn private(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err("Remote desktop identity is not private to this account".into());
    }
    Ok(())
}

fn authority(path: &Path) -> Result<(u32, Vec<u8>), Error> {
    private(path, false)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if file.metadata()?.len() > 4096 {
        return Err("Invalid desktop authority".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut rest = bytes.as_slice();
    fn field<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8], Error> {
        if rest.len() < 2 {
            return Err("Incomplete desktop authority".into());
        }
        let size = u16::from_be_bytes([rest[0], rest[1]]) as usize;
        *rest = &rest[2..];
        if rest.len() < size {
            return Err("Incomplete desktop authority".into());
        }
        let value = &rest[..size];
        *rest = &rest[size..];
        Ok(value)
    }
    if rest.len() < 2 {
        return Err("Invalid desktop authority".into());
    }
    rest = &rest[2..];
    let _address = field(&mut rest)?;
    let display = std::str::from_utf8(field(&mut rest)?)?.parse::<u32>()?;
    let protocol = field(&mut rest)?;
    let cookie = field(&mut rest)?;
    if protocol != b"MIT-MAGIC-COOKIE-1"
        || cookie.len() != 16
        || !rest.is_empty()
        || display > 65535
    {
        return Err("Invalid desktop authority".into());
    }
    Ok((display, cookie.to_vec()))
}

/// A shell prefix contains paths only, never the X authentication cookie.
/// The private environment is sourced on the host, outside shell history.
pub fn application_environment(session: &str) -> Result<String, Error> {
    let mirror = Mirror::connect_session(session)?;
    let manager = mirror
        .values(mirror.root, b"_NET_SUPPORTING_WM_CHECK")?
        .first()
        .copied()
        .ok_or("Application window manager is starting")?;
    mirror.connection.get_window_attributes(manager)?.reply()?;
    let runtime = mirror
        .application_runtime
        .as_ref()
        .ok_or("Application session unavailable")?;
    let environment = runtime.join("application-environment.sh");
    private(&environment, false)?;
    let pipe = runtime.join("application-launch.fifo");
    validate_launch_pipe(&pipe)?;
    let candidates: Vec<_> = std::fs::read_dir(runtime)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|s| s == "Xauthority"))
        .collect();
    if candidates.len() != 1 {
        return Err("Application display identity is not unique".into());
    }
    let (display, _) = authority(&candidates[0])?;
    let quote = |path: &Path| -> Result<String, Error> {
        let text = path.to_str().ok_or("Application path is not Unicode")?;
        if text.chars().any(char::is_control) {
            return Err("Invalid application path".into());
        }
        Ok(format!("'{}'", text.replace('\'', "'\\''")))
    };
    Ok(format!(". {}; unset WAYLAND_DISPLAY; export DISPLAY=':{display}' XAUTHORITY={} GDK_BACKEND=x11 QT_QPA_PLATFORM=xcb XDG_SESSION_TYPE=x11",
        quote(&environment)?, quote(&candidates[0])?))
}

fn validate_launch_pipe(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::FileTypeExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_fifo()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("Application launcher is not private to this session".into());
    }
    Ok(())
}
fn enqueue_application(runtime: &Path, command: &str) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::FileTypeExt;
    if command.trim().is_empty() || command.len() > 2048 || command.contains(['\n', '\r', '\0']) {
        return Err("Use one application command of at most 2048 bytes".into());
    }
    let path = runtime.join("application-launch.fifo");
    validate_launch_pipe(&path)?;
    let mut pipe = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = pipe.metadata()?;
    if !metadata.file_type().is_fifo()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("Application launcher ownership changed".into());
    }
    // Below Linux PIPE_BUF: a single write is atomic across simultaneous tabs.
    let line = format!("{command}\n");
    if pipe.write(line.as_bytes())? != line.len() {
        return Err("Application launcher is busy; retry".into());
    }
    Ok(())
}

impl Mirror {
    pub fn connect_session(session: &str) -> Result<Self, Error> {
        // The caller passes a parsed UUID; arbitrary paths/displays are refused.
        if session.len() != 36
            || !session.bytes().enumerate().all(|(n, b)| {
                if [8, 13, 18, 23].contains(&n) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err("Invalid desktop session".into());
        }
        let directory =
            std::path::PathBuf::from(std::env::var_os("HOME").ok_or("Home directory unavailable")?)
                .join(".aetherdesk/sessions")
                .join(session);
        private(&directory, true)?;
        let runtime = directory.join("runtime");
        private(&runtime, true)?;
        let candidates = std::fs::read_dir(&runtime)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("wayland-") && n.ends_with(".Xauthority"))
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err("The saved desktop has no unique live X11 identity".into());
        }
        let (display, cookie) = authority(&candidates[0])?;
        let socket = std::path::PathBuf::from(format!("/tmp/.X11-unix/X{display}"));
        if std::fs::symlink_metadata(&socket)?.uid() != unsafe { libc::getuid() } {
            return Err("Desktop socket belongs to another account".into());
        }
        let stream = DefaultStream::from_unix_stream(UnixStream::connect(socket)?)?.0;
        let connection = RustConnection::connect_to_stream_with_auth_info(
            stream,
            0,
            b"MIT-MAGIC-COOKIE-1".to_vec(),
            cookie,
        )?;
        let mut mirror = Self::from_connection(connection)?;
        mirror.application_runtime = Some(runtime);
        Ok(mirror)
    }
    pub(crate) fn from_connection(
        connection: impl Into<Arc<RustConnection>>,
    ) -> Result<Self, Error> {
        let connection = connection.into();
        let root = connection.setup().roots[0].root;
        let version = composite::query_version(&connection, 0, 4)?.reply()?;
        if version.major_version == 0 && version.minor_version < 2 {
            return Err("Desktop lacks window capture support".into());
        }
        damage::query_version(&connection, 1, 1)?.reply()?;
        xtest::get_version(&connection, 2, 2)?.reply()?;
        xfixes::query_version(&connection, 5, 0)?.reply()?;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        Ok(Self {
            connection,
            root,
            application_runtime: None,
            selected: None,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            wheel: [0; 2],
            popups: std::collections::BTreeMap::new(),
            menu_cache: Default::default(),
            atoms: Default::default(),
            last_read_pixels: 0,
            detached: None,
        })
    }
    fn atom(&self, name: &[u8]) -> Result<u32, Error> {
        if let Some(atom) = self.atoms.borrow().get(name) {
            return Ok(*atom);
        }
        let atom = self.connection.intern_atom(false, name)?.reply()?.atom;
        self.atoms.borrow_mut().insert(name.to_vec(), atom);
        Ok(atom)
    }
    fn values(&self, window: u32, name: &[u8]) -> Result<Vec<u32>, Error> {
        let reply = self
            .connection
            .get_property(
                false,
                window,
                self.atom(name)?,
                AtomEnum::ANY,
                0,
                wire::MAX_WINDOWS as u32,
            )?
            .reply()?;
        Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
    }
    fn text(&self, window: u32, name: &[u8]) -> Result<String, Error> {
        let reply = self
            .connection
            .get_property(false, window, self.atom(name)?, AtomEnum::ANY, 0, 256)?
            .reply()?;
        Ok(String::from_utf8_lossy(&reply.value)
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect())
    }
    fn info(&self, id: u32) -> Result<wire::WindowInfo, Error> {
        let geometry = self.connection.get_geometry(id)?.reply()?;
        wire::dimensions(u32::from(geometry.width), u32::from(geometry.height))?;
        let title = self.text(id, b"_NET_WM_NAME").unwrap_or_default();
        Ok(wire::WindowInfo {
            id,
            title: if title.is_empty() {
                self.text(id, b"WM_NAME")
                    .unwrap_or_else(|_| "Application".into())
            } else {
                title
            },
            width: geometry.width.into(),
            height: geometry.height.into(),
            transient_for: self.values(id, b"WM_TRANSIENT_FOR")?.first().copied(),
        })
    }
    fn list(&self) -> Result<Vec<wire::WindowInfo>, Error> {
        let special = [
            self.atom(b"_NET_WM_WINDOW_TYPE_DESKTOP")?,
            self.atom(b"_NET_WM_WINDOW_TYPE_DOCK")?,
        ];
        let mut clients = self.values(self.root, b"_NET_CLIENT_LIST")?;
        for id in detach::listed_clients(&self.connection, self.root)? {
            if !clients.contains(&id) {
                clients.push(id);
            }
        }
        let mut windows: Vec<_> = clients
            .into_iter()
            .take(wire::MAX_WINDOWS)
            .filter_map(|id| {
                let types = self.values(id, b"_NET_WM_WINDOW_TYPE").ok()?;
                if types.iter().any(|v| special.contains(v)) {
                    return None;
                }
                let attrs = self
                    .connection
                    .get_window_attributes(id)
                    .ok()?
                    .reply()
                    .ok()?;
                if attrs.map_state != MapState::VIEWABLE {
                    return None;
                }
                self.info(id).ok()
            })
            .collect();
        if self.detached.is_some() {
            if let Some(selected) = &self.selected {
                if !selected.destroyed && !windows.iter().any(|info| info.id == selected.id) {
                    windows.push(self.info(selected.id)?);
                }
            }
        }
        Ok(windows)
    }
    fn release_input(&mut self) -> Result<(), Error> {
        self.wheel = [0; 2];
        if self.keys.is_empty() && self.buttons.is_empty() {
            return Ok(());
        }
        let connection = &self.connection;
        let root = self.root;
        let release = |event, detail| -> Result<(), Error> {
            xtest::fake_input(connection, event, detail, 0, root, 0, 0, 0)?.check()?;
            Ok(())
        };
        let mut failure = None;
        // Try every owned input, even if another release fails. Keep failed
        // entries so error cleanup/Drop can retry rather than forgetting them.
        for key in self.keys.clone() {
            match release(KEY_RELEASE_EVENT, key) {
                Ok(()) => {
                    self.keys.remove(&key);
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        for button in self.buttons.clone() {
            match release(BUTTON_RELEASE_EVENT, button) {
                Ok(()) => {
                    self.buttons.remove(&button);
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        // Check the requests, but do not confuse a request round trip with
        // device-event processing: a WM's synchronous grab can defer XTest.
        if let Err(error) = connection.sync() {
            failure.get_or_insert_with(|| error.into());
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn wait_released(&self, keys: &[u8], buttons: &[u8]) -> Result<(), Error> {
        if keys.is_empty() && buttons.is_empty() {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            let keys_up = if keys.is_empty() {
                true
            } else {
                let state = self.connection.query_keymap()?.reply()?.keys;
                keys.iter()
                    .all(|&code| state[usize::from(code / 8)] & (1 << (code % 8)) == 0)
            };
            // Core QueryPointer represents buttons 1–5, not extended buttons
            // 8/9. Those retain checked, ordered XTest release semantics.
            let buttons_up = if buttons.iter().all(|code| *code > 5) {
                true
            } else {
                let state = u16::from(self.connection.query_pointer(self.root)?.reply()?.mask);
                buttons
                    .iter()
                    .filter(|code| **code <= 5)
                    .all(|&code| state & (1 << (7 + code)) == 0)
            };
            if keys_up && buttons_up {
                return Ok(());
            }
            // After a focus handoff another view may legitimately hold the
            // same key/button. Never interpret that global state as ownership
            // of its input, or inject another release into that application.
            if self.selected.is_some() && self.application_has_focus().is_ok_and(|focused| !focused)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("The host has not finished releasing application input".into());
            }
            // Do not inject extra releases or clear another client's input.
            // Yield so the WM/toolkit can replay its queued device events.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn clear(&mut self) -> Result<(), Error> {
        let keys: Vec<_> = self.keys.iter().copied().collect();
        let buttons: Vec<_> = self.buttons.iter().copied().collect();
        let input = self
            .release_input()
            .and_then(|_| self.wait_released(&keys, &buttons));
        self.clear_popups();
        // Return a live hidden client to the host WM before removing capture.
        drop(self.detached.take());
        if let Some(s) = self.selected.take() {
            let _ = xfixes::destroy_region(&self.connection, s.region);
            let _ = self.connection.free_pixmap(s.pixmap);
            let _ = damage::destroy(&self.connection, s.damage);
            let _ = composite::unredirect_window(&self.connection, s.id, Redirect::AUTOMATIC);
            let _ = self.connection.flush();
        }
        *self.menu_cache.borrow_mut() = None;
        input
    }
    fn select(&mut self, id: u32) -> Result<wire::WindowInfo, Error> {
        let info = self
            .list()?
            .into_iter()
            .find(|w| w.id == id)
            .ok_or("Choose a visible application from this desktop")?;
        self.clear()?;
        // Automatic keeps the original desktop representation and avoids
        // taking over manual-compositor ownership from the user's WM.
        composite::redirect_window(&self.connection, id, Redirect::AUTOMATIC)?.check()?;
        let damage = self.connection.generate_id()?;
        let pixmap = self.connection.generate_id()?;
        let region = self.connection.generate_id()?;
        let client_range = self.application_client_range(id);
        self.selected = Some(Selected {
            id,
            damage,
            pixmap,
            size: (info.width, info.height),
            pixels: Vec::new(),
            sequence: 0,
            dirty: true,
            destroyed: false,
            rebind: false,
            popups: Vec::new(),
            client_range,
            info: info.clone(),
            metadata_dirty: false,
            region,
        });
        let prepared = (|| -> Result<(), Error> {
            self.connection
                .change_window_attributes(
                    id,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                )?
                .check()?;
            damage::create(&self.connection, damage, id, ReportLevel::NON_EMPTY)?.check()?;
            xfixes::create_region(&self.connection, region, &[])?.check()?;
            composite::name_window_pixmap(&self.connection, id, pixmap)?.check()?;
            self.connection.flush()?;
            Ok(())
        })();
        if let Err(error) = prepared {
            let _ = self.clear();
            return Err(error);
        }
        Ok(info)
    }
    fn events(&mut self) -> Result<(), Error> {
        // Drain a bounded batch; input/transport cannot be starved by damage.
        for _ in 0..4096 {
            let Some(event) = self.connection.poll_for_event()? else {
                break;
            };
            self.popup_event(&event);
            if matches!(
                &event,
                Event::MapNotify(_)
                    | Event::UnmapNotify(_)
                    | Event::ConfigureNotify(_)
                    | Event::DestroyNotify(_)
                    | Event::ReparentNotify(_)
                    | Event::PropertyNotify(_)
            ) {
                *self.menu_cache.borrow_mut() = None;
            }
            if let Some(s) = &mut self.selected {
                match event {
                    Event::DamageNotify(e) if e.damage == s.damage => s.dirty = true,
                    Event::ConfigureNotify(e) if e.window == s.id => {
                        s.dirty = true;
                        s.metadata_dirty = true;
                    }
                    Event::PropertyNotify(e) if e.window == s.id => s.metadata_dirty = true,
                    Event::DestroyNotify(e) if e.window == s.id => s.destroyed = true,
                    Event::UnmapNotify(e) if e.window == s.id => s.dirty = true,
                    Event::MapNotify(e) if e.window == s.id => {
                        s.dirty = true;
                        s.rebind = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn frame(&mut self) -> Result<wire::Reply, Error> {
        self.events()?;
        if self.selected.as_ref().is_some_and(|s| s.metadata_dirty) {
            let info = self.info(self.selected.as_ref().unwrap().id)?;
            let s = self.selected.as_mut().unwrap();
            s.info = info;
            s.metadata_dirty = false;
        }
        let s = self.selected.as_ref().ok_or("No application selected")?;
        if s.destroyed {
            let _ = self.clear();
            return Err("The application window was closed".into());
        }
        let id = s.id;
        let info = s.info.clone();
        if let Some(detached) = &self.detached {
            detached.resize_holder(info.width, info.height)?;
        }
        let mut attrs = self.connection.get_window_attributes(id)?.reply()?;
        if attrs.map_state != MapState::VIEWABLE {
            let iconic = self.values(id, b"WM_STATE")?.first() == Some(&3);
            if self.detached.is_none() && iconic {
                self.release_input()?;
                self.clear_popups();
                self.detached = Some(detach::Detached::new(
                    self.connection.clone(),
                    self.root,
                    id,
                    &info,
                )?);
                if let Some(selected) = &mut self.selected {
                    selected.rebind = true;
                    selected.dirty = true;
                }
                attrs = self.connection.get_window_attributes(id)?.reply()?;
            }
            if attrs.map_state != MapState::VIEWABLE {
                return Err("The application is not available for live capture".into());
            }
        }
        let visual = self.connection.setup().roots[0]
            .allowed_depths
            .iter()
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.visual_id == attrs.visual)
            .ok_or("Application visual is unavailable")?;
        if visual.class != VisualClass::TRUE_COLOR
            || (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0xff00, 0xff)
        {
            return Err("Application uses an unsupported colour visual".into());
        }
        let popups = self.capture_popups(id)?;
        let s = self.selected.as_mut().unwrap();
        let popups_changed = s.popups != popups;
        if !s.dirty && s.size == (info.width, info.height) {
            if !popups_changed {
                return Ok(wire::Reply::Idle);
            }
            let base = s.sequence;
            s.sequence = s
                .sequence
                .checked_add(1)
                .ok_or("Window sequence exhausted")?;
            s.popups = popups.clone();
            return Ok(wire::Reply::Frame {
                window: id,
                sequence: s.sequence,
                base: Some(base),
                width: info.width,
                height: info.height,
                tiles: Vec::new(),
                popups,
            });
        }
        let (tiles, base) = self.capture_tiles()?;
        let s = self.selected.as_mut().unwrap();
        s.dirty = false;
        if tiles.is_empty() && !popups_changed {
            return Ok(wire::Reply::Idle);
        }
        s.popups = popups.clone();
        s.sequence = s
            .sequence
            .checked_add(1)
            .ok_or("Window sequence exhausted")?;
        Ok(wire::Reply::Frame {
            window: id,
            sequence: s.sequence,
            base,
            width: info.width,
            height: info.height,
            tiles,
            popups,
        })
    }
    fn active(&self) -> Result<u32, Error> {
        Ok(self
            .values(self.root, b"_NET_ACTIVE_WINDOW")?
            .first()
            .copied()
            .unwrap_or(0))
    }
    fn focus(&mut self) -> Result<(), Error> {
        let id = self.selected.as_ref().ok_or("No application selected")?.id;
        if self.application_has_focus()? {
            return Ok(());
        }
        self.release_input()?;
        if self.detached.is_some() {
            self.connection
                .set_input_focus(InputFocus::PARENT, id, x11rb::CURRENT_TIME)?
                .check()?;
            self.connection.sync()?;
            return if self.application_has_focus()? {
                Ok(())
            } else {
                Err(FocusChanged.into())
            };
        }
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, id, self.atom(b"_NET_ACTIVE_WINDOW")?, [2, 0, 0, 0, 0]),
            )?
            .check()?;
        self.connection.flush()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        while !self.application_has_focus()? {
            if Instant::now() >= deadline {
                return Err(FocusChanged.into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn require_focus(&mut self) -> Result<u32, Error> {
        let id = self.selected.as_ref().ok_or("No application selected")?.id;
        // XFWM can finish its old iconify/focus transaction after the hidden
        // client was remapped. Fresh input from the independent native view
        // may focus this owned client directly, without raising a desktop app
        // or dropping a modifier the user is still holding.
        if self.detached.is_some() && !self.application_has_focus()? {
            self.connection
                .set_input_focus(InputFocus::PARENT, id, x11rb::CURRENT_TIME)?
                .check()?;
            self.connection.sync()?;
        }
        if !self.application_has_focus()? {
            self.release_input()?;
            return Err(FocusChanged.into());
        }
        Ok(id)
    }
    fn application_has_focus(&self) -> Result<bool, Error> {
        let id = self.selected.as_ref().ok_or("No application selected")?.id;
        let active = self.active()?;
        let focused = self.connection.get_input_focus()?.reply()?.focus;
        if active == id && (focused == id || (focused <= 1 && self.detached.is_none())) {
            return Ok(true);
        }
        // A toolkit may focus a menu (or a child of it) while its popup grab
        // owns input. Discover current, ownership-checked menus rather than
        // trusting the previous captured frame or accepting any same-PID window.
        let menus = self.popup_candidates(id)?;
        let belongs = |mut window: u32| -> Result<bool, Error> {
            for _ in 0..32 {
                if window == id || menus.iter().any(|m| m.0 == window) {
                    return Ok(true);
                }
                if window <= 1 || window == self.root {
                    break;
                }
                let Ok(tree) = self.connection.query_tree(window)?.reply() else {
                    break;
                };
                window = tree.parent;
            }
            Ok(false)
        };
        // A genuinely different active application always wins, even if an old
        // menu from the selected app is still mapped.
        if self.detached.is_none() && active != 0 && !belongs(active)? {
            return Ok(false);
        }
        if focused > 1 {
            return belongs(focused);
        }
        // PointerRoot/None is used during some legacy menu grabs. Require an
        // actual pointer target in the currently validated family, not merely
        // a cached menu or an unset active-window property.
        if (active == 0 || self.detached.is_some()) && !menus.is_empty() {
            let mut target = self.root;
            for _ in 0..32 {
                let pointer = self.connection.query_pointer(target)?.reply()?;
                target = pointer.child;
                if target == 0 {
                    break;
                }
                if belongs(target)? {
                    return Ok(true);
                }
            }
        }
        Ok(self.detached.is_none() && (active == id || menus.iter().any(|m| m.0 == active)))
    }
    fn motion(&mut self, x: i32, y: i32) -> Result<(), Error> {
        let id = self.require_focus()?;
        if self.selected.as_ref().is_some_and(|s| s.metadata_dirty) {
            let info = self.info(id)?;
            let selected = self.selected.as_mut().unwrap();
            selected.info = info;
            selected.metadata_dirty = false;
        }
        let info = self.selected.as_ref().unwrap().info.clone();
        let in_client = x >= 0 && y >= 0 && x < info.width as i32 && y < info.height as i32;
        let in_menu = !in_client
            && self
                .popup_candidates(id)?
                .iter()
                .any(|&(_, px, py, width, height)| {
                    i64::from(x) >= i64::from(px)
                        && i64::from(y) >= i64::from(py)
                        && i64::from(x) < i64::from(px) + i64::from(width)
                        && i64::from(y) < i64::from(py) + i64::from(height)
                });
        if !(in_client || in_menu) || i16::try_from(x).is_err() || i16::try_from(y).is_err() {
            return Err("Pointer is outside the application and its menus".into());
        }
        let point = self
            .connection
            .translate_coordinates(id, self.root, x as i16, y as i16)?
            .reply()?;
        let root_size = self.connection.get_geometry(self.root)?.reply()?;
        if point.dst_x < 0
            || point.dst_y < 0
            || i32::from(point.dst_x) >= i32::from(root_size.width)
            || i32::from(point.dst_y) >= i32::from(root_size.height)
        {
            return Err("Move the original application fully inside the remote desktop before using this part of it".into());
        }
        xtest::fake_input(
            &self.connection,
            MOTION_NOTIFY_EVENT,
            0,
            0,
            self.root,
            point.dst_x,
            point.dst_y,
            0,
        )?
        .check()?;
        Ok(())
    }
    pub(crate) fn handle(&mut self, request: wire::Request) -> Result<wire::Reply, Error> {
        self.events()?;
        let before_keys = self.keys.clone();
        let before_buttons = self.buttons.clone();
        // The private holder never intercepts desktop mouse input. Expose its
        // input shape only for an atomic synthetic-input transaction. Other
        // X11 clients cannot receive an accidental click in that interval.
        let _input = if self.detached.is_some()
            && matches!(
                &request,
                wire::Request::Focus
                    | wire::Request::Key { .. }
                    | wire::Request::Move { .. }
                    | wire::Request::Button { .. }
                    | wire::Request::Wheel { .. }
            ) {
            let mut regions = vec![self.detached.as_ref().unwrap().input()];
            regions.extend(
                self.popups
                    .values()
                    .filter_map(|popup| popup.hidden_input()),
            );
            Some(detach::InputGuard::new(
                self.connection.clone(),
                regions,
                self.detached.as_ref().unwrap().viewport()?,
            )?)
        } else {
            None
        };
        let mut wheel_released = Vec::new();
        let result = self.handle_request(request, &mut wheel_released);
        drop(_input);
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                let keys: Vec<_> = before_keys.union(&self.keys).copied().collect();
                let buttons: Vec<_> = before_buttons.union(&self.buttons).copied().collect();
                if let Err(cleanup) = self
                    .release_input()
                    .and_then(|_| self.wait_released(&keys, &buttons))
                {
                    tracing::warn!(%cleanup, "Application input cleanup did not finish");
                }
                return Err(error);
            }
        };
        let keys: Vec<_> = before_keys.difference(&self.keys).copied().collect();
        let mut buttons: Vec<_> = before_buttons.difference(&self.buttons).copied().collect();
        buttons.extend(wheel_released);
        self.connection.flush()?;
        self.wait_released(&keys, &buttons)?;
        Ok(reply)
    }
    fn handle_request(
        &mut self,
        request: wire::Request,
        wheel_released: &mut Vec<u8>,
    ) -> Result<wire::Reply, Error> {
        use wire::Request::*;
        match request {
            Stream { .. } => {}
            List => return Ok(wire::Reply::Windows(self.list()?)),
            Select(id) => return Ok(wire::Reply::Selected(self.select(id)?)),
            Poll => return self.frame(),
            Focus => self.focus()?,
            Resize { width, height } => {
                wire::dimensions(width, height)?;
                let id = self.selected.as_ref().ok_or("No application selected")?.id;
                if let Some(detached) = &self.detached {
                    detached.resize_holder(width, height)?;
                    self.connection
                        .configure_window(
                            id,
                            &ConfigureWindowAux::new().width(width).height(height),
                        )?
                        .check()?;
                } else {
                    self.connection
                        .send_event(
                            false,
                            self.root,
                            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                            ClientMessageEvent::new(
                                32,
                                id,
                                self.atom(b"_NET_MOVERESIZE_WINDOW")?,
                                [1 | (3 << 10) | (2 << 12), 0, 0, width, height],
                            ),
                        )?
                        .check()?;
                }
                if let Some(s) = &mut self.selected {
                    s.dirty = true;
                }
            }
            Key { evdev, pressed } => {
                self.require_focus()?;
                let code = evdev
                    .checked_add(8)
                    .filter(|v| (8..=255).contains(v))
                    .ok_or("Unsupported application key")? as u8;
                if pressed {
                    if !self.keys.insert(code) {
                        return Ok(wire::Reply::Ack);
                    }
                } else if !self.keys.contains(&code) {
                    return Ok(wire::Reply::Ack);
                }
                xtest::fake_input(
                    &self.connection,
                    if pressed {
                        KEY_PRESS_EVENT
                    } else {
                        KEY_RELEASE_EVENT
                    },
                    code,
                    0,
                    self.root,
                    0,
                    0,
                    0,
                )?
                .check()?;
                if !pressed {
                    self.keys.remove(&code);
                }
            }
            Move { x, y } => self.motion(x, y)?,
            Button {
                button,
                pressed,
                x,
                y,
            } => {
                self.motion(x, y)?;
                let code = x_button(button)?;
                if pressed {
                    self.buttons.insert(code);
                } else if !self.buttons.contains(&code) {
                    return Ok(wire::Reply::Ack);
                }
                xtest::fake_input(
                    &self.connection,
                    if pressed {
                        BUTTON_PRESS_EVENT
                    } else {
                        BUTTON_RELEASE_EVENT
                    },
                    code,
                    0,
                    self.root,
                    0,
                    0,
                    0,
                )?
                .check()?;
                if !pressed {
                    self.buttons.remove(&code);
                }
            }
            Wheel {
                horizontal,
                vertical,
            } => {
                self.require_focus()?;
                for (index, value, positive, negative) in
                    [(0, vertical, 4, 5), (1, horizontal, 7, 6)]
                {
                    let steps = wheel_steps(&mut self.wheel[index], value);
                    for _ in 0..steps.abs() {
                        let code = if steps > 0 { positive } else { negative };
                        self.buttons.insert(code);
                        xtest::fake_input(
                            &self.connection,
                            BUTTON_PRESS_EVENT,
                            code,
                            0,
                            self.root,
                            0,
                            0,
                            0,
                        )?
                        .check()?;
                        xtest::fake_input(
                            &self.connection,
                            BUTTON_RELEASE_EVENT,
                            code,
                            0,
                            self.root,
                            0,
                            0,
                            0,
                        )?
                        .check()?;
                        self.buttons.remove(&code);
                        wheel_released.push(code);
                    }
                }
            }
            Release => self.release_input()?,
            Stop => self.clear()?,
            InputBatch(requests) => {
                if requests.is_empty() || requests.len() > 64 {
                    return Err("Invalid application input batch".into());
                }
                for request in requests {
                    if let Err(error) = self.handle(request.into()) {
                        let _ = self.release_input();
                        return Err(error);
                    }
                }
            }
            Launch(command) => {
                let runtime = self
                    .application_runtime
                    .as_ref()
                    .ok_or("Application launching is unavailable")?;
                enqueue_application(runtime, &command)?;
            }
        }
        Ok(wire::Reply::Ack)
    }
}
impl Drop for Mirror {
    fn drop(&mut self) {
        if let Err(error) = self.clear() {
            tracing::warn!(%error, "Independent window input cleanup failed");
        }
    }
}

/// Only TrueColor RGB masks used by this desktop are accepted. No YUV, palette,
/// lossy encoding or scaling is introduced. Unsupported layouts fail explicitly.
fn decode_pixels(
    width: u32,
    height: u32,
    bpp: u8,
    pad: u8,
    little: bool,
    data: &[u8],
) -> Result<Vec<u8>, Error> {
    let size = wire::dimensions(width, height)?;
    if !matches!(bpp, 24 | 32) || !matches!(pad, 8 | 16 | 32) {
        return Err("Unsupported application pixel layout".into());
    }
    let stride = (width as usize * bpp as usize).div_ceil(pad as usize) * (pad as usize / 8);
    if data.len() != stride * height as usize {
        return Err("Incomplete application capture".into());
    }
    let bytes = bpp as usize / 8;
    let mut output = vec![0; size];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let source = &data[y * stride + x * bytes..y * stride + (x + 1) * bytes];
            let value = if little {
                u32::from_le_bytes([
                    source[0],
                    source[1],
                    source[2],
                    if bytes == 4 { source[3] } else { 0 },
                ])
            } else if bytes == 4 {
                u32::from_be_bytes(source.try_into().unwrap())
            } else {
                u32::from_be_bytes([0, source[0], source[1], source[2]])
            };
            output[(y * width as usize + x) * 4..(y * width as usize + x + 1) * 4]
                .copy_from_slice(&[value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]);
        }
    }
    Ok(output)
}

// Tokio's blocking stdin adapter cannot cancel an abandoned read. Read the
// actual SSH pipes using readiness so the lease also expires with stdin open.
struct Pipe(AsyncFd<OwnedFd>);
impl Pipe {
    fn new(fd: i32) -> Result<Self, Error> {
        let duplicate = unsafe { libc::dup(fd) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(duplicate) };
        let flags = unsafe { libc::fcntl(duplicate, libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(duplicate, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(AsyncFd::new(owned)?))
    }
    async fn read_exact(&self, mut bytes: &mut [u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            let mut ready = self.0.readable().await?;
            let result = ready.try_io(|fd| {
                let count = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                    )
                };
                if count < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            });
            match result {
                Ok(Ok(0)) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(Ok(n)) => bytes = &mut bytes[n..],
                Ok(Err(error)) => return Err(error),
                Err(_) => {}
            }
        }
        Ok(())
    }
    async fn write_all(&self, mut bytes: &[u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            let mut ready = self.0.writable().await?;
            let result = ready.try_io(|fd| {
                let count = unsafe {
                    libc::write(fd.get_ref().as_raw_fd(), bytes.as_ptr().cast(), bytes.len())
                };
                if count < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            });
            match result {
                Ok(Ok(0)) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(Ok(n)) => bytes = &bytes[n..],
                Ok(Err(error)) => return Err(error),
                Err(_) => {}
            }
        }
        Ok(())
    }
}

pub async fn serve(session: &str) -> Result<(), Error> {
    updates::serve(session).await
}
#[cfg(test)]
pub(crate) fn verify_event_stream(session: &str, connection: &RustConnection, window: u32) {
    updates::verify_stream(session, connection, window);
}

#[cfg(test)]
pub(crate) fn verify_existing_window(
    connection: RustConnection,
    controller: &RustConnection,
    window: u32,
) {
    let mut mirror = Mirror::from_connection(connection).unwrap();
    let parent = controller
        .query_tree(window)
        .unwrap()
        .reply()
        .unwrap()
        .parent;
    assert!(
        matches!(mirror.handle(wire::Request::List).unwrap(),wire::Reply::Windows(windows) if windows.iter().any(|w|w.id==window))
    );
    let selected = mirror.handle(wire::Request::Select(window)).unwrap();
    let wire::Reply::Selected(info) = selected else {
        panic!("application was not selected")
    };
    let gc = controller.generate_id().unwrap();
    controller
        .create_gc(gc, window, &CreateGCAux::new())
        .unwrap()
        .check()
        .unwrap();
    for (index, colour) in [0xff0000, 0x00ff00, 0x0000ff].into_iter().enumerate() {
        controller
            .change_gc(gc, &ChangeGCAux::new().foreground(colour))
            .unwrap()
            .check()
            .unwrap();
        controller
            .poly_fill_rectangle(
                window,
                gc,
                &[x11rb::protocol::xproto::Rectangle {
                    x: index as i16 * 30,
                    y: 0,
                    width: 30,
                    height: 60,
                }],
            )
            .unwrap()
            .check()
            .unwrap();
    }
    let cover = controller.generate_id().unwrap();
    controller
        .create_window(
            0,
            cover,
            mirror.root,
            0,
            0,
            640,
            480,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x202020),
        )
        .unwrap()
        .check()
        .unwrap();
    controller.map_window(cover).unwrap().check().unwrap();
    controller
        .configure_window(
            cover,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )
        .unwrap()
        .check()
        .unwrap();
    controller.flush().unwrap();
    let wire::Reply::Frame {
        sequence,
        base,
        width,
        height,
        tiles,
        ..
    } = mirror.handle(wire::Request::Poll).unwrap()
    else {
        panic!("initial mirrored image missing")
    };
    let pixels = wire::apply_frame(None, sequence, base, width, height, tiles).unwrap();
    assert_eq!((width, height), (info.width, info.height));
    for (x, expected) in [
        (10, [0, 0, 255, 255]),
        (40, [0, 255, 0, 255]),
        (70, [255, 0, 0, 255]),
    ] {
        let offset = ((10 * width + x) * 4) as usize;
        assert_eq!(
            &pixels[offset..offset + 4],
            &expected,
            "covered application lost its exact colours"
        );
    }
    controller
        .change_gc(gc, &ChangeGCAux::new().foreground(0x112233))
        .unwrap()
        .check()
        .unwrap();
    controller
        .poly_fill_rectangle(
            window,
            gc,
            &[x11rb::protocol::xproto::Rectangle {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            }],
        )
        .unwrap()
        .check()
        .unwrap();
    controller.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match mirror.handle(wire::Request::Poll).unwrap() {
            wire::Reply::Frame {
                sequence: next,
                base,
                width: w,
                height: h,
                tiles,
                ..
            } => {
                let updated = wire::apply_frame(
                    Some((sequence, width, height, &pixels)),
                    next,
                    base,
                    w,
                    h,
                    tiles,
                )
                .unwrap();
                assert_eq!(&updated[..4], &[0x33, 0x22, 0x11, 255]);
                break;
            }
            wire::Reply::Idle => {}
            other => panic!("unexpected capture response: {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "DAMAGE did not refresh the covered application"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.destroy_window(cover).unwrap().check().unwrap();
    for (width, height) in [(220, 120), (info.width, info.height)] {
        mirror
            .handle(wire::Request::Resize { width, height })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
            if (u32::from(geometry.width), u32::from(geometry.height)) == (width, height) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "window manager did not apply application resize"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let wire::Reply::Frame {
            base,
            width: w,
            height: h,
            tiles,
            sequence,
            ..
        } = mirror.handle(wire::Request::Poll).unwrap()
        else {
            panic!("resize did not refresh the mirror")
        };
        assert_eq!((w, h), (width, height));
        assert!(base.is_none());
        wire::apply_frame(None, sequence, base, w, h, tiles).unwrap();
    }
    capture::verify_partial(&mut mirror, controller, window, gc, info.width, info.height);
    popups::verify_menus(&mut mirror, controller, window);
    mirror.handle(wire::Request::Focus).unwrap();
    let baseline = controller.query_keymap().unwrap().reply().unwrap();
    for code in [38, 56] {
        assert_eq!(baseline.keys[code / 8] & (1 << (code % 8)), 0);
    }
    // Another client owns this unrelated key. Mirror cleanup must leave it
    // alone, including repeated Release, Stop and the helper's Drop path.
    xtest::fake_input(controller, KEY_PRESS_EVENT, 56, 0, mirror.root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    let root = mirror.root;
    let assert_input = |owned_down: bool, button_down: bool| {
        // Query through the other X11 connection immediately after Ack. There
        // is deliberately no sleep, retry, or mirror-side query in this check.
        let keys = controller.query_keymap().unwrap().reply().unwrap().keys;
        assert_eq!(
            keys[38 / 8] & (1 << (38 % 8)) != 0,
            owned_down,
            "mirror left its injected key held or failed to press it"
        );
        assert_ne!(
            keys[56 / 8] & (1 << (56 % 8)),
            0,
            "mirror released a key owned by another client"
        );
        let mask = controller
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .mask;
        assert_eq!(
            u16::from(mask) & u16::from(KeyButMask::BUTTON1) != 0,
            button_down,
            "mirror left its injected button held or failed to press it"
        );
    };
    let press_owned = |mirror: &mut Mirror| {
        mirror
            .handle(wire::Request::InputBatch(vec![
                wire::WindowInput::Key {
                    evdev: 30,
                    pressed: true,
                },
                wire::WindowInput::Button {
                    button: 1,
                    pressed: true,
                    x: 10,
                    y: 10,
                },
            ]))
            .unwrap();
    };
    for _ in 0..8 {
        press_owned(&mut mirror);
        assert_input(true, true);
        assert!(matches!(
            mirror.handle(wire::Request::Release).unwrap(),
            wire::Reply::Ack
        ));
        assert_input(false, false);
        mirror.handle(wire::Request::Release).unwrap(); // idempotent
        assert_input(false, false);
    }
    // A real synchronous WM grab can defer device processing beyond XSync.
    // Release must not acknowledge success before the host unfreezes it.
    press_owned(&mut mirror);
    let grabbed = controller
        .grab_pointer(
            false,
            window,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE,
            GrabMode::SYNC,
            GrabMode::ASYNC,
            0u32,
            0u32,
            x11rb::CURRENT_TIME,
        )
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(grabbed.status, GrabStatus::SUCCESS);
    std::thread::scope(|scope| {
        let thaw = scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(40));
            controller
                .allow_events(Allow::ASYNC_POINTER, x11rb::CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
            controller
                .ungrab_pointer(x11rb::CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
            controller.sync().unwrap();
        });
        mirror.handle(wire::Request::Release).unwrap();
        assert_input(false, false);
        thaw.join().unwrap();
    });
    // Ordinary matching key/button release also waits for checked success.
    press_owned(&mut mirror);
    mirror
        .handle(wire::Request::Key {
            evdev: 30,
            pressed: false,
        })
        .unwrap();
    mirror
        .handle(wire::Request::Button {
            button: 1,
            pressed: false,
            x: 10,
            y: 10,
        })
        .unwrap();
    assert_input(false, false);
    assert!(mirror.handle(wire::Request::InputBatch(vec![])).is_err());
    assert!(mirror
        .handle(wire::Request::InputBatch(vec![
            wire::WindowInput::Release;
            65
        ]))
        .is_err());
    assert!(mirror
        .handle(wire::Request::InputBatch(vec![
            wire::WindowInput::Key {
                evdev: 30,
                pressed: true
            },
            wire::WindowInput::Key {
                evdev: u32::MAX,
                pressed: true
            },
        ]))
        .is_err());
    assert_input(false, false);
    press_owned(&mut mirror);
    assert!(matches!(
        mirror.handle(wire::Request::Stop).unwrap(),
        wire::Reply::Ack
    ));
    assert_input(false, false);
    mirror.handle(wire::Request::Select(window)).unwrap();
    mirror.handle(wire::Request::Focus).unwrap();
    press_owned(&mut mirror);
    assert_input(true, true);
    drop(mirror);
    assert_input(false, false);
    xtest::fake_input(controller, KEY_RELEASE_EVENT, 56, 0, root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    while controller.poll_for_event().unwrap().is_some() {}
    assert_eq!(
        controller
            .query_tree(window)
            .unwrap()
            .reply()
            .unwrap()
            .parent,
        parent
    );
    assert_eq!(
        controller
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .unwrap()
            .map_state,
        MapState::VIEWABLE
    );
    controller.free_gc(gc).unwrap().check().unwrap();
}

#[cfg(test)]
pub(crate) fn verify_minimized_window(
    connection: RustConnection,
    controller: &RustConnection,
    window: u32,
) {
    detach::verify_minimized(Arc::new(connection), controller, window);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_launcher_rejects_nonprivate_files_and_enqueues_atomic_commands() {
        use std::ffi::CString;
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let pipe = directory.path().join("application-launch.fifo");
        std::fs::write(&pipe, b"not a pipe").unwrap();
        assert!(enqueue_application(directory.path(), "xterm").is_err());
        std::fs::remove_file(&pipe).unwrap();
        let c = CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Hold a reader so the nonblocking writer cannot hang on a dead broker.
        let mut reader = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&pipe)
            .unwrap();
        for bad in ["", "xterm\nother", "xterm\r", "x\0"] {
            assert!(enqueue_application(directory.path(), bad).is_err());
        }
        assert!(enqueue_application(directory.path(), &"x".repeat(2049)).is_err());
        enqueue_application(directory.path(), "printf '%s' 'literal; argument'").unwrap();
        let mut buffer = [0u8; 4096];
        let n = reader.read(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"printf '%s' 'literal; argument'\n");
        std::fs::set_permissions(&pipe, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(enqueue_application(directory.path(), "xterm").is_err());
    }
    #[test]
    fn x11_wheel_fractions_accumulate_without_amplification_or_axis_leakage() {
        let mut axes = [0; 2];
        for value in [30, 30, 30] {
            assert_eq!(wheel_steps(&mut axes[0], value), 0);
        }
        assert_eq!(wheel_steps(&mut axes[1], -120), -1);
        assert_eq!(wheel_steps(&mut axes[0], 30), 1);
        assert_eq!(axes, [0; 2]);
        assert_eq!(wheel_steps(&mut axes[0], 60), 0);
        assert_eq!(wheel_steps(&mut axes[0], -60), 0);
        assert_eq!(wheel_steps(&mut axes[0], -240), -2);
        assert_eq!(wheel_steps(&mut axes[0], i32::MIN), -8);
        assert!(axes[0].abs() < 120);
        assert_eq!(wheel_steps(&mut axes[1], i32::MAX), 8);
        assert!(axes[1].abs() < 120);
    }
    #[test]
    fn window_capture_converts_padded_truecolour_without_channel_loss() {
        assert_eq!(
            decode_pixels(1, 2, 24, 32, true, &[2, 7, 255, 0, 9, 254, 1, 0]).unwrap(),
            vec![2, 7, 255, 255, 9, 254, 1, 255]
        );
        assert_eq!(
            decode_pixels(1, 1, 32, 32, false, &[0, 255, 7, 2]).unwrap(),
            vec![2, 7, 255, 255]
        );
        assert!(decode_pixels(1, 1, 16, 32, true, &[0; 4]).is_err());
        assert!(decode_pixels(2, 1, 32, 32, true, &[0; 4]).is_err());
    }
    #[tokio::test]
    async fn window_mirror_pipe_read_can_expire_without_closing_stdin() {
        use std::io::Write;
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let input = Pipe::new(reader.as_raw_fd()).unwrap();
        let mut bytes = [0; 4];
        assert!(
            tokio::time::timeout(Duration::from_millis(30), input.read_exact(&mut bytes))
                .await
                .is_err()
        );
        writer.write_all(b"test").unwrap();
        tokio::time::timeout(Duration::from_secs(1), input.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"test");
    }
}
