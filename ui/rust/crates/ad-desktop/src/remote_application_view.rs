use super::*;
use std::time::Instant;
pub(super) fn display_scale(ctx: &egui::Context, zoom: f32) -> geometry::Scale {
    geometry::Scale::new(
        ctx.input(|i| i.viewport().native_pixels_per_point.unwrap_or(1.0)),
        ctx.pixels_per_point(),
        zoom,
    )
}
impl Applications {
    fn upload_frame(&mut self, ctx: &egui::Context, frame: &worker::Frame) {
        if frame.sequence != self.sequence
            && frame
                .size
                .iter()
                .all(|side| *side <= ctx.input(|i| i.max_texture_side))
        {
            let same_size = self
                .texture
                .as_ref()
                .is_some_and(|t| t.size() == frame.size);
            if !same_size {
                self.texture = Some(ctx.load_texture(
                    format!("application-{:?}", self.id),
                    egui::ColorImage::new(
                        frame.size,
                        vec![egui::Color32::BLACK; frame.size[0] * frame.size[1]],
                    ),
                    egui::TextureOptions::NEAREST,
                ));
            }
            let old = self
                .uploaded_image
                .as_ref()
                .filter(|old| same_size && old.size == frame.size);
            let texture = self.texture.as_mut().unwrap();
            for (&(x, y), image) in frame.tiles.iter() {
                if old
                    .and_then(|old| old.tiles.get(&(x, y)))
                    .is_some_and(|previous| std::sync::Arc::ptr_eq(previous, image))
                {
                    continue;
                }
                texture.set_partial(
                    [x as usize, y as usize],
                    egui::ImageData::Color(image.clone()),
                    egui::TextureOptions::NEAREST,
                );
            }
            self.uploaded_image = Some(frame.clone());
            self.sequence = frame.sequence;
        }
    }
    pub(super) fn popup_title(&self, id: u32) -> String {
        format!("{} - menu {id}", self.title)
    }
    pub(super) fn viewport(&mut self, ctx: &egui::Context, state: &State) {
        let Some(window) = state.selected.clone() else {
            return;
        };
        let caption = native::caption(&window.title);
        let builder = crate::app_layout::builder(
            ctx,
            self.id,
            if self.native.handle().is_some() {
                &caption
            } else {
                &self.title
            },
            display_scale(ctx, self.content_zoom)
                .logical(egui::vec2(window.width as f32, window.height as f32))
                + egui::vec2(0.0, 32.0),
        )
        .with_min_inner_size(egui::vec2(100.0, 80.0));
        let mut close = false;
        let mut any_focus = false;
        self.popup_textures
            .retain(|id, _| state.popups.iter().any(|p| p.info.window == *id));
        self.popup_native
            .retain(|id, _| state.popups.iter().any(|p| p.info.window == *id));
        ctx.show_viewport_immediate(self.id, builder, |ctx, _| {
            self.native.bind(&self.title);
            self.native.set_caption(&caption);
            crate::app_layout::update(ctx, "remote-application");
            let mut zoom_changed = false;
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
                return;
            }
            egui::TopBottomPanel::top("application-actions").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if !state.alive
                        && crate::ui_icons::button(
                            ui,
                            crate::ui_icons::Icon::Refresh,
                            "Reconnect application view",
                            false,
                        )
                        .clicked()
                    {
                        self.retry_at = Instant::now();
                        self.open_window(ctx, window.id);
                    }
                    if crate::ui_icons::button(
                        ui,
                        crate::ui_icons::Icon::Tab,
                        "Return to desktop (keep application running)",
                        false,
                    )
                    .clicked()
                    {
                        close = true;
                    }
                    if crate::ui_icons::button(
                        ui,
                        crate::ui_icons::Icon::Resolution,
                        "Resize application to this window",
                        false,
                    )
                    .clicked()
                    {
                        let extent = display_scale(ctx, self.content_zoom)
                            .extent(ctx.available_rect().size());
                        let size = egui::vec2(extent.0 as f32, extent.1 as f32);
                        if wire::dimensions(size.x.max(64.0) as u32, size.y.max(64.0) as u32)
                            .is_ok()
                        {
                            self.send(Request::Resize {
                                width: size.x.max(64.0) as u32,
                                height: size.y.max(64.0) as u32,
                            });
                        }
                    }
                    let zoom = crate::ui_icons::button(
                        ui,
                        crate::ui_icons::Icon::Zoom,
                        "Application zoom",
                        false,
                    );
                    egui::Popup::menu(&zoom)
                        .layout(egui::Layout::top_down(egui::Align::Min))
                        .show(|ui| {
                            ui.set_width(220.0);
                            ui.label("Application zoom");
                            zoom_changed |= ui
                                .add(
                                    egui::Slider::new(&mut self.content_zoom, 0.5..=3.0)
                                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                )
                                .changed();
                            if ui.button("Reset to 100%").clicked() {
                                self.content_zoom = 1.0;
                                zoom_changed = true;
                            }
                        });
                    ui.add(egui::Label::new(&window.title).truncate())
                        .on_hover_text(&window.title);
                });
            });
            self.host_titlebar(ctx, state, &window);
            let scale = display_scale(ctx, self.content_zoom);
            let mut image_rect = egui::Rect::NOTHING;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if let Some(frame) = &state.image {
                        self.upload_frame(ctx, frame);
                    }
                    let (rect, response) = ui.allocate_exact_size(
                        ui.available_size().max(egui::vec2(1.0, 1.0)),
                        egui::Sense::click_and_drag(),
                    );
                    image_rect =
                        scale.image(rect, egui::vec2(window.width as f32, window.height as f32));
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
                    if let Some(texture) = &self.texture {
                        ui.painter().image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    any_focus |= self.application_input(
                        ui,
                        &response,
                        image_rect,
                        (window.width, window.height),
                        (0, 0),
                        true,
                        state.alive,
                    );
                    let extent = scale.extent(rect.size());
                    if zoom_changed {
                        // A view zoom is not a remote resize, just like a DPI transition.
                        self.resize_due = None;
                        self.last_extent = Some(extent);
                        self.resize_after = Instant::now() + Duration::from_millis(500);
                    }
                    if self.sized_window != Some(window.id) {
                        let header = (ctx.content_rect().height() - rect.height()).max(0.0);
                        let desired = scale
                            .logical(egui::vec2(window.width as f32, window.height as f32))
                            + egui::vec2(0.0, header);
                        // Initial sizing is a one-time viewport command, never an HWND resize while drawing.
                        let desired = crate::app_layout::application_initial_size(ctx, desired);
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(desired));
                        self.sized_window = Some(window.id);
                        self.resize_after = Instant::now() + Duration::from_millis(500);
                        self.last_extent = None;
                        self.resize_environment = None;
                    } else if let Some(environment) =
                        crate::app_layout::application_resize_environment(ctx)
                    {
                        if self.resize_environment != Some(environment) {
                            self.resize_environment = Some(environment);
                            self.last_extent = Some(extent);
                            self.resize_due = None;
                            self.resize_after = Instant::now() + Duration::from_millis(500);
                        } else if Instant::now() >= self.resize_after {
                            if self.last_extent != Some(extent) {
                                self.last_extent = Some(extent);
                                self.resize_due =
                                    Some((Instant::now() + Duration::from_millis(180), extent));
                            }
                            if self
                                .resize_due
                                .is_some_and(|(due, _)| Instant::now() >= due)
                            {
                                self.resize_due = None;
                                if extent != (window.width, window.height)
                                    && wire::dimensions(extent.0, extent.1).is_ok()
                                {
                                    self.send(Request::Resize {
                                        width: extent.0,
                                        height: extent.1,
                                    });
                                }
                            }
                        }
                    } else {
                        // Never resize either side while Windows is dragging,
                        // adapting DPI, or applying our native geometry command.
                        self.resize_due = None;
                    }
                    if let Some(error) = self.error.as_ref().or(state.error.as_ref()) {
                        ui.painter().text(
                            rect.left_top() + egui::vec2(8.0, 8.0),
                            egui::Align2::LEFT_TOP,
                            error,
                            egui::FontId::proportional(13.0),
                            egui::Color32::LIGHT_RED,
                        );
                    }
                });
            if !close {
                let owner = self.native.handle();
                for popup in &state.popups {
                    any_focus |= self.popup_view(ctx, popup, owner, image_rect, scale, state.alive);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(if self.resize_due.is_some() {
                50
            } else {
                250
            }));
        });
        let active = any_focus;
        if state.streaming && self.stream_active != Some(active) {
            self.send(Request::Stream { active });
            self.stream_active = Some(active);
        }
        if !any_focus {
            self.release();
        } else {
            self.focused = true;
        }
        if close {
            self.release();
            self.send(Request::Stop);
            self.worker.take();
            self.view = false;
            self.texture = None;
            self.titlebar_texture = None;
            self.popup_textures.clear();
            self.popup_native.clear();
            self.native = Default::default();
            self.stream_active = None;
            self.sized_window = None;
        }
    }
    fn popup_view(
        &mut self,
        ctx: &egui::Context,
        popup: &worker::PopupState,
        owner: Option<isize>,
        anchor: egui::Rect,
        scale: geometry::Scale,
        alive: bool,
    ) -> bool {
        let info = &popup.info;
        let limit = ctx.input(|i| i.max_texture_side);
        if info.width as usize > limit || info.height as usize > limit {
            return false;
        }
        let changed = self
            .popup_textures
            .get(&info.window)
            .is_none_or(|(_, image)| !std::sync::Arc::ptr_eq(image, &popup.image));
        if changed {
            let texture = ctx.load_texture(
                format!("application-menu-{}", info.window),
                egui::ImageData::Color(popup.image.clone()),
                egui::TextureOptions::NEAREST,
            );
            self.popup_textures
                .insert(info.window, (texture, popup.image.clone()));
        }
        let texture = self.popup_textures[&info.window].0.clone();
        let ppp = ctx.pixels_per_point();
        let physical = owner
            .and_then(crate::remote_geometry::screen_origin)
            .map(|origin| {
                geometry::menu_bounds(
                    origin,
                    anchor,
                    ppp,
                    scale,
                    info,
                    owner.and_then(crate::remote_geometry::work_area),
                )
            });
        let origin = physical
            .map(|r| egui::pos2(r.x as f32 / ppp, r.y as f32 / ppp))
            .unwrap_or_else(|| {
                ctx.input(|i| i.viewport().inner_rect.map(|r| r.min))
                    .unwrap_or_default()
                    + anchor.min.to_vec2()
                    + scale.logical(egui::vec2(info.x as f32, info.y as f32))
            });
        let size = physical
            .map(|r| egui::vec2(r.width as f32, r.height as f32) / ppp)
            .unwrap_or_else(|| scale.logical(egui::vec2(info.width as f32, info.height as f32)));
        let token = self.popup_title(info.window);
        let caption = "Application menu";
        let bound = self
            .popup_native
            .get(&info.window)
            .and_then(|w| w.handle())
            .is_some();
        let builder = egui::ViewportBuilder::default()
            .with_title(if bound { caption } else { &token })
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_transparent(true)
            .with_position(origin)
            .with_inner_size(size);
        let mut focused = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of(("application-menu", self.id, info.window)),
            builder,
            |ctx, _| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    self.send(Request::Key {
                        evdev: 1,
                        pressed: true,
                    });
                    self.send(Request::Key {
                        evdev: 1,
                        pressed: false,
                    });
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let (area, response) = ui.allocate_exact_size(
                            ui.available_size().max(egui::vec2(1.0, 1.0)),
                            egui::Sense::click_and_drag(),
                        );
                        // Native bounds already include parent DPI, zoom and work-area fitting.
                        // Using this popup's own DPI a second time would detach the hit targets.
                        let image = area;
                        ui.painter().image(
                            texture.id(),
                            image,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                        focused = self.application_input(
                            ui,
                            &response,
                            image,
                            (info.width, info.height),
                            (info.x, info.y),
                            false,
                            alive,
                        );
                    });
            },
        );
        let native = self.popup_native.entry(info.window).or_default();
        native.bind(&token);
        // The main title is obtained from the source; menu captions are hidden
        // from the taskbar and never used for identity or keyboard ownership.
        native.set_caption(caption);
        if let Some(hwnd) = native.handle() {
            if let Some(owner) = owner {
                if let Err(error) = crate::remote_geometry::set_owner(hwnd, owner) {
                    self.error = Some(error);
                }
            }
            if let Some(bounds) = physical {
                if let Err(error) = native.place(bounds) {
                    self.error = Some(error);
                }
            }
        }
        focused
    }
    fn application_keypad(&mut self, key: &crate::remote_keypad::KeypadEvent) {
        self.sync_modifiers(egui::Modifiers {
            shift: key.modifiers & 1 != 0,
            ctrl: key.modifiers & 2 != 0,
            alt: key.modifiers & 4 != 0,
            mac_cmd: key.modifiers & 8 != 0,
            command: key.modifiers & 2 != 0,
        });
        if key.pressed {
            self.keys.insert(key.scan_code);
        } else {
            self.keys.remove(&key.scan_code);
        }
        if !key.repeat {
            self.send(Request::Key {
                evdev: key.scan_code,
                pressed: key.pressed,
            });
        }
    }
    fn application_input(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        image: egui::Rect,
        size: (u32, u32),
        offset: (i32, i32),
        activate: bool,
        alive: bool,
    ) -> bool {
        if !alive {
            return false;
        }
        if let Some(client) = &self.cursor_client {
            self.cursor.paint(
                ui,
                image,
                image.width() / size.0.max(1) as f32,
                client.cursor(),
                response.hovered(),
            );
        }
        let scrolling = crate::remote_mouse::scroll_interaction(ui, response, image, false);
        if (response.hovered() && ui.input(|i| i.pointer.any_pressed())) || scrolling {
            response.request_focus();
            if activate {
                self.send(Request::Focus);
            }
        }
        let keyboard_focus = response.has_focus() && ui.input(|i| i.focused);
        // Menus receive hover events while the application keeps keyboard focus.
        if !keyboard_focus && !response.hovered() && self.buttons.is_empty() {
            return false;
        }
        let mut keypad = crate::remote_keypad::Batch::default();
        if keyboard_focus {
            let foreground = crate::remote_geometry::foreground();
            if self.input_window != foreground {
                self.release();
                self.input_window = foreground;
            }
            self.sync_modifiers(crate::remote_keyboard::current_modifiers(
                ui.input(|i| i.modifiers),
            ));
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            keypad = foreground
                .map(|window| crate::remote_keypad::take(self.id, window))
                .unwrap_or_default();
            if keypad.overflowed {
                self.release();
            }
        }
        let mut motion = None;
        let point = |pos| {
            let w = WindowInfo {
                id: 0,
                title: String::new(),
                width: size.0,
                height: size.1,
                transient_for: None,
            };
            let (x, y) = super::point(pos, image, &w);
            (x.saturating_add(offset.0), y.saturating_add(offset.1))
        };
        for event in ui.input(|i| i.events.clone()) {
            if keyboard_focus
                && matches!(&event, egui::Event::Key { key: egui::Key::Escape, pressed: true, modifiers, .. } if modifiers.ctrl && modifiers.alt)
                && !self.keys.contains(&100)
                && !keypad
                    .events
                    .iter()
                    .any(|key| key.scan_code == 100 && key.pressed)
            {
                self.release();
                response.surrender_focus();
                return false;
            }
            let keys = crate::remote_keypad::take_gui_ordered(&mut keypad.events, &event);
            if !keys.is_empty() {
                for key in &keys {
                    self.application_keypad(key);
                }
                continue;
            }
            match event {
                ref event
                    if keyboard_focus && crate::remote_clipboard::shortcut(event).is_some() =>
                {
                    let previous = self.modifiers;
                    self.sync_modifiers(egui::Modifiers {
                        ctrl: true,
                        command: true,
                        ..previous
                    });
                    let evdev = crate::remote_clipboard::shortcut(event).unwrap();
                    for pressed in [true, false] {
                        self.send(Request::Key { evdev, pressed });
                    }
                    self.sync_modifiers(previous);
                }
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } if keyboard_focus => {
                    if key == egui::Key::Escape && pressed && modifiers.ctrl && modifiers.alt {
                        self.release();
                        response.surrender_focus();
                        return false;
                    }
                    self.sync_modifiers(modifiers);
                    if !repeat {
                        if let Some(evdev) =
                            crate::remote_view::scan_code(physical_key.unwrap_or(key))
                        {
                            if pressed {
                                self.keys.insert(evdev);
                            } else {
                                self.keys.remove(&evdev);
                            }
                            self.send(Request::Key { evdev, pressed });
                        }
                    }
                }
                egui::Event::PointerMoved(pos)
                    if (response.hovered() && image.contains(pos)) || !self.buttons.is_empty() =>
                {
                    motion = Some(point(pos));
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } if image.contains(pos) || !self.buttons.is_empty() => {
                    self.sync_modifiers(modifiers);
                    let (x, y) = point(pos);
                    motion = None;
                    let button = crate::remote_mouse::gui_button(button);
                    if pressed {
                        self.buttons.insert(button);
                    } else {
                        self.buttons.remove(&button);
                    }
                    self.send(Request::Button {
                        button,
                        pressed,
                        x,
                        y,
                    });
                }
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                } if scrolling => {
                    self.sync_modifiers(modifiers);
                    if let Some(ad_stream_protocol::InputMessage::PointerWheel {
                        horizontal,
                        vertical,
                    }) = self.wheel.convert(unit, delta)
                    {
                        if let Some((x, y)) = motion
                            .take()
                            .or_else(|| ui.input(|i| i.pointer.hover_pos()).map(point))
                        {
                            self.send(Request::Move { x, y });
                        }
                        self.send(Request::Wheel {
                            horizontal,
                            vertical,
                        });
                    }
                }
                _ => {}
            }
        }
        for key in &keypad.events {
            self.application_keypad(key);
        }
        if keyboard_focus {
            self.sync_modifiers(crate::remote_keyboard::current_modifiers(
                ui.input(|i| i.modifiers),
            ));
        }
        if let Some((x, y)) = motion {
            self.send(Request::Move { x, y });
        }
        keyboard_focus
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_tiles_are_not_uploaded_again_and_skipped_paints_keep_every_patch() {
        use std::sync::Arc;
        let ctx = egui::Context::default();
        let mut app = Applications::new(None, uuid::Uuid::new_v4());
        let tile = |colour| Arc::new(egui::ColorImage::new([256, 1], vec![colour; 256]));
        let red = tile(egui::Color32::RED);
        let green = tile(egui::Color32::GREEN);
        let a = worker::Frame {
            sequence: 1,
            size: [512, 1],
            tiles: Arc::new(std::collections::BTreeMap::from([
                ((0, 0), red),
                ((256, 0), green),
            ])),
        };
        let _ = ctx.run(Default::default(), |ctx| app.upload_frame(ctx, &a));
        let id = app.texture.as_ref().unwrap().id();
        // Both source tiles changed while the UI skipped sequence 2.
        let blue = tile(egui::Color32::BLUE);
        let c = worker::Frame {
            sequence: 3,
            size: [512, 1],
            tiles: Arc::new(std::collections::BTreeMap::from([
                ((0, 0), blue.clone()),
                ((256, 0), tile(egui::Color32::YELLOW)),
            ])),
        };
        let output = ctx.run(Default::default(), |ctx| app.upload_frame(ctx, &c));
        let patches: Vec<_> = output
            .textures_delta
            .set
            .iter()
            .filter(|(texture, _)| *texture == id)
            .map(|(_, delta)| delta)
            .collect();
        assert!(patches.iter().all(|delta| delta.pos.is_some()));
        assert_eq!(
            patches
                .iter()
                .map(|delta| delta.image.size()[0] * delta.image.size()[1])
                .sum::<usize>(),
            512
        );
        let d = worker::Frame {
            sequence: 4,
            size: [512, 1],
            tiles: Arc::new(std::collections::BTreeMap::from([
                ((0, 0), blue),
                ((256, 0), tile(egui::Color32::WHITE)),
            ])),
        };
        let output = ctx.run(Default::default(), |ctx| app.upload_frame(ctx, &d));
        let patches: Vec<_> = output
            .textures_delta
            .set
            .iter()
            .filter(|(texture, _)| *texture == id)
            .map(|(_, delta)| delta)
            .collect();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].pos, Some([256, 0]));
        let output = ctx.run(Default::default(), |ctx| app.upload_frame(ctx, &d));
        assert!(!output
            .textures_delta
            .set
            .iter()
            .any(|(texture, _)| *texture == id));
    }
    #[test]
    fn clipboard_commands_only_reach_a_focused_application_and_release_temporary_control() {
        use std::sync::Arc;
        for (shift, ctrl) in [(false, false), (true, false), (false, true), (true, true)] {
            let modifiers = egui::Modifiers {
                shift,
                ctrl,
                command: ctrl,
                ..egui::Modifiers::NONE
            };
            crate::remote_keyboard::with_test_modifiers(modifiers, || {
                let ctx = egui::Context::default();
                ctx.options_mut(|o| o.max_passes = 1.try_into().unwrap());
                let (commands, mut requests) = tokio::sync::mpsc::channel(64);
                let mut app = Applications::new(None, uuid::Uuid::new_v4());
                app.worker = Some(Worker {
                    commands,
                    state: Arc::new(parking_lot::Mutex::new(State::default())),
                    cancel: tokio_util::sync::CancellationToken::new(),
                });
                for focused in [true, false] {
                    let _ = ctx.run(
                        egui::RawInput {
                            focused,
                            modifiers,
                            events: vec![
                                egui::Event::Copy,
                                egui::Event::Paste("private text".into()),
                                egui::Event::Cut,
                            ],
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let (image, response) = ui.allocate_exact_size(
                                    egui::vec2(300.0, 160.0),
                                    egui::Sense::click_and_drag(),
                                );
                                response.request_focus();
                                app.application_input(
                                    ui,
                                    &response,
                                    image,
                                    (300, 160),
                                    (0, 0),
                                    true,
                                    true,
                                );
                            });
                        },
                    );
                    let mut keys = Vec::new();
                    while let Ok(request) = requests.try_recv() {
                        if let Request::Key { evdev, pressed } = request {
                            keys.push((evdev, pressed));
                        }
                    }
                    if focused {
                        let mut expected = Vec::new();
                        if ctrl {
                            expected.push((29, true));
                        }
                        if shift {
                            expected.push((42, true));
                        }
                        // An already-held Ctrl belongs to the user: clipboard commands
                        // must not release it between C, V and X. A temporary Ctrl must
                        // instead be released after each command, preserving Shift.
                        expected.extend(if ctrl {
                            vec![
                                (46, true),
                                (46, false),
                                (47, true),
                                (47, false),
                                (45, true),
                                (45, false),
                            ]
                        } else {
                            vec![
                                (29, true),
                                (46, true),
                                (46, false),
                                (29, false),
                                (29, true),
                                (47, true),
                                (47, false),
                                (29, false),
                                (29, true),
                                (45, true),
                                (45, false),
                                (29, false),
                            ]
                        });
                        assert_eq!(
                            keys, expected,
                            "clipboard edges changed with Shift={shift}, Ctrl={ctrl}"
                        );
                    } else {
                        assert!(
                            keys.is_empty(),
                            "background application consumed a local clipboard command"
                        );
                    }
                    assert_eq!(app.modifiers.ctrl, ctrl);
                    assert_eq!(app.modifiers.shift, shift);
                }
                app.release();
                assert_eq!(app.modifiers, egui::Modifiers::NONE);
                if shift || ctrl {
                    assert!(
                        std::iter::from_fn(|| requests.try_recv().ok())
                            .any(|r| matches!(r, Request::Release)),
                        "Held modifiers must be released when the view loses capture"
                    );
                }
            });
        }
    }
    #[test]
    fn scrolling_activates_the_application_before_ordered_motion_and_wheel() {
        use std::sync::Arc;
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.max_passes = 1.try_into().unwrap());
        let (commands, mut requests) = tokio::sync::mpsc::channel(64);
        let mut app = Applications::new(None, uuid::Uuid::new_v4());
        app.worker = Some(Worker {
            commands,
            state: Arc::new(parking_lot::Mutex::new(State::default())),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        let mut text = String::new();
        let mut rect = egui::Rect::NOTHING;
        for stage in 0..4 {
            let pos = if stage == 3 {
                egui::pos2(450.0, 400.0)
            } else {
                egui::pos2(80.0, 80.0)
            };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if stage > 0 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, 1.0),
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run(egui::RawInput {
                focused: stage != 2,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 450.0))),
                events,
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let local = ui.text_edit_singleline(&mut text);
                    let (image, response) = ui.allocate_exact_size(egui::vec2(300.0, 160.0), egui::Sense::click_and_drag());
                    rect = image;
                    if stage == 0 { local.request_focus(); }
                    let captured = app.application_input(ui, &response, image, (300, 160), (0, 0), true, true);
                    if stage == 1 {
                        assert!(captured, "scroll did not activate the image while a local field owned keyboard focus");
                        assert!(!crate::remote_mouse::scroll_interaction(ui, &response, image, true), "overlay must block scrolling");
                        assert!(!crate::remote_mouse::scroll_interaction(ui, &response, egui::Rect::NOTHING, false), "letterbox margin must not scroll");
                    }
                });
            });
            let mut sent = Vec::new();
            while let Ok(request) = requests.try_recv() {
                sent.push(request);
            }
            if stage == 1 {
                let focus = sent
                    .iter()
                    .position(|r| matches!(r, Request::Focus))
                    .expect("missing activation");
                let movement = sent
                    .iter()
                    .position(|r| matches!(r, Request::Move { x, y } if *x >= 0 && *y >= 0))
                    .expect("missing pointer location");
                let wheel = sent
                    .iter()
                    .position(|r| {
                        matches!(
                            r,
                            Request::Wheel {
                                horizontal: 0,
                                vertical: 120
                            }
                        )
                    })
                    .expect("missing wheel");
                assert!(focus < movement && movement < wheel);
                assert_eq!(
                    sent.iter()
                        .filter(|r| matches!(r, Request::Wheel { .. }))
                        .count(),
                    1
                );
            } else {
                assert!(
                    !sent.iter().any(|r| matches!(r, Request::Wheel { .. })),
                    "background/outside scroll reached host at stage {stage}, image={rect:?}"
                );
            }
        }
    }
}
