//! Mirror the real WM title strip, never a desktop/root screenshot. Windows'
//! own caption remains separate: stopping the view must not close its client.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Bounds {
    frame: u32,
    frame_width: u16,
    frame_height: u16,
    x: u32,
    width: u32,
    height: u32,
}
pub(super) struct Capture {
    connection: Arc<RustConnection>,
    bounds: Bounds,
    pixmap: u32,
    damage: u32,
    dirty: bool,
    bounds_dirty: bool,
    checked_at: Instant,
    last: Option<wire::Popup>,
}
impl Capture {
    pub(super) fn event(&mut self, event: &Event) {
        if matches!(event, Event::ConfigureNotify(e) if e.window == self.bounds.frame)
            || matches!(event, Event::ReparentNotify(e) if e.window == self.bounds.frame)
            || matches!(event, Event::DestroyNotify(e) if e.window == self.bounds.frame)
        {
            self.bounds_dirty = true;
        }
        if matches!(event, Event::DamageNotify(e) if e.damage == self.damage)
            || matches!(event, Event::ConfigureNotify(e) if e.window == self.bounds.frame)
            || matches!(event, Event::MapNotify(e) if e.window == self.bounds.frame)
        {
            self.dirty = true;
        }
    }
    fn new(connection: Arc<RustConnection>, bounds: Bounds) -> Result<Self, Error> {
        let mut capture = Self {
            pixmap: connection.generate_id()?,
            damage: connection.generate_id()?,
            connection,
            bounds,
            dirty: true,
            bounds_dirty: false,
            checked_at: Instant::now(),
            last: None,
        };
        composite::redirect_window(&capture.connection, bounds.frame, Redirect::AUTOMATIC)?
            .check()?;
        damage::create(
            &capture.connection,
            capture.damage,
            bounds.frame,
            ReportLevel::NON_EMPTY,
        )?
        .check()?;
        composite::name_window_pixmap(&capture.connection, bounds.frame, capture.pixmap)?
            .check()?;
        capture.connection.flush()?;
        capture.dirty = true;
        Ok(capture)
    }
    fn image(&mut self) -> Result<wire::Popup, Error> {
        if !self.dirty {
            if let Some(last) = &self.last {
                return Ok(last.clone());
            }
        }
        let b = self.bounds;
        damage::subtract(&self.connection, self.damage, 0u32, 0u32)?.check()?;
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.pixmap,
                b.x as i16,
                0,
                b.width as u16,
                b.height as u16,
                u32::MAX,
            )?
            .reply()?;
        let format = self
            .connection
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .ok_or("Title bar pixel format unavailable")?;
        let pixels = decode_pixels(
            b.width,
            b.height,
            format.bits_per_pixel,
            format.scanline_pad,
            self.connection.setup().image_byte_order == ImageOrder::LSB_FIRST,
            &image.data,
        )?;
        let mut tiles = Vec::new();
        for x in (0..b.width).step_by(256) {
            let region = Rectangle {
                x,
                y: 0,
                width: (b.width - x).min(256),
                height: b.height,
            };
            let mut bytes = Vec::with_capacity((region.width * region.height * 4) as usize);
            for y in 0..b.height {
                let start = ((y * b.width + x) * 4) as usize;
                bytes.extend_from_slice(&pixels[start..start + region.width as usize * 4]);
            }
            tiles.push(LosslessTile::encode(region, bytes)?);
        }
        let image = wire::Popup {
            window: b.frame,
            x: b.x as i32,
            y: 0,
            width: b.width,
            height: b.height,
            tiles,
        };
        wire::validate_titlebar(&image)?;
        self.last = Some(image.clone());
        self.dirty = false;
        Ok(image)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.connection.free_pixmap(self.pixmap);
        let _ = damage::destroy(&self.connection, self.damage);
        let _ =
            composite::unredirect_window(&self.connection, self.bounds.frame, Redirect::AUTOMATIC);
        let _ = self.connection.flush();
    }
}
fn expired(error: &Error) -> bool {
    // Every queried resource in this capture path is the selected client, an
    // ancestor derived from QueryTree, or our own named pixmap. WM reparenting
    // can retire any of these between requests. Check the client independently
    // before deciding whether to retire only the strip or the whole view.
    matches!(error.downcast_ref::<x11rb::errors::ReplyError>(),
        Some(x11rb::errors::ReplyError::X11Error(e))
            if matches!(e.error_kind, x11rb::protocol::ErrorKind::Window
                | x11rb::protocol::ErrorKind::Drawable | x11rb::protocol::ErrorKind::Pixmap))
}
impl Mirror {
    fn titlebar_bounds(&self) -> Result<Option<Bounds>, Error> {
        let id = self.selected.as_ref().ok_or("No application selected")?.id;
        let geometry = self.connection.get_geometry(id)?.reply()?;
        let mut frame = id;
        for _ in 0..32 {
            let tree = self.connection.query_tree(frame)?.reply()?;
            if tree.parent == self.root {
                if frame == id {
                    return Ok(None);
                }
                let attrs = self.connection.get_window_attributes(frame)?.reply()?;
                if attrs.map_state != MapState::VIEWABLE {
                    return Ok(None);
                }
                let visual = self.connection.setup().roots[0]
                    .allowed_depths
                    .iter()
                    .flat_map(|d| d.visuals.iter())
                    .find(|v| v.visual_id == attrs.visual);
                if !visual.is_some_and(|v| {
                    v.class == VisualClass::TRUE_COLOR
                        && (v.red_mask, v.green_mask, v.blue_mask) == (0xff0000, 0xff00, 0xff)
                }) {
                    return Ok(None);
                }
                let outer = self.connection.get_geometry(frame)?.reply()?;
                let offset = self
                    .connection
                    .translate_coordinates(id, frame, 0, 0)?
                    .reply()?;
                let width = u32::from(geometry.width);
                if offset.dst_x < 0
                    || offset.dst_x > 1024
                    || offset.dst_y <= 0
                    || offset.dst_y as u32 > wire::MAX_TITLEBAR_HEIGHT
                    || offset.dst_x as u32 + width > u32::from(outer.width)
                    || offset.dst_y as u32 > u32::from(outer.height)
                {
                    return Ok(None);
                }
                // Validate before naming/reading the frame's pixmap.
                if wire::dimensions(width, offset.dst_y as u32).is_err() {
                    return Ok(None);
                }
                return Ok(Some(Bounds {
                    frame,
                    frame_width: outer.width,
                    frame_height: outer.height,
                    x: offset.dst_x as u32,
                    width,
                    height: offset.dst_y as u32,
                }));
            }
            if tree.parent == 0 || tree.parent == frame {
                return Ok(None);
            }
            frame = tree.parent;
        }
        Ok(None)
    }
    pub(super) fn titlebar_image(&mut self) -> Result<Option<wire::Popup>, Error> {
        if !self.want_titlebar {
            return Ok(None);
        }
        if self.detached.is_some() {
            // The WM frame was withdrawn, while the independently hosted client
            // remains live. Preserve its last strip; controls restore its real
            // WM frame before sending a click. Never click a stale frame XID.
            return Ok(self
                .titlebar
                .as_ref()
                .and_then(|c| c.last.clone())
                .filter(|p| {
                    self.selected
                        .as_ref()
                        .is_some_and(|s| s.info.width == p.width)
                }));
        }
        let result = (|| -> Result<Option<wire::Popup>, Error> {
            // Application repainting must not add a chain of synchronous
            // geometry queries to every frame. Invalidate on WM/client events,
            // with a short fallback check for missed reparent/resize events.
            if self.titlebar.as_ref().is_some_and(|c| {
                !c.bounds_dirty && c.checked_at.elapsed() < Duration::from_millis(250)
            }) && self
                .selected
                .as_ref()
                .is_some_and(|s| !s.metadata_dirty && !s.rebind)
            {
                return Ok(Some(self.titlebar.as_mut().unwrap().image()?));
            }
            let Some(bounds) = self.titlebar_bounds()? else {
                // Iconification and WM withdrawal can precede frame() taking
                // over the client. Retain the existing strip through this gap.
                let id = self.selected.as_ref().ok_or("No application selected")?.id;
                if self
                    .connection
                    .get_window_attributes(id)?
                    .reply()?
                    .map_state
                    != MapState::VIEWABLE
                {
                    return Ok(self.titlebar.as_ref().and_then(|c| c.last.clone()));
                }
                self.titlebar.take();
                return Ok(None);
            };
            if self.titlebar.as_ref().is_none_or(|c| c.bounds != bounds) {
                self.titlebar.take();
                self.titlebar = Some(Capture::new(self.connection.clone(), bounds)?);
            }
            let capture = self.titlebar.as_mut().unwrap();
            capture.bounds_dirty = false;
            capture.checked_at = Instant::now();
            Ok(Some(self.titlebar.as_mut().unwrap().image()?))
        })();
        match result {
            Err(error) if expired(&error) => {
                self.titlebar.take();
                if let Some(selected) = &self.selected {
                    self.connection
                        .get_window_attributes(selected.id)?
                        .reply()?;
                }
                Ok(None)
            }
            result => result,
        }
    }
    fn check_titlebar_target(&self, window: u32) -> Result<(), Error> {
        if !self.want_titlebar
            || !self
                .selected
                .as_ref()
                .is_some_and(|s| s.id == window && !s.destroyed)
        {
            return Err("Application title bar no longer belongs to this view".into());
        }
        Ok(())
    }
    pub(super) fn close_application(&mut self, window: u32) -> Result<(), Error> {
        self.check_titlebar_target(window)?;
        self.release_input()?;
        let delete = self.atom(b"WM_DELETE_WINDOW")?;
        if !self.values(window, b"WM_PROTOCOLS")?.contains(&delete) {
            return Err("This application does not support a normal close request. Close it from its own menu.".into());
        }
        // Deliberately no XKillClient/DestroyWindow/process termination fallback.
        // The application decides whether to show Save/Cancel and remain open.
        self.connection
            .send_event(
                false,
                window,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(
                    32,
                    window,
                    self.atom(b"WM_PROTOCOLS")?,
                    [delete, x11rb::CURRENT_TIME, 0, 0, 0],
                ),
            )?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }
    pub(super) fn titlebar_click(&mut self, window: u32, x: u32, y: u32) -> Result<(), Error> {
        self.check_titlebar_target(window)?;
        let displayed = self
            .titlebar
            .as_ref()
            .and_then(|c| c.last.as_ref())
            .ok_or("Application title bar is not ready")?;
        if x >= displayed.width || y >= displayed.height {
            return Err("Click is outside the application title bar".into());
        }
        let displayed_size = (displayed.width, displayed.height);
        self.release_input()?;
        if self.detached.is_some() {
            self.clear_popups();
            drop(self.detached.take());
            if let Some(s) = &mut self.selected {
                s.rebind = true;
                s.dirty = true;
                s.metadata_dirty = true;
            }
        }
        let deadline = Instant::now() + Duration::from_millis(750);
        let bounds = loop {
            if let Some(bounds) = self.titlebar_bounds()? {
                break bounds;
            }
            if Instant::now() >= deadline {
                return Err("The host title bar is not available yet".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if (bounds.width, bounds.height) != displayed_size {
            self.titlebar.take();
            return Err(
                "The host title bar changed size. Click its refreshed control again.".into(),
            );
        }
        self.focus()?;
        let origin = self
            .connection
            .translate_coordinates(bounds.frame, self.root, 0, 0)?
            .reply()?;
        let px = i32::from(origin.dst_x) + bounds.x as i32 + x as i32;
        let py = i32::from(origin.dst_y) + y as i32;
        let desktop = self.connection.get_geometry(self.root)?.reply()?;
        if px < 0 || py < 0 || px >= i32::from(desktop.width) || py >= i32::from(desktop.height) {
            return Err("Move the original application title bar inside the remote desktop before using this control".into());
        }
        let px = i16::try_from(px).map_err(|_| "Title bar is outside the desktop")?;
        let py = i16::try_from(py).map_err(|_| "Title bar is outside the desktop")?;
        xtest::fake_input(
            &self.connection,
            MOTION_NOTIFY_EVENT,
            0,
            0,
            self.root,
            px,
            py,
            0,
        )?
        .check()?;
        // Verify actual hit ownership after activation/raising, not merely the
        // old picture's coordinates. Another app or its modal dialog must win.
        let hit = self.connection.query_pointer(self.root)?.reply()?;
        if hit.child != bounds.frame {
            return Err(FocusChanged.into());
        }
        self.buttons.insert(1);
        xtest::fake_input(
            &self.connection,
            BUTTON_PRESS_EVENT,
            1,
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
            1,
            0,
            self.root,
            0,
            0,
            0,
        )?
        .check()?;
        self.buttons.remove(&1);
        self.wait_released(&[], &[1])?;
        self.connection.flush()?;
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn verify(connection: RustConnection, controller: &RustConnection, window: u32) {
    let mut mirror = Mirror::from_connection(connection).unwrap();
    mirror
        .handle(wire::Request::Titlebar { enabled: true })
        .unwrap();
    mirror.select(window).unwrap();
    let delete = mirror.atom(b"WM_DELETE_WINDOW").unwrap();
    let protocols = mirror.atom(b"WM_PROTOCOLS").unwrap();
    controller
        .change_property32(
            PropMode::REPLACE,
            window,
            protocols,
            AtomEnum::ATOM,
            &[delete],
        )
        .unwrap()
        .check()
        .unwrap();
    let image = mirror
        .titlebar_image()
        .unwrap()
        .expect("real XFWM title bar missing");
    wire::validate_titlebar(&image).unwrap();
    let pixels = wire::apply_frame(
        None,
        1,
        None,
        image.width,
        image.height,
        image.tiles.clone(),
    )
    .unwrap();
    assert!(
        pixels.chunks_exact(4).any(|p| p != &pixels[..4]),
        "WM decorations were blank"
    );
    assert_eq!(
        mirror.titlebar_image().unwrap().unwrap(),
        image,
        "unchanged caption must reuse its encoded strip"
    );
    assert!(mirror
        .handle(wire::Request::CloseApplication { window: window + 1 })
        .is_err());
    assert!(mirror
        .handle(wire::Request::TitlebarClick {
            window,
            x: image.width,
            y: 1
        })
        .is_err());
    let wait_close = || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(Event::ClientMessage(event)) = controller.poll_for_event().unwrap() {
                if event.window == window
                    && event.type_ == protocols
                    && event.data.as_data32()[0] == delete
                {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "host Close did not reach the actual client"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Simulate Cancel/unsaved-work handling by keeping this client alive.
        assert_eq!(
            controller
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .map_state,
            MapState::VIEWABLE
        );
    };
    // This is the actual theme's upper-right Close hit target, not a helper
    // process exit or an arbitrary application screenshot.
    mirror
        .handle(wire::Request::TitlebarClick {
            window,
            x: image.width - 12,
            y: image.height / 2,
        })
        .unwrap();
    wait_close();
    assert!(mirror.keys.is_empty() && mirror.buttons.is_empty());
    mirror
        .handle(wire::Request::CloseApplication { window })
        .unwrap();
    wait_close();
    // An original minimized on the desktop is still usable in the independent
    // view. Its retained caption must address the newly restored WM frame,
    // never the old frame XID that was destroyed during withdrawal.
    let iconic = mirror.atom(b"WM_CHANGE_STATE").unwrap();
    controller
        .send_event(
            false,
            mirror.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, window, iconic, [3, 0, 0, 0, 0]),
        )
        .unwrap()
        .check()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while controller
        .get_window_attributes(window)
        .unwrap()
        .reply()
        .unwrap()
        .map_state
        == MapState::VIEWABLE
    {
        assert!(
            Instant::now() < deadline,
            "WM did not minimize the caption fixture"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    mirror.frame().unwrap();
    assert!(mirror.detached.is_some());
    assert_eq!(mirror.titlebar_image().unwrap().unwrap(), image);
    mirror
        .handle(wire::Request::TitlebarClick {
            window,
            x: image.width - 12,
            y: image.height / 2,
        })
        .unwrap();
    wait_close();
    assert!(mirror.detached.is_none());
    mirror.frame().unwrap();
    mirror.handle(wire::Request::Stop).unwrap();
    assert!(
        mirror.list().unwrap().iter().any(|w| w.id == window),
        "stopping view closed its host client"
    );
}
