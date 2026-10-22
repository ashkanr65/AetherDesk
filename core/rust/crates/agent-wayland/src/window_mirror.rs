//! Opt-in X11 window mirror. Never changes the desktop compositing preference,
//! replaces the WM, reparents/unmaps a client or closes an application.
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
    time::{Duration, Instant},
};
use tokio::io::unix::AsyncFd;
use x11rb::{
    connection::Connection,
    protocol::{
        composite::{self, Redirect},
        damage::{self, ReportLevel},
        xproto::*,
        xtest, Event,
    },
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
};

#[path = "window_popups.rs"]
mod popups;
use popups::PopupCapture;

type Error = Box<dyn std::error::Error + Send + Sync>;
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
}
pub struct Mirror {
    connection: RustConnection,
    root: u32,
    selected: Option<Selected>,
    keys: BTreeSet<u8>,
    buttons: BTreeSet<u8>,
    popups: std::collections::BTreeMap<u32, PopupCapture>,
}

fn private(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.is_dir() != directory
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
        Self::from_connection(connection)
    }
    pub(crate) fn from_connection(connection: RustConnection) -> Result<Self, Error> {
        let root = connection.setup().roots[0].root;
        let version = composite::query_version(&connection, 0, 4)?.reply()?;
        if version.major_version == 0 && version.minor_version < 2 {
            return Err("Desktop lacks window capture support".into());
        }
        damage::query_version(&connection, 1, 1)?.reply()?;
        xtest::get_version(&connection, 2, 2)?.reply()?;
        Ok(Self {
            connection,
            root,
            selected: None,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            popups: std::collections::BTreeMap::new(),
        })
    }
    fn atom(&self, name: &[u8]) -> Result<u32, Error> {
        Ok(self.connection.intern_atom(false, name)?.reply()?.atom)
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
        let clients = self.values(self.root, b"_NET_CLIENT_LIST")?;
        Ok(clients
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
            .collect())
    }
    fn release_input(&mut self) -> Result<(), Error> {
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
        // flush() only writes to the socket. A same-connection server round
        // trip makes Release/Stop acknowledgements meaningful to other clients.
        if let Err(error) = connection.sync() {
            failure.get_or_insert_with(|| error.into());
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn clear(&mut self) -> Result<(), Error> {
        let input = self.release_input();
        self.clear_popups();
        if let Some(s) = self.selected.take() {
            let _ = self.connection.free_pixmap(s.pixmap);
            let _ = damage::destroy(&self.connection, s.damage);
            let _ = composite::unredirect_window(&self.connection, s.id, Redirect::AUTOMATIC);
            let _ = self.connection.flush();
        }
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
        });
        let prepared = (|| -> Result<(), Error> {
            self.connection
                .change_window_attributes(
                    id,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
                )?
                .check()?;
            damage::create(&self.connection, damage, id, ReportLevel::NON_EMPTY)?.check()?;
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
            if let Some(s) = &mut self.selected {
                match event {
                    Event::DamageNotify(e) if e.damage == s.damage => s.dirty = true,
                    Event::ConfigureNotify(e) if e.window == s.id => s.dirty = true,
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
        let s = self.selected.as_ref().ok_or("No application selected")?;
        if s.destroyed {
            let _ = self.clear();
            return Err("The application window was closed".into());
        }
        let id = s.id;
        let info = self.info(id)?;
        let attrs = self.connection.get_window_attributes(id)?.reply()?;
        if attrs.map_state != MapState::VIEWABLE {
            return Err("Restore the application in the remote desktop to continue viewing".into());
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
        if s.rebind || s.size != (info.width, info.height) {
            self.connection.free_pixmap(s.pixmap)?.check()?;
            s.pixmap = self.connection.generate_id()?;
            composite::name_window_pixmap(&self.connection, id, s.pixmap)?.check()?;
            s.size = (info.width, info.height);
            s.pixels.clear();
            s.rebind = false;
        }
        damage::subtract(&self.connection, s.damage, 0u32, 0u32)?.check()?;
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                s.pixmap,
                0,
                0,
                info.width as u16,
                info.height as u16,
                u32::MAX,
            )?
            .reply()?;
        let format = self
            .connection
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .ok_or("Unsupported window pixel format")?;
        let pixels = decode_pixels(
            info.width,
            info.height,
            format.bits_per_pixel,
            format.scanline_pad,
            self.connection.setup().image_byte_order == ImageOrder::LSB_FIRST,
            &image.data,
        )?;
        let base = (!s.pixels.is_empty()).then_some(s.sequence);
        let mut tiles = Vec::new();
        for y in (0..info.height).step_by(256) {
            for x in (0..info.width).step_by(256) {
                let region = Rectangle {
                    x,
                    y,
                    width: (info.width - x).min(256),
                    height: (info.height - y).min(256),
                };
                let mut bytes = Vec::with_capacity((region.width * region.height * 4) as usize);
                let mut changed = base.is_none();
                for row in y..y + region.height {
                    let start = ((row * info.width + x) * 4) as usize;
                    let end = start + (region.width * 4) as usize;
                    if !changed && pixels[start..end] != s.pixels[start..end] {
                        changed = true;
                    }
                    bytes.extend_from_slice(&pixels[start..end]);
                }
                if changed {
                    tiles.push(LosslessTile::encode(region, bytes)?);
                }
            }
        }
        s.dirty = false;
        if tiles.is_empty() && !popups_changed {
            return Ok(wire::Reply::Idle);
        }
        s.popups = popups.clone();
        s.sequence = s
            .sequence
            .checked_add(1)
            .ok_or("Window sequence exhausted")?;
        s.pixels = pixels;
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
        if self.active()? == id {
            return Ok(());
        }
        self.release_input()?;
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
        while self.active()? != id {
            if Instant::now() >= deadline {
                return Err("Activate the application in the remote desktop before typing".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn require_focus(&mut self) -> Result<u32, Error> {
        let id = self.selected.as_ref().ok_or("No application selected")?.id;
        if self.active()? != id {
            self.release_input()?;
            return Err(
                "Application focus changed; click its independent view to activate it again".into(),
            );
        }
        Ok(id)
    }
    fn motion(&mut self, x: i32, y: i32) -> Result<(), Error> {
        let id = self.require_focus()?;
        let info = self.info(id)?;
        let in_client = x >= 0 && y >= 0 && x < info.width as i32 && y < info.height as i32;
        let in_menu = self.selected.as_ref().is_some_and(|s| {
            s.popups.iter().any(|p| {
                i64::from(x) >= i64::from(p.x)
                    && i64::from(y) >= i64::from(p.y)
                    && i64::from(x) < i64::from(p.x) + i64::from(p.width)
                    && i64::from(y) < i64::from(p.y) + i64::from(p.height)
            })
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
        use wire::Request::*;
        match request {
            List => return Ok(wire::Reply::Windows(self.list()?)),
            Select(id) => return Ok(wire::Reply::Selected(self.select(id)?)),
            Poll => return self.frame(),
            Focus => self.focus()?,
            Resize { width, height } => {
                wire::dimensions(width, height)?;
                let id = self.selected.as_ref().ok_or("No application selected")?.id;
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
                    self.keys.insert(code);
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
                let code = match button {
                    1 => 1,
                    2 => 3,
                    3 => 2,
                    4 => 8,
                    5 => 9,
                    _ => return Err("Unsupported application button".into()),
                };
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
                for (value, positive, negative) in [(vertical, 4, 5), (horizontal, 7, 6)] {
                    let steps = ((i64::from(value).abs() + 119) / 120).min(8);
                    for _ in 0..steps {
                        let code = if value > 0 { positive } else { negative };
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
                    }
                }
            }
            Release => self.release_input()?,
            Stop => self.clear()?,
        }
        self.connection.flush()?;
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
    let input = Pipe::new(0)?;
    let output = Pipe::new(1)?;
    async fn send(output: &Pipe, reply: &wire::Reply) -> Result<(), Error> {
        let bytes = wire::encode(reply)?;
        tokio::time::timeout(Duration::from_secs(10), async {
            output.write_all(wire::MAGIC).await?;
            output
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await?;
            output.write_all(&bytes).await
        })
        .await??;
        Ok(())
    }
    let mut mirror = match Mirror::connect_session(session) {
        Ok(mirror) => mirror,
        Err(error) => {
            send(
                &output,
                &wire::Reply::Error(error.to_string().chars().take(240).collect()),
            )
            .await?;
            return Ok(());
        }
    };
    send(&output, &wire::Reply::Ready).await?;
    loop {
        let received = tokio::time::timeout(Duration::from_secs(15), async {
            let mut magic = [0; 8];
            input.read_exact(&mut magic).await?;
            if &magic != wire::MAGIC {
                return Err(std::io::Error::other("Invalid window stream identity"));
            }
            let mut length = [0; 4];
            input.read_exact(&mut length).await?;
            let size = u32::from_be_bytes(length) as usize;
            if size > 64 * 1024 {
                return Err(std::io::Error::other("Oversized window request"));
            }
            let mut bytes = vec![0; size];
            input.read_exact(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        })
        .await;
        let bytes = match received {
            Ok(Ok(bytes)) => bytes,
            _ => break,
        };
        let request: wire::Request = wire::decode(&bytes)?;
        let stop = matches!(request, wire::Request::Stop);
        let clear_on_error = matches!(request, wire::Request::Poll | wire::Request::Select(_));
        let reply = match mirror.handle(request) {
            Ok(reply) => reply,
            Err(error) => {
                if let Err(cleanup_error) = mirror.release_input() {
                    tracing::warn!(%cleanup_error, "Independent window input release failed");
                }
                if clear_on_error {
                    let _ = mirror.clear();
                }
                wire::Reply::Error(error.to_string().chars().take(240).collect())
            }
        };
        send(&output, &reply).await?;
        if stop {
            break;
        }
    }
    Ok(())
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
            .handle(wire::Request::Key {
                evdev: 30,
                pressed: true,
            })
            .unwrap();
        mirror
            .handle(wire::Request::Button {
                button: 1,
                pressed: true,
                x: 10,
                y: 10,
            })
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
mod tests {
    use super::*;
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
