use crate::remote::{RemoteClient, Request};
use ad_stream_protocol::InputMessage;
use eframe::egui;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
#[path = "remote_presentation.rs"]
mod presentation;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Tab,
    Current,
    All,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAction {
    Disconnect,
    End,
}
pub struct RemoteView {
    action: Option<SessionAction>,
    input_focus: bool,
    pub client: Arc<RemoteClient>,
    texture: Option<egui::TextureHandle>,
    sequence: u64,
    size: (u32, u32),
    pending_layout: Option<crate::remote_geometry::Layout>,
    sent_layout: Option<crate::remote_geometry::Layout>,
    monitor_properties: Vec<ad_stream_protocol::MonitorProperties>,
    pending_monitor_properties: Vec<ad_stream_protocol::MonitorProperties>,
    sent_monitor_properties: Vec<ad_stream_protocol::MonitorProperties>,
    resize_due: Option<Instant>,
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u8>,
    modifiers: u16,
    mode: Mode,
    resolution_scale: f32,
    tab_target: (u32, u32),
    resolution_settings: bool,
    display_settings: bool,
    placement: crate::remote_toolbar::Placement,
    placement_valid: bool,
    toolbar_rect: Option<egui::Rect>,
    toolbar_drag: Option<egui::Vec2>,
    native_keyboard: crate::remote_keyboard::NativeKeyboard,
    raw_active: bool,
    keyboard_paused: bool,
    keyboard_layout: Option<(String, String)>,
    layout_tick: Instant,
    monitor_snapshot: Vec<crate::remote_geometry::Monitor>,
    monitor_tick: Instant,
    selected_monitor: Option<String>,
    window_title: String,
    window: Option<isize>,
    id: egui::ViewportId,
    last_pointer: (i32, i32),
    error: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    last_clipboard: String,
    clipboard_tick: Instant,
    toolbar_until: Instant,
}
impl RemoteView {
    fn native_surface(
        &self,
        window: isize,
        ctx: &egui::Context,
        viewport: egui::Rect,
        image: egui::Rect,
    ) -> Option<crate::remote_keyboard::Surface> {
        let (width, height) = crate::remote_geometry::client_size(window)?;
        let (x, y) = crate::remote_geometry::screen_origin(window)?;
        let scale_x = width as f64 / viewport.width().max(1.0) as f64;
        let scale_y = height as f64 / viewport.height().max(1.0) as f64;
        let physical = |rect: egui::Rect| crate::remote_geometry::PhysicalRect {
            x: (x as f64 + (rect.left() - viewport.left()) as f64 * scale_x).round() as i32,
            y: (y as f64 + (rect.top() - viewport.top()) as f64 * scale_y).round() as i32,
            width: (rect.width() as f64 * scale_x).round().max(1.0) as u32,
            height: (rect.height() as f64 * scale_y).round().max(1.0) as u32,
        };
        Some(crate::remote_keyboard::Surface {
            image: physical(image),
            size: self.size,
            toolbar: if self.mode == Mode::Tab {
                None
            } else {
                self.toolbar_capture_rect(ctx).map(physical)
            },
        })
    }
    pub fn new(client: Arc<RemoteClient>, id: uuid::Uuid) -> Self {
        let saved = crate::remote_toolbar::load();
        let placement_valid = saved.is_ok();
        let error = saved.as_ref().err().cloned();
        let monitors = crate::remote_geometry::monitors();
        let monitor_properties = monitors
            .iter()
            .map(crate::remote_geometry::monitor_properties)
            .collect();
        Self {
            action: None,
            input_focus: false,
            client,
            texture: None,
            sequence: 0,
            size: (1280, 720),
            pending_layout: None,
            sent_layout: None,
            monitor_properties,
            pending_monitor_properties: vec![],
            sent_monitor_properties: vec![],
            resize_due: None,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            modifiers: 0,
            mode: Mode::Tab,
            resolution_scale: 1.0,
            tab_target: (1280, 720),
            resolution_settings: false,
            display_settings: false,
            placement: saved.unwrap_or_default(),
            placement_valid,
            toolbar_rect: None,
            toolbar_drag: None,
            native_keyboard: crate::remote_keyboard::NativeKeyboard::new(),
            raw_active: false,
            keyboard_paused: false,
            keyboard_layout: None,
            layout_tick: Instant::now() - Duration::from_secs(1),
            monitor_snapshot: monitors,
            monitor_tick: Instant::now(),
            selected_monitor: None,
            window_title: format!("AetherDesk Remote Desktop {id}"),
            window: None,
            id: egui::ViewportId::from_hash_of(id),
            last_pointer: (0, 0),
            error,
            clipboard: arboard::Clipboard::new().ok(),
            last_clipboard: String::new(),
            clipboard_tick: Instant::now(),
            toolbar_until: Instant::now() + Duration::from_secs(2),
        }
    }
    pub fn take_action(&mut self) -> Option<SessionAction> {
        self.action.take()
    }
    pub fn captures_keyboard(&self) -> bool {
        self.input_focus
    }
    fn session_action(&mut self, action: SessionAction) {
        self.release();
        self.set_mode(Mode::Tab);
        self.action = Some(action);
    }
    fn send(&mut self, request: Request) -> bool {
        match self.client.send(request) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
    fn input(&mut self, event: InputMessage) {
        self.send(Request::Input { event });
    }
    pub fn release(&mut self) {
        self.input_focus = false;
        self.native_keyboard.release();
        self.raw_active = false;
        for scan_code in std::mem::take(&mut self.keys) {
            self.input(InputMessage::Keyboard {
                scan_code,
                pressed: false,
                modifiers: 0,
                repeat: false,
            });
        }
        for button in std::mem::take(&mut self.buttons) {
            let (x, y) = self.last_pointer;
            self.input(InputMessage::PointerButton {
                button,
                pressed: false,
                x,
                y,
            });
        }
        self.sync_modifiers(egui::Modifiers::default());
    }
    fn sync_modifiers(&mut self, m: egui::Modifiers) {
        let new = u16::from(m.shift)
            | (u16::from(m.ctrl) << 1)
            | (u16::from(m.alt) << 2)
            | (u16::from(m.mac_cmd) << 3)
            | lock_state();
        for (bit, scan_code) in [(1, 42), (2, 29), (4, 56), (8, 125)] {
            if self.modifiers & bit != new & bit {
                self.input(InputMessage::Keyboard {
                    scan_code,
                    pressed: new & bit != 0,
                    modifiers: new,
                    repeat: false,
                });
            }
        }
        self.modifiers = new;
    }
    fn surface(&mut self, ui: &mut egui::Ui) {
        if let Some(image) = self.client.prepared_after(self.sequence) {
            let (position, pixels) = image.upload_after(self.sequence, self.texture.is_some());
            let color = egui::ImageData::Color(pixels);
            if let Some(texture) = &mut self.texture {
                if let Some(position) = position {
                    texture.set_partial(position, color, egui::TextureOptions::LINEAR);
                } else {
                    texture.set(color, egui::TextureOptions::LINEAR);
                }
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    format!("remote-{:?}", self.id),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
            self.sequence = image.sequence;
            self.size = (image.width, image.height);
        }
        let available = ui.available_size().max(egui::vec2(1.0, 1.0));
        let response = surface_response(ui, self.id, available);
        let rect = response.rect;
        let native_window = if self.mode == Mode::Tab {
            if ui.input(|i| i.focused) {
                crate::remote_geometry::foreground()
            } else {
                None
            }
        } else {
            self.window
        };
        let viewport = ui.ctx().content_rect();
        let pixels = native_window.and_then(crate::remote_geometry::client_size);
        let target = if let Some((width, height)) = pixels {
            crate::remote_geometry::single_size(
                (rect.width() / viewport.width().max(1.0) * width as f32).round() as u32,
                (rect.height() / viewport.height().max(1.0) * height as f32).round() as u32,
            )
        } else {
            crate::remote_geometry::single_size(
                (rect.width() * ui.ctx().pixels_per_point()).round() as u32,
                (rect.height() * ui.ctx().pixels_per_point()).round() as u32,
            )
        };
        self.tab_target = target;
        let layout = self.layout_for(self.resolution_scale).ok();
        if let Some(layout) = layout {
            let properties = if self.mode == Mode::All {
                self.monitor_properties.clone()
            } else {
                let device = if self.mode == Mode::Tab {
                    native_window.and_then(crate::remote_geometry::window_monitor)
                } else {
                    self.selected_monitor.clone()
                };
                crate::remote_geometry::select(&self.monitor_snapshot, device.as_deref())
                    .and_then(|m| {
                        self.monitor_properties
                            .iter()
                            .find(|p| p.name == m.device)
                            .cloned()
                    })
                    .into_iter()
                    .collect()
            };
            // Geometry and metadata travel together, only after a real change.
            let properties = if properties.len() == layout.monitors.len() {
                properties
            } else {
                vec![]
            };
            if self.pending_layout.as_ref() != Some(&layout)
                || self.pending_monitor_properties != properties
            {
                self.pending_monitor_properties = properties.clone();
                self.pending_layout = Some(layout.clone());
                self.resize_due = Some(Instant::now() + Duration::from_millis(180));
            }
            if self.resize_due.is_some_and(|due| Instant::now() >= due) {
                if self.sent_layout.as_ref() == Some(&layout)
                    && self.sent_monitor_properties == properties
                {
                    self.resize_due = None;
                } else if self.send(Request::MonitorLayout {
                    width: layout.width,
                    height: layout.height,
                    monitors: layout.monitors.clone(),
                    properties: properties.clone(),
                }) {
                    self.sent_layout = Some(layout);
                    self.sent_monitor_properties = properties;
                    self.resize_due = None;
                } else {
                    self.resize_due = Some(Instant::now() + Duration::from_millis(250));
                }
            }
        }
        let ratio = (rect.width() / self.size.0 as f32).min(rect.height() / self.size.1 as f32);
        let image_rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(self.size.0 as f32 * ratio, self.size.1 as f32 * ratio),
        );
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for the desktop…",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
        }
        if self.display_settings || self.resolution_settings {
            self.release();
            self.keyboard_paused = true;
            response.surrender_focus();
            // Keep the existing idle cadence so resize debounce and video still
            // advance while the local options window owns keyboard input.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            return;
        }
        if self.native_keyboard.take_exit() {
            self.release();
            self.keyboard_paused = true;
            response.surrender_focus();
            if self.mode != Mode::Tab {
                self.set_mode(Mode::Tab);
            }
            return;
        }
        let toolbar_blocked = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| self.pointer_is_toolbar(ui.ctx(), p));
        if ui.input(|i| i.pointer.any_pressed()) && (toolbar_blocked || !response.hovered()) {
            self.release();
            self.keyboard_paused = true;
            response.surrender_focus();
        }
        if !toolbar_blocked
            && (response.clicked() || (response.hovered() && ui.input(|i| i.pointer.any_pressed())))
        {
            self.keyboard_paused = false;
            response.request_focus();
        }
        restore_surface_focus(ui, &response, self.keyboard_paused);
        // A hook lease can expire while a modal or slow frame is active. Never
        // suppress GUI key events merely because the previous frame owned it.
        if self.raw_active && !self.native_keyboard.active() {
            self.release();
        }
        if self.raw_active && ui.input(|i| i.events.iter().any(gui_key_reached_window)) {
            // An exclusive hook would have swallowed these keys. Do not throw
            // away the only copy when a hook expires, is removed by Windows,
            // or passes an event through during a focus/route transition.
            self.release();
            self.native_keyboard.recover();
        }
        let focused = response.has_focus()
            && ui.input(|i| i.focused)
            && !self.keyboard_paused
            && !(toolbar_blocked && ui.input(|i| i.pointer.any_pressed()));
        if !focused {
            self.release();
        } else {
            let composing = native_window.is_some_and(|w| self.native_keyboard.composing(w));
            let capture = self.native_keyboard.ready() && !composing;
            if !capture && self.raw_active {
                self.release();
            }
            if self.layout_tick.elapsed() > Duration::from_millis(16) {
                self.layout_tick = Instant::now();
                if let Some((layout, variant)) = self.native_keyboard.layout() {
                    let value = (layout.to_owned(), variant.to_owned());
                    if self.keyboard_layout.as_ref() != Some(&value)
                        && self.send(Request::KeyboardLayout {
                            layout: value.0.clone(),
                            variant: value.1.clone(),
                        })
                    {
                        self.keyboard_layout = Some(value);
                    }
                }
            }
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
            for event in ui.input(|i| i.events.clone()) {
                match event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed,
                        repeat,
                        modifiers,
                        ..
                    } if !self.raw_active => {
                        if key == egui::Key::Escape && pressed && modifiers.ctrl && modifiers.alt {
                            self.release();
                            self.keyboard_paused = true;
                            response.surrender_focus();
                            if self.mode != Mode::Tab {
                                self.set_mode(Mode::Tab);
                            }
                            break;
                        }
                        self.sync_modifiers(modifiers);
                        if let Some(scan_code) = scan_code(physical_key.unwrap_or(key)) {
                            if pressed {
                                self.keys.insert(scan_code);
                            } else {
                                self.keys.remove(&scan_code);
                            }
                            self.input(InputMessage::Keyboard {
                                scan_code,
                                pressed,
                                repeat,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                        self.input(InputMessage::UnicodeText { text })
                    }
                    egui::Event::PointerMoved(pos)
                        if !self.raw_active
                            && ((!toolbar_blocked
                                && response.hovered()
                                && image_rect.contains(pos))
                                || !self.buttons.is_empty()) =>
                    {
                        let (x, y) = point(pos, image_rect, self.size);
                        self.last_pointer = (x, y);
                        self.input(InputMessage::PointerMove { x, y, monitor: 0 });
                    }
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } if !self.raw_active
                        && ((!toolbar_blocked
                            && response.hovered()
                            && image_rect.contains(pos))
                            || !self.buttons.is_empty()) =>
                    {
                        let (x, y) = point(pos, image_rect, self.size);
                        self.last_pointer = (x, y);
                        let button = match button {
                            egui::PointerButton::Primary => 1,
                            egui::PointerButton::Middle => 2,
                            egui::PointerButton::Secondary => 3,
                            egui::PointerButton::Extra1 => 8,
                            egui::PointerButton::Extra2 => 9,
                        };
                        if pressed {
                            self.buttons.insert(button);
                        } else {
                            self.buttons.remove(&button);
                        }
                        self.input(InputMessage::PointerButton {
                            button,
                            pressed,
                            x,
                            y,
                        });
                    }
                    egui::Event::MouseWheel { unit, delta, .. }
                        if !self.raw_active && response.hovered() && !toolbar_blocked =>
                    {
                        let scale = match unit {
                            egui::MouseWheelUnit::Point => 3.0,
                            egui::MouseWheelUnit::Line => 120.0,
                            egui::MouseWheelUnit::Page => 360.0,
                        };
                        self.input(InputMessage::PointerWheel {
                            horizontal: (delta.x * scale) as i32,
                            vertical: (delta.y * scale) as i32,
                        });
                    }
                    _ => {}
                }
            }
            if !self.raw_active && !self.keyboard_paused {
                self.sync_modifiers(ui.input(|i| i.modifiers));
            }
            // Finish the current GUI event batch before acquiring native capture,
            // so the click that focuses the surface cannot be lost or doubled.
            if capture && !self.keyboard_paused {
                if let Some((window, mapping)) = native_window.and_then(|w| {
                    self.native_surface(w, ui.ctx(), viewport, image_rect)
                        .map(|mapping| (w, mapping))
                }) {
                    let buttons = std::mem::take(&mut self.buttons);
                    if !self.raw_active {
                        self.release();
                    }
                    self.raw_active =
                        self.native_keyboard
                            .activate(window, &self.client, mapping, &buttons);
                    if !self.raw_active {
                        self.native_keyboard.release();
                        self.buttons = buttons;
                    }
                } else if self.raw_active {
                    self.release();
                }
            }
        }
        self.input_focus = focused && !self.keyboard_paused;
        if self.clipboard_tick.elapsed() > Duration::from_millis(300) {
            self.clipboard_tick = Instant::now();
            let state = self.client.status();
            if let Some(text) = state.clipboard {
                self.last_clipboard = text.clone();
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text);
                }
            } else if focused {
                if let Some(text) = self
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.get_text().ok())
                    .filter(|t| t.len() <= 1024 * 1024 && t != &self.last_clipboard)
                {
                    self.last_clipboard = text.clone();
                    self.send(Request::Clipboard { text });
                }
            }
        }
        if self.native_keyboard.unavailable() {
            self.error.get_or_insert("Windows key capture could not start. Ordinary keys and the explicit Ctrl+Alt+Del button remain available.".into());
        }
        if let Some(error) = &self.error {
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                error,
                egui::FontId::proportional(15.0),
                egui::Color32::LIGHT_RED,
            );
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}
impl Drop for RemoteView {
    fn drop(&mut self) {
        self.release();
    }
}
fn surface_response(ui: &mut egui::Ui, id: egui::ViewportId, size: egui::Vec2) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    // Both auto IDs and parent UI scopes can change as surrounding controls
    // change. The desktop's focus identity belongs to the session itself.
    ui.interact(
        rect,
        egui::Id::new(("remote-surface", id)),
        egui::Sense::click_and_drag(),
    )
}
fn restore_surface_focus(ui: &egui::Ui, response: &egui::Response, paused: bool) {
    // Reclaim an orphaned focus after activation/layout changes, but respect
    // deliberate Ctrl+Alt+Esc release and all focused local inputs/dialogs.
    if !paused
        && ui.is_enabled()
        && ui.input(|i| i.focused)
        && ui.memory(|memory| memory.focused().is_none())
    {
        response.request_focus();
        tracing::info!("remote desktop keyboard focus restored");
    }
}
fn gui_key_reached_window(event: &egui::Event) -> bool {
    match event {
        egui::Event::Key {
            key, physical_key, ..
        } => {
            // The native route intentionally passes lock keys to Windows so
            // its lock indicators work. They are not evidence of hook loss.
            scan_code(physical_key.unwrap_or(*key))
                .is_some_and(|scan| !matches!(scan, 58 | 69 | 70))
        }
        _ => false,
    }
}
fn point(pos: egui::Pos2, rect: egui::Rect, size: (u32, u32)) -> (i32, i32) {
    (
        ((pos.x - rect.left()) / rect.width() * size.0 as f32).clamp(0.0, (size.0 - 1) as f32)
            as i32,
        ((pos.y - rect.top()) / rect.height() * size.1 as f32).clamp(0.0, (size.1 - 1) as f32)
            as i32,
    )
}
fn scan_code(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        Escape => 1,
        Num1 | Exclamationmark => 2,
        Num2 => 3,
        Num3 => 4,
        Num4 => 5,
        Num5 => 6,
        Num6 => 7,
        Num7 => 8,
        Num8 => 9,
        Num9 => 10,
        Num0 => 11,
        Minus => 12,
        Equals | Plus => 13,
        Backspace => 14,
        Tab => 15,
        Q => 16,
        W => 17,
        E => 18,
        R => 19,
        T => 20,
        Y => 21,
        U => 22,
        I => 23,
        O => 24,
        P => 25,
        OpenBracket | OpenCurlyBracket => 26,
        CloseBracket | CloseCurlyBracket => 27,
        Enter => 28,
        A => 30,
        S => 31,
        D => 32,
        F => 33,
        G => 34,
        H => 35,
        J => 36,
        K => 37,
        L => 38,
        Semicolon | Colon => 39,
        Quote => 40,
        Backtick => 41,
        Backslash | Pipe => 43,
        Z => 44,
        X => 45,
        C => 46,
        V => 47,
        B => 48,
        N => 49,
        M => 50,
        Comma => 51,
        Period => 52,
        Slash | Questionmark => 53,
        Space => 57,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        F11 => 87,
        F12 => 88,
        F13 => 183,
        F14 => 184,
        F15 => 185,
        F16 => 186,
        F17 => 187,
        F18 => 188,
        F19 => 189,
        F20 => 190,
        F21 => 191,
        F22 => 192,
        F23 => 193,
        F24 => 194,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        _ => return None,
    })
}
fn lock_state() -> u16 {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn GetKeyState(key: i32) -> i16;
        }
        use ad_stream_protocol::{
            MODIFIER_CAPS_LOCK, MODIFIER_LOCK_STATE_VALID, MODIFIER_NUM_LOCK,
        };
        MODIFIER_LOCK_STATE_VALID
            | if unsafe { GetKeyState(0x14) } & 1 != 0 {
                MODIFIER_CAPS_LOCK
            } else {
                0
            }
            | if unsafe { GetKeyState(0x90) } & 1 != 0 {
                MODIFIER_NUM_LOCK
            } else {
                0
            }
    }
    #[cfg(not(windows))]
    {
        0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_surface_focus_survives_changing_controls_and_locks_navigation_keys() {
        let ctx = egui::Context::default();
        let id = egui::ViewportId::from_hash_of("keyboard-focus-fixture");
        let mut focus = None;
        for count in [0, 3, 1, 4, 0] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.push_id(count, |ui| {
                            for _ in 0..count {
                                let _ = ui.button("Changing control");
                            }
                            let size = ui.available_size();
                            let response = surface_response(ui, id, size);
                            if let Some(expected) = focus {
                                assert_eq!(
                                    response.id, expected,
                                    "controls changed the desktop key-focus identity"
                                );
                                assert!(response.has_focus());
                            }
                            response.request_focus();
                            focus = Some(response.id);
                        });
                    });
                },
            );
        }
        for (key, expected) in [
            (egui::Key::A, 30),
            (egui::Key::E, 18),
            (egui::Key::F, 33),
            (egui::Key::B, 48),
            (egui::Key::U, 22),
            (egui::Key::K, 37),
            (egui::Key::W, 17),
            (egui::Key::Y, 21),
            (egui::Key::T, 20),
            (egui::Key::Minus, 12),
            (egui::Key::Plus, 13),
            (egui::Key::Tab, 15),
            (egui::Key::F24, 194),
        ] {
            assert_eq!(scan_code(key), Some(expected));
        }
    }
    #[test]
    fn desktop_focus_recovers_after_activation_but_respects_local_fields_and_release() {
        let ctx = egui::Context::default();
        let id = egui::ViewportId::from_hash_of("focus-recovery");
        let mut local_text = String::new();
        for stage in 0..6 {
            let _ = ctx.run(
                egui::RawInput {
                    focused: stage != 1,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let local = ui.text_edit_singleline(&mut local_text);
                        let size = ui.available_size();
                        let remote = surface_response(ui, id, size);
                        match stage {
                            1 | 2 => remote.surrender_focus(),
                            3 => local.request_focus(),
                            4 => local.surrender_focus(),
                            _ => {}
                        }
                        restore_surface_focus(ui, &remote, stage == 4);
                        match stage {
                            0 | 2 | 5 => assert!(
                                remote.has_focus(),
                                "desktop did not recover at stage {stage}"
                            ),
                            1 => assert!(!remote.has_focus(), "unfocused viewport captured input"),
                            3 => {
                                assert!(local.has_focus());
                                assert!(!remote.has_focus());
                            }
                            4 => assert!(!remote.has_focus(), "explicit release was ignored"),
                            _ => unreachable!(),
                        }
                    });
                },
            );
        }
    }
    #[test]
    fn gui_keys_bypass_stale_native_capture_without_replaying_text_events() {
        for key in [
            egui::Key::A,
            egui::Key::Enter,
            egui::Key::Tab,
            egui::Key::ArrowLeft,
            egui::Key::F10,
        ] {
            for pressed in [true, false] {
                assert!(gui_key_reached_window(&egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }));
            }
        }
        // Text accompanies ordinary key events. Forwarding it as another key
        // would duplicate letters, so only physical key edges trigger recovery.
        assert!(!gui_key_reached_window(&egui::Event::Text("a".into())));
        assert!(!gui_key_reached_window(&egui::Event::PointerGone));
        assert!(!gui_key_reached_window(&egui::Event::Ime(
            egui::ImeEvent::Commit("a".into())
        )));
    }
    #[test]
    fn scaled_coordinates_stay_inside_frame() {
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 50.0));
        assert_eq!(point(egui::pos2(60.0, 45.0), r, (200, 100)), (100, 50));
        assert_eq!(point(egui::pos2(900.0, -9.0), r, (200, 100)), (199, 0));
    }
    #[test]
    fn remote_shortcuts_use_evdev_codes() {
        assert_eq!(scan_code(egui::Key::W), Some(17));
        assert_eq!(scan_code(egui::Key::End), Some(107));
        assert_eq!(scan_code(egui::Key::F12), Some(88));
    }
}
