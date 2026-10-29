use super::*;
use std::time::Instant;
fn pixel_image_rect(area: egui::Rect, size: egui::Vec2, pixels_per_point: f32) -> egui::Rect {
    egui::Rect::from_min_size(area.min, size / pixels_per_point.max(0.01))
}
impl Applications {
    pub(super) fn popup_title(&self, id: u32) -> String {
        format!("{} - menu {id}", self.title)
    }
    pub(super) fn viewport(&mut self, ctx: &egui::Context, state: &State) {
        let Some(window) = state.selected.clone() else {
            return;
        };
        let ppp = ctx.pixels_per_point().max(0.01);
        let builder = crate::app_layout::builder(
            ctx,
            self.id,
            &self.title,
            egui::vec2(window.width as f32 / ppp, window.height as f32 / ppp + 32.0),
        )
        .with_min_inner_size(egui::vec2(100.0, 80.0));
        let mut close = false;
        let mut any_focus = false;
        self.popup_textures
            .retain(|id, _| state.popups.iter().any(|p| p.info.window == *id));
        ctx.show_viewport_immediate(self.id, builder, |ctx, _| {
            crate::app_layout::update(ctx, "remote-application");
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
                return;
            }
            egui::TopBottomPanel::top("application-actions").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if crate::ui_icons::button(
                        ui,
                        crate::ui_icons::Icon::Tab,
                        "Stop viewing (keep application running)",
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
                        let size = ctx.available_rect().size() * ctx.pixels_per_point();
                        if wire::dimensions(size.x.max(64.0) as u32, size.y.max(64.0) as u32)
                            .is_ok()
                        {
                            self.send(Request::Resize {
                                width: size.x.max(64.0) as u32,
                                height: size.y.max(64.0) as u32,
                            });
                        }
                    }
                    ui.add(egui::Label::new(&window.title).truncate())
                        .on_hover_text(&window.title);
                });
            });
            let mut image_rect = egui::Rect::NOTHING;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if let Some((sequence, image)) = &state.image {
                        if *sequence != self.sequence
                            && image
                                .size
                                .iter()
                                .all(|side| *side <= ctx.input(|i| i.max_texture_side))
                        {
                            if self.texture.is_some()
                                && self
                                    .uploaded_image
                                    .as_ref()
                                    .is_some_and(|old| std::sync::Arc::ptr_eq(old, image))
                            {
                                // Only a menu changed: keep the main GPU resource.
                            } else if let Some(texture) =
                                self.texture.as_mut().filter(|t| t.size() == image.size)
                            {
                                texture.set(
                                    egui::ImageData::Color(image.clone()),
                                    egui::TextureOptions::NEAREST,
                                );
                            } else {
                                self.texture = Some(ctx.load_texture(
                                    format!("application-{:?}", self.id),
                                    egui::ImageData::Color(image.clone()),
                                    egui::TextureOptions::NEAREST,
                                ));
                            }
                            self.uploaded_image = Some(image.clone());
                            self.sequence = *sequence;
                        }
                    }
                    let (rect, response) = ui.allocate_exact_size(
                        ui.available_size().max(egui::vec2(1.0, 1.0)),
                        egui::Sense::click_and_drag(),
                    );
                    image_rect = pixel_image_rect(
                        rect,
                        egui::vec2(window.width as f32, window.height as f32),
                        ctx.pixels_per_point(),
                    );
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
                    let extent = (
                        (rect.width() * ctx.pixels_per_point()).round().max(64.0) as u32,
                        (rect.height() * ctx.pixels_per_point()).round().max(64.0) as u32,
                    );
                    if self.sized_window != Some(window.id) {
                        let header = (ctx.content_rect().height() - rect.height()).max(0.0);
                        let desired = egui::vec2(
                            window.width as f32 / ctx.pixels_per_point(),
                            window.height as f32 / ctx.pixels_per_point() + header,
                        );
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
                let owner = crate::remote_geometry::window(&self.title);
                for popup in &state.popups {
                    any_focus |= self.popup_view(ctx, popup, owner, image_rect, state.alive);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        });
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
            self.popup_textures.clear();
            self.sized_window = None;
        }
    }
    fn popup_view(
        &mut self,
        ctx: &egui::Context,
        popup: &worker::PopupState,
        owner: Option<isize>,
        anchor: egui::Rect,
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
        let origin = ctx
            .input(|i| i.viewport().inner_rect.map(|r| r.min))
            .unwrap_or_default()
            + anchor.min.to_vec2()
            + egui::vec2(info.x as f32, info.y as f32) / ppp;
        let title = self.popup_title(info.window);
        let builder = egui::ViewportBuilder::default()
            .with_title(&title)
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_transparent(true)
            .with_position(origin)
            .with_inner_size(egui::vec2(info.width as f32, info.height as f32) / ppp);
        let mut focused = false;
        let physical = owner
            .and_then(crate::remote_geometry::screen_origin)
            .map(|(x, y)| crate::remote_geometry::PhysicalRect {
                x: x + (anchor.left() * ppp).round() as i32 + info.x,
                y: y + (anchor.top() * ppp).round() as i32 + info.y,
                width: info.width,
                height: info.height,
            });
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
                        let image = pixel_image_rect(
                            area,
                            egui::vec2(info.width as f32, info.height as f32),
                            ctx.pixels_per_point(),
                        );
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
        if let Some(hwnd) = crate::remote_geometry::window(&title) {
            if let Some(owner) = owner {
                if let Err(error) = crate::remote_geometry::set_owner(hwnd, owner) {
                    self.error = Some(error);
                }
            }
            if let Some(bounds) = physical {
                if let Err(error) = crate::remote_geometry::place(hwnd, bounds, &title) {
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
    fn clipboard_commands_only_reach_a_focused_application_and_release_temporary_control() {
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
        for focused in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    focused,
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
                        app.application_input(ui, &response, image, (300, 160), (0, 0), true, true);
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
                assert_eq!(
                    keys,
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
                        (29, false)
                    ]
                );
            } else {
                assert!(
                    keys.is_empty(),
                    "background application consumed a local clipboard command"
                );
            }
            assert!(!app.modifiers.ctrl);
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
    #[test]
    fn menu_origin_adds_client_and_signed_popup_offsets_without_scaling_the_app() {
        let native_origin = egui::pos2(-1600.0, 40.0);
        let anchor = egui::Rect::from_min_size(egui::pos2(0.0, 32.0), egui::vec2(300.0, 160.0));
        let position = native_origin + anchor.min.to_vec2() + egui::vec2(290.0, -20.0) / 2.0;
        assert_eq!(position, egui::pos2(-1455.0, 62.0));
    }
    #[test]
    fn application_pixels_are_not_magnified_by_container_or_ui_scale() {
        let area = egui::Rect::from_min_size(egui::pos2(3.0, 7.0), egui::vec2(1200.0, 900.0));
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let rect = pixel_image_rect(area, egui::vec2(300.0, 160.0), ppp);
            assert_eq!(rect.min, area.min);
            assert!((rect.width() * ppp - 300.0).abs() < 0.01);
            assert!((rect.height() * ppp - 160.0).abs() < 0.01);
        }
    }
}
