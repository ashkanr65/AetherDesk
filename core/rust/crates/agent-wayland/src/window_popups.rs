//! Capture only menus belonging to the selected client, never a root screenshot.
use super::*;
use std::collections::BTreeSet;

pub(super) struct PopupCapture {
    id: u32,
    pixmap: u32,
    damage: u32,
    size: (u32, u32),
    dirty: bool,
    last: Option<wire::Popup>,
}
impl PopupCapture {
    fn destroy(self, connection: &RustConnection) {
        let _ = connection.free_pixmap(self.pixmap);
        let _ = damage::destroy(connection, self.damage);
        let _ = composite::unredirect_window(connection, self.id, Redirect::AUTOMATIC);
    }
}
fn related(
    transient: u32,
    selected: u32,
    family: &BTreeSet<u32>,
    owner_match: bool,
    menu_type: bool,
    adjacent: bool,
) -> bool {
    transient == selected || family.contains(&transient) || (owner_match && menu_type && adjacent)
}
fn overlaps(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    i64::from(a.0) < i64::from(b.0) + i64::from(b.2) + 32
        && i64::from(a.0) + i64::from(a.2) + 32 > i64::from(b.0)
        && i64::from(a.1) < i64::from(b.1) + i64::from(b.3) + 32
        && i64::from(a.1) + i64::from(a.3) + 32 > i64::from(b.1)
}
// Use the server's actual resource ownership, not guessed XID bit widths.
fn in_client(id: u32, range: (u32, u32)) -> bool {
    range.0 != 0 && id & !range.1 == range.0
}
impl Mirror {
    pub(super) fn application_client_range(&self, selected: u32) -> Option<(u32, u32)> {
        x11rb::protocol::res::query_clients(&self.connection)
            .ok()?
            .reply()
            .ok()?
            .clients
            .into_iter()
            .map(|c| (c.resource_base, c.resource_mask))
            .find(|&range| in_client(selected, range))
    }
    pub(super) fn clear_popups(&mut self) {
        for (_, capture) in std::mem::take(&mut self.popups) {
            capture.destroy(&self.connection);
        }
        let _ = self.connection.flush();
    }
    pub(super) fn popup_event(&mut self, event: &Event) {
        for popup in self.popups.values_mut() {
            match event {
                Event::DamageNotify(e) if e.damage == popup.damage => popup.dirty = true,
                _ => {}
            }
        }
    }
    pub(super) fn popup_candidates(
        &self,
        selected: u32,
    ) -> Result<Vec<(u32, i32, i32, u32, u32)>, Error> {
        let origin = self
            .connection
            .translate_coordinates(selected, self.root, 0, 0)?
            .reply()?;
        let size = self.connection.get_geometry(selected)?.reply()?;
        let owner = self
            .values(selected, b"WM_CLIENT_LEADER")?
            .first()
            .copied()
            .unwrap_or(selected);
        let pid = self.values(selected, b"_NET_WM_PID")?.first().copied();
        let types = [
            b"_NET_WM_WINDOW_TYPE_DROPDOWN_MENU".as_slice(),
            b"_NET_WM_WINDOW_TYPE_POPUP_MENU",
            b"_NET_WM_WINDOW_TYPE_COMBO",
            b"_NET_WM_WINDOW_TYPE_TOOLTIP",
        ]
        .iter()
        .map(|name| self.atom(name))
        .collect::<Result<Vec<_>, _>>()?;
        let client_range = self.selected.as_ref().and_then(|s| s.client_range);
        let active = self.active()?;
        let selected_active = active == selected || active == 0;
        let mut family = BTreeSet::new();
        let mut regions = vec![(
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            u32::from(size.width),
            u32::from(size.height),
        )];
        let children = self.connection.query_tree(self.root)?.reply()?.children;
        let mut candidates = Vec::new();
        // Root children are in stacking order. Repeat for submenus whose
        // transient parent occurs later; preserve stacking order in the result.
        for _ in 0..wire::MAX_POPUPS {
            let mut added = false;
            for &id in children.iter().take(1024) {
                if id == selected || family.contains(&id) {
                    continue;
                }
                let Ok(cookie) = self.connection.get_window_attributes(id) else {
                    continue;
                };
                let Ok(attrs) = cookie.reply() else {
                    continue;
                };
                if attrs.map_state != MapState::VIEWABLE || !attrs.override_redirect {
                    continue;
                }
                let Ok(geometry) = self.connection.get_geometry(id) else {
                    continue;
                };
                let Ok(geometry) = geometry.reply() else {
                    continue;
                };
                if wire::dimensions(geometry.width.into(), geometry.height.into()).is_err() {
                    continue;
                }
                let rect = (
                    i32::from(geometry.x),
                    i32::from(geometry.y),
                    u32::from(geometry.width),
                    u32::from(geometry.height),
                );
                let transient = self
                    .values(id, b"WM_TRANSIENT_FOR")
                    .unwrap_or_default()
                    .first()
                    .copied()
                    .unwrap_or(0);
                let leader = self
                    .values(id, b"WM_CLIENT_LEADER")
                    .unwrap_or_default()
                    .first()
                    .copied();
                let other_pid = self
                    .values(id, b"_NET_WM_PID")
                    .unwrap_or_default()
                    .first()
                    .copied();
                let declared_types = self.values(id, b"_NET_WM_WINDOW_TYPE").unwrap_or_default();
                let same_client = client_range.is_some_and(|range| in_client(id, range));
                // Older Xt/Motif override shells may lack transient, PID and
                // EWMH menu properties. Accept only adjacent, untyped windows
                // from the selected application's proven X client while active.
                let menu_type = declared_types.iter().any(|v| types.contains(v))
                    || (declared_types.is_empty()
                        && same_client
                        && (selected_active
                            || active == id
                            || self
                                .selected
                                .as_ref()
                                .is_some_and(|s| s.popups.iter().any(|p| p.window == active))));
                let owner_match =
                    same_client || leader == Some(owner) || (pid.is_some() && pid == other_pid);
                if !related(
                    transient,
                    selected,
                    &family,
                    owner_match,
                    menu_type,
                    regions.iter().any(|&r| overlaps(rect, r)),
                ) {
                    continue;
                }
                if candidates.len() >= wire::MAX_POPUPS {
                    return Err("Too many application menus".into());
                }
                family.insert(id);
                regions.push(rect);
                added = true;
                candidates.push((
                    id,
                    rect.0 - i32::from(origin.dst_x),
                    rect.1 - i32::from(origin.dst_y),
                    rect.2,
                    rect.3,
                ));
            }
            if !added {
                break;
            }
        }
        candidates.sort_by_key(|(id, ..)| children.iter().position(|v| v == id));
        Ok(candidates)
    }
    fn popup_capture(
        &mut self,
        id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<wire::Popup, Error> {
        if self
            .popups
            .get(&id)
            .is_some_and(|c| c.size != (width, height))
        {
            self.popups.remove(&id).unwrap().destroy(&self.connection);
        }
        if !self.popups.contains_key(&id) {
            let pixmap = self.connection.generate_id()?;
            let damage_id = self.connection.generate_id()?;
            composite::redirect_window(&self.connection, id, Redirect::AUTOMATIC)?.check()?;
            self.popups.insert(
                id,
                PopupCapture {
                    id,
                    pixmap,
                    damage: damage_id,
                    size: (width, height),
                    dirty: true,
                    last: None,
                },
            );
            damage::create(&self.connection, damage_id, id, ReportLevel::NON_EMPTY)?.check()?;
            composite::name_window_pixmap(&self.connection, id, pixmap)?.check()?;
        }
        let capture = self.popups.get_mut(&id).unwrap();
        if !capture.dirty {
            if let Some(last) = &capture.last {
                let mut popup = last.clone();
                popup.x = x;
                popup.y = y;
                return Ok(popup);
            }
        }
        let attrs = self.connection.get_window_attributes(id)?.reply()?;
        damage::subtract(&self.connection, capture.damage, 0u32, 0u32)?.check()?;
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                capture.pixmap,
                0,
                0,
                width as u16,
                height as u16,
                u32::MAX,
            )?
            .reply()?;
        let visual = self.connection.setup().roots[0]
            .allowed_depths
            .iter()
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.visual_id == attrs.visual)
            .ok_or("Menu visual unavailable")?;
        if visual.class != VisualClass::TRUE_COLOR
            || (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0xff00, 0xff)
        {
            return Err("Unsupported menu colour format".into());
        }
        let format = self
            .connection
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .ok_or("Menu pixel layout unavailable")?;
        let little = self.connection.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let mut pixels = decode_pixels(
            width,
            height,
            format.bits_per_pixel,
            format.scanline_pad,
            little,
            &image.data,
        )?;
        if image.depth == 32 && format.bits_per_pixel == 32 {
            let stride = (width as usize * 32).div_ceil(format.scanline_pad as usize)
                * (format.scanline_pad as usize / 8);
            for row in 0..height as usize {
                for col in 0..width as usize {
                    pixels[(row * width as usize + col) * 4 + 3] =
                        image.data[row * stride + col * 4 + if little { 3 } else { 0 }];
                }
            }
        }
        let mut tiles = Vec::new();
        for ty in (0..height).step_by(256) {
            for tx in (0..width).step_by(256) {
                let region = Rectangle {
                    x: tx,
                    y: ty,
                    width: (width - tx).min(256),
                    height: (height - ty).min(256),
                };
                let mut data =
                    Vec::with_capacity(region.width as usize * region.height as usize * 4);
                for row in ty..ty + region.height {
                    let start = ((row * width + tx) * 4) as usize;
                    data.extend_from_slice(&pixels[start..start + region.width as usize * 4]);
                }
                tiles.push(LosslessTile::encode(region, data)?);
            }
        }
        let popup = wire::Popup {
            window: id,
            x,
            y,
            width,
            height,
            tiles,
        };
        capture.last = Some(popup.clone());
        capture.dirty = false;
        Ok(popup)
    }
    pub(super) fn capture_popups(&mut self, selected: u32) -> Result<Vec<wire::Popup>, Error> {
        let candidates = self.popup_candidates(selected)?;
        let selected_size = self.connection.get_geometry(selected)?.reply()?;
        let mut budget = u64::from(selected_size.width) * u64::from(selected_size.height);
        for &(_, _, _, w, h) in &candidates {
            budget += u64::from(w) * u64::from(h);
        }
        if budget > wire::MAX_PIXELS as u64 {
            return Err("Application menus exceed capture budget".into());
        }
        let ids = candidates.iter().map(|c| c.0).collect::<BTreeSet<_>>();
        for id in self
            .popups
            .keys()
            .copied()
            .filter(|id| !ids.contains(id))
            .collect::<Vec<_>>()
        {
            self.popups.remove(&id).unwrap().destroy(&self.connection);
        }
        let mut output = Vec::new();
        for (id, x, y, w, h) in candidates {
            match self.popup_capture(id, x, y, w, h) {
                Ok(popup) => output.push(popup),
                Err(error) => {
                    if let Some(c) = self.popups.remove(&id) {
                        c.destroy(&self.connection);
                    }
                    // Unmapping between enumeration and GetImage is normal.
                    if self
                        .connection
                        .get_window_attributes(id)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .is_some_and(|a| a.map_state == MapState::VIEWABLE)
                    {
                        return Err(error);
                    }
                }
            }
        }
        wire::validate_popups(&output)?;
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menus_require_an_application_owner_and_do_not_include_other_desktop_windows() {
        assert!(in_client(0x2800012, (0x2800000, 0x1fffff)));
        assert!(!in_client(0x2a00012, (0x2800000, 0x1fffff)));
        assert!(!in_client(3, (0, u32::MAX)));
        let family = BTreeSet::from([20]);
        assert!(related(10, 10, &family, false, false, false));
        assert!(related(20, 10, &family, false, false, false));
        assert!(related(0, 10, &family, true, true, true));
        assert!(!related(0, 10, &family, true, false, true));
        assert!(!related(0, 10, &family, false, true, true));
        assert!(!related(0, 10, &family, true, true, false));
    }
}

#[cfg(test)]
pub(super) fn verify_menus(mirror: &mut Mirror, controller: &RustConnection, window: u32) {
    use x11rb::wrapper::ConnectionExt as _;
    mirror.handle(wire::Request::Focus).unwrap();
    let origin = controller
        .translate_coordinates(window, mirror.root, 0, 0)
        .unwrap()
        .reply()
        .unwrap();
    let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
    let width = u32::from(geometry.width);
    let height = u32::from(geometry.height);
    let menu_atom = controller
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
    let transient_atom = controller
        .intern_atom(false, b"WM_TRANSIENT_FOR")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let root = mirror.root;
    let create = |x: i16, y: i16, w: u16, h: u16, transient: Option<u32>, colour: u32| {
        let id = controller.generate_id().unwrap();
        controller
            .create_window(
                0,
                id,
                root,
                x,
                y,
                w,
                h,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(colour),
            )
            .unwrap()
            .check()
            .unwrap();
        controller
            .change_property32(
                PropMode::REPLACE,
                id,
                type_atom,
                AtomEnum::ATOM,
                &[menu_atom],
            )
            .unwrap()
            .check()
            .unwrap();
        if let Some(parent) = transient {
            controller
                .change_property32(
                    PropMode::REPLACE,
                    id,
                    transient_atom,
                    AtomEnum::WINDOW,
                    &[parent],
                )
                .unwrap()
                .check()
                .unwrap();
        }
        controller.map_window(id).unwrap().check().unwrap();
        id
    };
    let x = origin.dst_x + geometry.width as i16 - 20;
    let y = origin.dst_y + 20;
    let menu = create(x, y, 80, 80, Some(window), 0x123456);
    let submenu = create(x + 60, y + 10, 60, 60, Some(menu), 0xabcdef);
    let unrelated = create(1, 1, 30, 30, None, 0xdeadbe);
    let legacy = create(x, y + 110, 50, 40, None, 0x987654);
    controller
        .delete_property(legacy, type_atom)
        .unwrap()
        .check()
        .unwrap();
    assert!(
        mirror.selected.as_ref().unwrap().client_range.is_some(),
        "fixture requires XRes ownership"
    );
    // An adjacent untyped window from another client must never be captured.
    let foreign = mirror.connection.generate_id().unwrap();
    mirror
        .connection
        .create_window(
            0,
            foreign,
            root,
            x,
            y + 170,
            30,
            30,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(0xdeadbe),
        )
        .unwrap()
        .check()
        .unwrap();
    mirror
        .connection
        .map_window(foreign)
        .unwrap()
        .check()
        .unwrap();
    let wire::Reply::Frame {
        width: w,
        height: h,
        popups,
        ..
    } = mirror.handle(wire::Request::Poll).unwrap()
    else {
        panic!("mapped application menus were not published");
    };
    assert_eq!(
        (w, h),
        (width, height),
        "menu stretched the application canvas"
    );
    assert_eq!(
        popups.iter().map(|p| p.window).collect::<Vec<_>>(),
        vec![menu, submenu, legacy]
    );
    let menu_image = &popups[0];
    assert!(
        menu_image.x + menu_image.width as i32 > width as i32,
        "fixture menu must extend outside the client"
    );
    let bytes = wire::apply_frame(
        None,
        1,
        None,
        menu_image.width,
        menu_image.height,
        menu_image.tiles.clone(),
    )
    .unwrap();
    assert_eq!(
        &bytes[..4],
        &[0x56, 0x34, 0x12, 255],
        "menu colours changed"
    );
    // Toolkits can move actual X focus into menus/children, and temporarily
    // advertise a menu as the active window. Neither is an unrelated app.
    let active_atom = mirror.atom(b"_NET_ACTIVE_WINDOW").unwrap();
    for focused in [menu, submenu, legacy] {
        controller
            .set_input_focus(InputFocus::PARENT, focused, 0u32)
            .unwrap()
            .check()
            .unwrap();
        controller
            .change_property32(
                PropMode::REPLACE,
                root,
                active_atom,
                AtomEnum::WINDOW,
                &[focused],
            )
            .unwrap()
            .check()
            .unwrap();
        assert!(
            mirror.application_has_focus().unwrap(),
            "owned menu lost focus"
        );
        mirror
            .handle(wire::Request::Key {
                evdev: 42,
                pressed: true,
            })
            .unwrap();
        mirror
            .handle(wire::Request::Key {
                evdev: 42,
                pressed: false,
            })
            .unwrap();
    }
    controller
        .change_property32(
            PropMode::REPLACE,
            root,
            active_atom,
            AtomEnum::WINDOW,
            &[foreign],
        )
        .unwrap()
        .check()
        .unwrap();
    assert!(
        !mirror.application_has_focus().unwrap(),
        "foreign window acquired application input"
    );
    assert!(mirror
        .handle(wire::Request::Key {
            evdev: 42,
            pressed: true
        })
        .is_err());
    controller
        .change_property32(
            PropMode::REPLACE,
            root,
            active_atom,
            AtomEnum::WINDOW,
            &[window],
        )
        .unwrap()
        .check()
        .unwrap();
    controller
        .set_input_focus(InputFocus::PARENT, window, 0u32)
        .unwrap()
        .check()
        .unwrap();
    mirror
        .handle(wire::Request::Move {
            x: menu_image.x + 40,
            y: menu_image.y + 20,
        })
        .unwrap();
    let pointer = controller.query_pointer(menu).unwrap().reply().unwrap();
    assert_eq!(
        (pointer.win_x, pointer.win_y),
        (40, 20),
        "outside-client menu coordinates were clipped"
    );
    mirror.handle(wire::Request::Stop).unwrap();
    assert!(mirror.popups.is_empty());
    assert_eq!(
        controller
            .get_window_attributes(menu)
            .unwrap()
            .reply()
            .unwrap()
            .map_state,
        MapState::VIEWABLE,
        "stopping the view closed the original menu"
    );
    mirror.handle(wire::Request::Select(window)).unwrap();
    mirror
        .connection
        .destroy_window(foreign)
        .unwrap()
        .check()
        .unwrap();
    for id in [submenu, menu, unrelated, legacy] {
        controller.destroy_window(id).unwrap().check().unwrap();
    }
    let wire::Reply::Frame { popups, .. } = mirror.handle(wire::Request::Poll).unwrap() else {
        panic!("menu cleanup did not publish an image");
    };
    assert!(
        popups.is_empty(),
        "closed menus survived in the independent view"
    );
}
