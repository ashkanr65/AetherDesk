//! Keep an explicitly mirrored, iconified X11 client live without painting it
//! over the desktop. The server save-set restores it if this helper disappears.
use super::*;
use x11rb::protocol::shape::{self, SK};

const INDEX: &[u8] = b"_AETHERDESK_CAPTURE_WINDOWS_V1";
struct ServerLock(Arc<RustConnection>);
impl ServerLock {
    fn new(connection: Arc<RustConnection>) -> Result<Self, Error> {
        connection.grab_server()?.check()?;
        Ok(Self(connection))
    }
}
impl Drop for ServerLock {
    fn drop(&mut self) {
        let _ = self.0.ungrab_server();
        let _ = self.0.flush();
    }
}
fn records(connection: &RustConnection, root: u32, atom: u32) -> Result<Vec<[u32; 3]>, Error> {
    let reply = connection
        .get_property(
            false,
            root,
            atom,
            AtomEnum::WINDOW,
            0,
            (wire::MAX_WINDOWS * 3) as u32,
        )?
        .reply()?;
    if reply.bytes_after != 0 {
        return Err("Independent application directory exceeds its limit".into());
    }
    let values: Vec<_> = reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default();
    Ok(values
        .chunks_exact(3)
        .filter_map(|value| {
            let [parent, holder, client] = <[u32; 3]>::try_from(value).ok()?;
            let tree = connection.query_tree(holder).ok()?.reply().ok()?;
            let top = connection.query_tree(parent).ok()?.reply().ok()?;
            let attrs = connection
                .get_window_attributes(parent)
                .ok()?
                .reply()
                .ok()?;
            let app = connection.query_tree(client).ok()?.reply().ok()?;
            (tree.parent == parent
                && top.parent == root
                && attrs.override_redirect
                && app.parent == holder)
                .then_some([parent, holder, client])
        })
        .collect())
}
pub(super) fn listed_clients(connection: &RustConnection, root: u32) -> Result<Vec<u32>, Error> {
    let atom = connection.intern_atom(false, INDEX)?.reply()?.atom;
    Ok(records(connection, root, atom)?
        .into_iter()
        .map(|record| record[2])
        .collect())
}

pub(super) struct HiddenInput {
    connection: Arc<RustConnection>,
    pub window: u32,
    saved: u32,
    empty: u32,
    opacity: Option<(u32, Option<u32>)>,
}
impl HiddenInput {
    pub fn new(connection: Arc<RustConnection>, window: u32) -> Result<Self, Error> {
        let saved = connection.generate_id()?;
        let empty = connection.generate_id()?;
        let rectangles = shape::get_rectangles(&connection, window, SK::INPUT)?
            .reply()?
            .rectangles;
        xfixes::create_region(&connection, saved, &rectangles)?.check()?;
        if let Err(error) = xfixes::create_region(&connection, empty, &[])?.check() {
            let _ = xfixes::destroy_region(&connection, saved);
            return Err(error.into());
        }
        let mut state = Self {
            connection,
            window,
            saved,
            empty,
            opacity: None,
        };
        if composite::redirect_window(&state.connection, window, Redirect::MANUAL)?
            .check()
            .is_err()
        {
            // XFWM can already own manual redirection of root-level menus.
            // Its compositing opacity hides presentation, not backing pixels.
            let selection = state
                .connection
                .intern_atom(false, b"_NET_WM_CM_S0")?
                .reply()?
                .atom;
            if state
                .connection
                .get_selection_owner(selection)?
                .reply()?
                .owner
                == 0
            {
                return Err("The host compositor cannot hide this independent menu safely".into());
            }
            let atom = state
                .connection
                .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")?
                .reply()?
                .atom;
            let previous = state
                .connection
                .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?
                .reply()?
                .value32()
                .and_then(|mut values| values.next());
            state.opacity = Some((atom, previous));
            composite::redirect_window(&state.connection, window, Redirect::AUTOMATIC)?.check()?;
            state
                .connection
                .change_property32(PropMode::REPLACE, window, atom, AtomEnum::CARDINAL, &[0])?
                .check()?;
        }
        xfixes::set_window_shape_region(&state.connection, window, SK::INPUT, 0, 0, empty)?
            .check()?;
        state.connection.sync()?;
        Ok(state)
    }
    pub fn regions(&self) -> (u32, u32, u32) {
        (self.window, self.saved, self.empty)
    }
}
impl Drop for HiddenInput {
    fn drop(&mut self) {
        let _ = xfixes::set_window_shape_region(
            &self.connection,
            self.window,
            SK::INPUT,
            0,
            0,
            self.saved,
        );
        if let Some((atom, previous)) = self.opacity {
            if let Some(value) = previous {
                let _ = self.connection.change_property32(
                    PropMode::REPLACE,
                    self.window,
                    atom,
                    AtomEnum::CARDINAL,
                    &[value],
                );
            } else {
                let _ = self.connection.delete_property(self.window, atom);
            }
            let _ =
                composite::unredirect_window(&self.connection, self.window, Redirect::AUTOMATIC);
        } else {
            let _ = composite::unredirect_window(&self.connection, self.window, Redirect::MANUAL);
        }
        let _ = xfixes::destroy_region(&self.connection, self.saved);
        let _ = xfixes::destroy_region(&self.connection, self.empty);
        let _ = self.connection.flush();
    }
}

pub(super) struct InputGuard {
    connection: Arc<RustConnection>,
    regions: Vec<(u32, u32, u32)>,
    viewport: (u32, u32, u32),
}
impl InputGuard {
    pub fn new(
        connection: Arc<RustConnection>,
        regions: Vec<(u32, u32, u32)>,
        viewport: (u32, u32, u32),
    ) -> Result<Self, Error> {
        connection.grab_server()?.check()?;
        let guard = Self {
            connection,
            regions,
            viewport,
        };
        guard
            .connection
            .configure_window(
                viewport.0,
                &ConfigureWindowAux::new()
                    .width(viewport.1 + 1)
                    .height(viewport.2 + 1),
            )?
            .check()?;
        for &(window, saved, _) in &guard.regions {
            xfixes::set_window_shape_region(&guard.connection, window, SK::INPUT, 0, 0, saved)?
                .check()?;
        }
        Ok(guard)
    }
}
impl Drop for InputGuard {
    fn drop(&mut self) {
        for &(window, _, empty) in &self.regions {
            let _ =
                xfixes::set_window_shape_region(&self.connection, window, SK::INPUT, 0, 0, empty);
        }
        let _ = self.connection.configure_window(
            self.viewport.0,
            &ConfigureWindowAux::new().width(1).height(1),
        );
        let _ = self.connection.ungrab_server();
        let _ = self.connection.flush();
    }
}

pub(super) struct Detached {
    connection: Arc<RustConnection>,
    root: u32,
    client: u32,
    holder: u32,
    parent: u32,
    input: Option<HiddenInput>,
    position: (i16, i16),
    saved: bool,
}
impl Detached {
    pub fn new(
        connection: Arc<RustConnection>,
        root: u32,
        client: u32,
        info: &wire::WindowInfo,
    ) -> Result<Self, Error> {
        shape::query_version(&connection)?.reply()?;
        let geometry = connection.get_geometry(root)?.reply()?;
        let point = connection
            .translate_coordinates(client, root, 0, 0)?
            .reply()?;
        let x = i32::from(point.dst_x).clamp(
            0,
            i32::from(geometry.width)
                .saturating_sub(info.width as i32)
                .max(0),
        ) as i16;
        let y = i32::from(point.dst_y).clamp(
            0,
            i32::from(geometry.height)
                .saturating_sub(info.height as i32)
                .max(0),
        ) as i16;
        // A nested holder avoids competing with XFWM's per-root-child manual
        // redirection. Its one-pixel parent stays outside the desktop. Input
        // expands the clipping parent only inside a server-grab transaction.
        let parent = connection.generate_id()?;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                parent,
                root,
                -1,
                -1,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().override_redirect(1),
            )?
            .check()?;
        let holder = connection.generate_id()?;
        if let Err(error) = connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                holder,
                parent,
                x + 1,
                y + 1,
                info.width as u16,
                info.height as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )?
            .check()
        {
            let _ = connection.destroy_window(parent);
            return Err(error.into());
        }
        let mut state = Self {
            connection,
            root,
            client,
            holder,
            parent,
            input: None,
            position: (x, y),
            saved: false,
        };
        state.input = Some(HiddenInput::new(state.connection.clone(), holder)?);
        // Never destroy a container containing somebody else's client. The
        // save-set is installed before reparenting and removed only after a
        // checked reparent back to the host root.
        state
            .connection
            .change_save_set(SetMode::INSERT, client)?
            .check()?;
        state.saved = true;
        // ICCCM withdrawal also removes the old WM's passive button grabs.
        // Reparenting an already-iconified client alone does not do that.
        state
            .connection
            .send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                UnmapNotifyEvent {
                    response_type: UNMAP_NOTIFY_EVENT,
                    sequence: 0,
                    event: root,
                    window: client,
                    from_configure: false,
                },
            )?
            .check()?;
        let state_atom = state
            .connection
            .intern_atom(false, b"WM_STATE")?
            .reply()?
            .atom;
        let clients = state
            .connection
            .intern_atom(false, b"_NET_CLIENT_LIST")?
            .reply()?
            .atom;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let list = state
                .connection
                .get_property(
                    false,
                    root,
                    clients,
                    AtomEnum::WINDOW,
                    0,
                    wire::MAX_WINDOWS as u32,
                )?
                .reply()?;
            let managed = list
                .value32()
                .is_some_and(|mut values| values.any(|id| id == client));
            let status = state
                .connection
                .get_property(false, client, state_atom, AtomEnum::ANY, 0, 2)?
                .reply()?
                .value32()
                .and_then(|mut values| values.next())
                .unwrap_or(0);
            if !managed && status == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(
                    "The host window manager did not release the minimized application".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        state
            .connection
            .reparent_window(client, holder, 0, 0)?
            .check()?;
        state.connection.map_window(parent)?.check()?;
        state.connection.map_window(holder)?.check()?;
        state.connection.map_window(client)?.check()?;
        let wm_state = state
            .connection
            .intern_atom(false, b"WM_STATE")?
            .reply()?
            .atom;
        state
            .connection
            .change_property32(PropMode::REPLACE, client, wm_state, wm_state, &[1, 0])?
            .check()?;
        // Notify toolkits that the live mapped view is no longer iconified;
        // retaining EWMH HIDDEN can otherwise suspend their frame clocks.
        let net_state = state
            .connection
            .intern_atom(false, b"_NET_WM_STATE")?
            .reply()?
            .atom;
        let hidden = state
            .connection
            .intern_atom(false, b"_NET_WM_STATE_HIDDEN")?
            .reply()?
            .atom;
        let property = state
            .connection
            .get_property(false, client, net_state, AtomEnum::ATOM, 0, 64)?
            .reply()?;
        if property.bytes_after != 0 {
            return Err("Application window state exceeds the supported limit".into());
        }
        let values: Vec<_> = property
            .value32()
            .map(|values| values.filter(|atom| *atom != hidden).collect())
            .unwrap_or_default();
        state
            .connection
            .change_property32(
                PropMode::REPLACE,
                client,
                net_state,
                AtomEnum::ATOM,
                &values,
            )?
            .check()?;
        state.connection.sync()?;
        {
            let _lock = ServerLock::new(state.connection.clone())?;
            let atom = state.connection.intern_atom(false, INDEX)?.reply()?.atom;
            let mut live = records(&state.connection, root, atom)?;
            if live.len() >= wire::MAX_WINDOWS {
                return Err("Too many independent applications".into());
            }
            live.push([parent, holder, client]);
            let values: Vec<u32> = live.into_iter().flatten().collect();
            state
                .connection
                .change_property32(PropMode::REPLACE, root, atom, AtomEnum::WINDOW, &values)?
                .check()?;
        }
        Ok(state)
    }
    pub fn input(&self) -> (u32, u32, u32) {
        self.input.as_ref().unwrap().regions()
    }
    pub fn viewport(&self) -> Result<(u32, u32, u32), Error> {
        let root = self.connection.get_geometry(self.root)?.reply()?;
        Ok((self.parent, root.width.into(), root.height.into()))
    }
    pub fn resize_holder(&self, width: u32, height: u32) -> Result<(), Error> {
        let geometry = self.connection.get_geometry(self.holder)?.reply()?;
        if u32::from(geometry.width) != width || u32::from(geometry.height) != height {
            self.connection
                .configure_window(
                    self.holder,
                    &ConfigureWindowAux::new().width(width).height(height),
                )?
                .check()?;
            let input = self.input.as_ref().unwrap();
            xfixes::set_region(
                &self.connection,
                input.saved,
                &[x11rb::protocol::xproto::Rectangle {
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                }],
            )?
            .check()?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn verify_minimized(
    connection: Arc<RustConnection>,
    controller: &RustConnection,
    window: u32,
) {
    let mut mirror = Mirror::from_connection(connection).unwrap();
    let root = mirror.root;
    let iconic = controller
        .intern_atom(false, b"WM_CHANGE_STATE")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let minimize = || {
        controller
            .send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, window, iconic, [3, 0, 0, 0, 0]),
            )
            .unwrap()
            .check()
            .unwrap();
        controller.flush().unwrap();
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
                "host WM did not iconify its window"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    mirror.select(window).unwrap();
    minimize();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let reply = mirror.frame().unwrap();
        if matches!(reply, wire::Reply::Frame { .. }) && mirror.detached.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "minimized view did not remain live"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let holder = mirror.detached.as_ref().unwrap().holder;
    let viewport = mirror.detached.as_ref().unwrap().parent;
    let geometry = controller.get_geometry(viewport).unwrap().reply().unwrap();
    assert_eq!(
        (geometry.x, geometry.y, geometry.width, geometry.height),
        (-1, -1, 1, 1)
    );
    assert_eq!(
        controller
            .query_tree(window)
            .unwrap()
            .reply()
            .unwrap()
            .parent,
        holder
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
    assert!(mirror.list().unwrap().iter().any(|w| w.id == window));
    assert!(
        Mirror::from_connection(mirror.connection.clone())
            .unwrap()
            .list()
            .unwrap()
            .iter()
            .any(|w| w.id == window),
        "another directory helper lost the independently hosted window"
    );
    let gc = controller.generate_id().unwrap();
    controller
        .create_gc(gc, window, &CreateGCAux::new().foreground(0x5ac39e))
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
                width: 30,
                height: 30,
            }],
        )
        .unwrap()
        .check()
        .unwrap();
    controller.sync().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        mirror.frame().unwrap();
        if mirror
            .selected
            .as_ref()
            .unwrap()
            .pixels
            .chunks_exact(4)
            .any(|pixel| pixel[..3] == [0x9e, 0xc3, 0x5a])
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "hidden application stopped painting"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let shape = shape::get_rectangles(controller, holder, SK::INPUT)
        .unwrap()
        .reply()
        .unwrap();
    assert!(
        shape.rectangles.is_empty(),
        "hidden view blocks host desktop input"
    );
    while controller.poll_for_event().unwrap().is_some() {}
    mirror.handle(wire::Request::Focus).unwrap();
    mirror
        .handle(wire::Request::Key {
            evdev: 42,
            pressed: true,
        })
        .unwrap();
    for pressed in [true, false] {
        mirror
            .handle(wire::Request::Button {
                button: 1,
                pressed,
                x: 10,
                y: 10,
            })
            .unwrap();
    }
    mirror
        .handle(wire::Request::Key {
            evdev: 42,
            pressed: false,
        })
        .unwrap();
    controller.sync().unwrap();
    let mut clicked = false;
    while let Some(event) = controller.poll_for_event().unwrap() {
        if let Event::ButtonPress(event) = event {
            if event.event == window {
                clicked = true;
                assert!(event.state.contains(KeyButMask::SHIFT));
            }
        }
    }
    assert!(
        clicked,
        "hidden application did not receive a real modified click"
    );
    assert!(shape::get_rectangles(controller, holder, SK::INPUT)
        .unwrap()
        .reply()
        .unwrap()
        .rectangles
        .is_empty());
    // A root-level legacy menu is hidden from the desktop but still captured
    // and clicked through the same real-input path, under XFWM compositing.
    let origin = controller
        .translate_coordinates(window, root, 0, 0)
        .unwrap()
        .reply()
        .unwrap();
    let menu = controller.generate_id().unwrap();
    controller
        .create_window(
            0,
            menu,
            root,
            origin.dst_x + 20,
            origin.dst_y + 20,
            60,
            40,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0x2d67a9)
                .event_mask(EventMask::BUTTON_PRESS),
        )
        .unwrap()
        .check()
        .unwrap();
    let transient = controller
        .intern_atom(false, b"WM_TRANSIENT_FOR")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let menu_type = controller
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_POPUP_MENU")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let type_atom = controller
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    controller
        .change_property32(
            PropMode::REPLACE,
            menu,
            transient,
            AtomEnum::WINDOW,
            &[window],
        )
        .unwrap()
        .check()
        .unwrap();
    controller
        .change_property32(
            PropMode::REPLACE,
            menu,
            type_atom,
            AtomEnum::ATOM,
            &[menu_type],
        )
        .unwrap()
        .check()
        .unwrap();
    controller.map_window(menu).unwrap().check().unwrap();
    controller.sync().unwrap();
    mirror.frame().unwrap();
    assert!(mirror.popups.get(&menu).unwrap().hidden_input().is_some());
    let menus = mirror.capture_popups(window).unwrap();
    let captured = menus.iter().find(|popup| popup.window == menu).unwrap();
    for tile in captured.tiles.clone() {
        assert!(
            tile.decode()
                .unwrap()
                .chunks_exact(4)
                .all(|pixel| pixel[..3] == [0xa9, 0x67, 0x2d]),
            "hiding a menu changed its captured colours"
        );
    }
    while controller.poll_for_event().unwrap().is_some() {}
    for pressed in [true, false] {
        mirror
            .handle(wire::Request::Button {
                button: 1,
                pressed,
                x: 25,
                y: 25,
            })
            .unwrap();
    }
    controller.sync().unwrap();
    let mut menu_clicked = false;
    while let Some(event) = controller.poll_for_event().unwrap() {
        if let Event::ButtonPress(event) = event {
            menu_clicked |= event.event == menu;
        }
    }
    assert!(menu_clicked, "hidden menu did not receive a real click");
    controller.destroy_window(menu).unwrap().check().unwrap();
    mirror.frame().unwrap();
    mirror
        .handle(wire::Request::Resize {
            width: 300,
            height: 180,
        })
        .unwrap();
    let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
    assert_eq!((geometry.width, geometry.height), (300, 180));
    mirror.clear().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let parent = controller
            .query_tree(window)
            .unwrap()
            .reply()
            .unwrap()
            .parent;
        if parent != root
            && controller
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .map_state
                == MapState::VIEWABLE
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closing a view did not restore its live client to the host WM"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // A helper disappearing must not take its reparented application with it.
    mirror.select(window).unwrap();
    minimize();
    mirror.frame().unwrap();
    assert!(mirror.detached.is_some());
    let holder = mirror.detached.as_ref().unwrap().holder;
    assert_eq!(
        unsafe { libc::shutdown(mirror.connection.stream().as_raw_fd(), libc::SHUT_RDWR) },
        0
    );
    drop(mirror);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let parent = controller
            .query_tree(window)
            .unwrap()
            .reply()
            .unwrap()
            .parent;
        if parent != holder
            && parent != root
            && controller
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .unwrap()
                .map_state
                == MapState::VIEWABLE
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server save-set did not rescue its client after a helper disconnect"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.free_gc(gc).unwrap().check().unwrap();
}
impl Drop for Detached {
    fn drop(&mut self) {
        let parent = self
            .connection
            .query_tree(self.client)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|tree| tree.parent);
        let restored = if parent == Some(self.holder) {
            let result = (|| -> Result<(), Error> {
                self.connection
                    .reparent_window(self.client, self.root, self.position.0, self.position.1)?
                    .check()?;
                self.connection.map_window(self.client)?.check()?;
                Ok(())
            })();
            result.is_ok()
        } else {
            true
        };
        if restored {
            // Prune this record and any holders whose helpers disappeared.
            if let Ok(_lock) = ServerLock::new(self.connection.clone()) {
                let _ = (|| -> Result<(), Error> {
                    let atom = self.connection.intern_atom(false, INDEX)?.reply()?.atom;
                    let values: Vec<u32> = records(&self.connection, self.root, atom)?
                        .into_iter()
                        .filter(|record| record[0] != self.parent)
                        .flatten()
                        .collect();
                    self.connection
                        .change_property32(
                            PropMode::REPLACE,
                            self.root,
                            atom,
                            AtomEnum::WINDOW,
                            &values,
                        )?
                        .check()?;
                    Ok(())
                })();
            }
            if self.saved {
                let _ = self
                    .connection
                    .change_save_set(SetMode::DELETE, self.client);
            }
            drop(self.input.take());
            let _ = self.connection.destroy_window(self.parent);
        } else {
            // Leave the save-set and holder intact until connection teardown;
            // destroying the holder after a failed restore would kill its app.
            tracing::warn!("Could not return an independent application to its desktop; server save-set remains active");
        }
        let _ = self.connection.flush();
    }
}
