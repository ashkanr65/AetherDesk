//! Terminal-first host-rendered application sessions. Files, credentials,
//! monitoring and explicit termination remain owned by the connection tab.
use crate::remote_applications::Applications;
use ad_stream_protocol::window_stream::Request;
use eframe::egui;
use std::collections::BTreeMap;

pub(crate) struct Workspace {
    directory: Applications,
    client: std::sync::Arc<crate::remote::RemoteClient>,
    seen: std::collections::BTreeSet<u32>,
    recently_closed: BTreeMap<u32, std::time::Instant>,
    clipboard: crate::remote_clipboard::Clipboard,
    last_clipboard: String,
    tick: std::time::Instant,
    layout: Option<(String, String)>,
    keyboard: crate::remote_keyboard::NativeKeyboard,
    monitor_layout: Option<(u32, u32, Vec<ad_stream_protocol::MonitorProperties>)>,
    monitor_tick: std::time::Instant,
    last_window: Option<u32>,
    views: BTreeMap<u32, Applications>,
    session: uuid::Uuid,
    command: String,
    search: String,
    expanded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_application_view_id_is_stable_and_scoped_to_its_session() {
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        assert_eq!(view_id(a, 1), view_id(a, 1));
        assert_ne!(view_id(a, 1), view_id(b, 1));
        assert_ne!(view_id(a, 1), view_id(a, 2));
    }
    #[test]
    fn application_panel_leaves_room_for_the_terminal_at_small_sizes_and_large_fonts() {
        for size in [
            egui::vec2(240.0, 220.0),
            egui::vec2(360.0, 320.0),
            egui::vec2(1000.0, 700.0),
        ] {
            for font in [13.0, 22.0, 30.0] {
                let ctx = egui::Context::default();
                ctx.style_mut(|s| {
                    for style in [egui::TextStyle::Body, egui::TextStyle::Button] {
                        s.text_styles
                            .insert(style, egui::FontId::proportional(font));
                    }
                });
                let mut workspace = Workspace::new(
                    crate::remote::RemoteClient::test_stub(ctx.clone()),
                    uuid::Uuid::new_v4(),
                );
                for _ in 0..3 {
                    let mut remaining = 0.0;
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                workspace.panel(ui);
                                remaining = ui.available_height();
                            });
                        },
                    );
                    assert!(remaining>size.y*0.45,"application panel displaced the terminal: {size:?}, font {font}, remaining {remaining}");
                }
            }
        }
    }
}
fn view_id(session: uuid::Uuid, window: u32) -> uuid::Uuid {
    let mut source = [0u8; 20];
    source[..16].copy_from_slice(session.as_bytes());
    source[16..].copy_from_slice(&window.to_le_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA256, &source);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}
impl Workspace {
    pub(crate) fn new(
        client: std::sync::Arc<crate::remote::RemoteClient>,
        session: uuid::Uuid,
    ) -> Self {
        Self {
            directory: Applications::new(client.window_target(), session),
            client,
            seen: Default::default(),
            recently_closed: Default::default(),
            clipboard: Default::default(),
            last_clipboard: String::new(),
            tick: std::time::Instant::now() - std::time::Duration::from_secs(2),
            layout: None,
            keyboard: crate::remote_keyboard::NativeKeyboard::new(),
            monitor_layout: None,
            monitor_tick: std::time::Instant::now() - std::time::Duration::from_secs(3),
            last_window: None,
            views: BTreeMap::new(),
            session,
            command: String::new(),
            search: String::new(),
            expanded: true,
        }
    }
    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) {
        if crate::ui_icons::button(
            ui,
            crate::ui_icons::Icon::Applications,
            "Show or hide applications",
            self.expanded,
        )
        .clicked()
        {
            self.expanded = !self.expanded;
        }
    }
    pub(crate) fn panel(&mut self, ui: &mut egui::Ui) {
        self.directory.start(ui.ctx());
        if !self.expanded {
            return;
        }
        ui.add_space(4.0);
        // A bounded panel leaves the terminal usable on small windows.
        let height = (ui.available_height() * 0.35).clamp(40.0, 210.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("application-panel",self.session))
                .max_height(height).auto_shrink([false,true]).show(ui,|ui| {
            ui.label("Remote Applications");
            let width = (ui.available_width()-36.0).max(40.0);
            let mut launch = false;
            ui.horizontal(|ui| {
                let edit = ui.add(egui::TextEdit::singleline(&mut self.command)
                    .desired_width(width).hint_text("Application command, e.g. xterm"));
                launch = edit.lost_focus() && ui.input(|i|i.key_pressed(egui::Key::Enter));
                launch |= crate::ui_icons::button(ui, crate::ui_icons::Icon::Go,
                    "Launch application on the host", false).clicked();
            });
            if launch && !self.command.trim().is_empty() {
                if self.command.len() > 2048 || self.command.contains(['\n','\r','\0']) {
                    self.directory.error = Some("Use one application command of at most 2048 bytes".into());
                } else {
                    self.directory.error=None;
                    self.directory.send(Request::Launch(self.command.trim().to_owned()));
                    self.command.clear();
                }
            }
            ui.add(egui::TextEdit::singleline(&mut self.search)
                .desired_width(ui.available_width()).hint_text("Search running applications"));
            let state = self.directory.worker.as_ref().map(|w|w.state.lock().clone()).unwrap_or_default();
            if let Some(error) = self.directory.error.as_ref().or(state.error.as_ref()) {
                ui.add(egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
            }
            let search = self.search.to_lowercase();
            egui::ScrollArea::vertical().id_salt(("application-list",self.session))
                .max_height(110.0).auto_shrink([false,true]).show(ui, |ui| {
                for window in &state.windows {
                    if !window.title.to_lowercase().contains(&search) { continue; }
                    let opened = self.views.contains_key(&window.id);
                    if ui.add(egui::Button::new(&window.title).selected(opened)
                        .truncate().min_size(egui::vec2(ui.available_width(),28.0))).clicked() {
                        if !opened && self.views.len()>=8 {
                            self.directory.error=Some("Eight independent windows are open. Close a view before opening another; the application stays running.".into());
                            continue;
                        }
                        self.seen.insert(window.id);
                        let view = self.views.entry(window.id).or_insert_with(|| {
                            // Stable, distinct native identities for simultaneous windows.
                            let id = view_id(self.session,window.id);
                            let mut view=Applications::new(self.directory.target.clone(),id);
                            view.managed=true;
                            view.cursor_client=Some(self.client.clone());
                            view
                        });
                        if !view.is_open() { view.open_window(ui.ctx(),window.id); }
                        else { view.focus_view(ui.ctx()); }
                    }
                }
                if state.windows.is_empty() {
                    ui.add(egui::Label::new("Launch an application above, or from the terminal. Click its name here to open its independent window.").wrap());
                }
            });
            });
        });
    }
    pub(crate) fn windows(&mut self, ctx: &egui::Context) {
        self.directory.start(ctx);
        let windows = self
            .directory
            .worker
            .as_ref()
            .map(|w| w.state.lock().windows.clone())
            .unwrap_or_default();
        self.recently_closed
            .retain(|_, time| time.elapsed() < std::time::Duration::from_secs(3));
        self.seen.retain(|id| {
            windows.iter().any(|w| w.id == *id)
                || self.views.contains_key(id)
                || self.recently_closed.contains_key(id)
        });
        for window in windows.iter().filter(|w| w.transient_for.is_none()) {
            if self.views.len() < 8 && self.seen.insert(window.id) {
                let mut view = Applications::new(
                    self.directory.target.clone(),
                    view_id(self.session, window.id),
                );
                view.cursor_client = Some(self.client.clone());
                view.managed = true;
                view.open_window(ctx, window.id);
                self.views.insert(window.id, view);
            }
        }
        if self.tick.elapsed() > std::time::Duration::from_millis(300) {
            self.tick = std::time::Instant::now();
            // A private canvas accommodates the largest local monitor; each
            // native view keeps its own DPI/placement. No combined desktop
            // image is sent for multiple client monitors.
            if self.monitor_tick.elapsed() > std::time::Duration::from_secs(2) {
                self.monitor_tick = std::time::Instant::now();
                let monitors = crate::remote_geometry::monitors();
                let width = monitors
                    .iter()
                    .map(|m| m.rect.width)
                    .max()
                    .unwrap_or(1920)
                    .clamp(640, 8192)
                    & !1;
                let height = monitors
                    .iter()
                    .map(|m| m.rect.height)
                    .max()
                    .unwrap_or(1080)
                    .clamp(480, 8192)
                    & !1;
                let properties: Vec<_> = monitors
                    .iter()
                    .max_by_key(|m| m.rect.width as u64 * m.rect.height as u64)
                    .map(crate::remote_geometry::monitor_properties)
                    .into_iter()
                    .collect();
                if self
                    .monitor_layout
                    .as_ref()
                    .is_none_or(|(w, h, p)| *w != width || *h != height || p != &properties)
                    && ad_stream_protocol::valid_desktop_size(width, height)
                {
                    if self
                        .client
                        .send(crate::remote::Request::MonitorLayout {
                            width,
                            height,
                            monitors: vec![ad_stream_protocol::Rectangle {
                                x: 0,
                                y: 0,
                                width,
                                height,
                            }],
                            properties: properties.clone(),
                        })
                        .is_ok()
                    {
                        self.monitor_layout = Some((width, height, properties));
                    }
                }
            }
            let focused = self.captures_keyboard();
            let state = self.client.status();
            if let Some(text) = &state.clipboard {
                self.last_clipboard = text.clone();
            }
            if let Some(text) = self
                .clipboard
                .poll(state.clipboard, focused)
                .filter(|text| text != &self.last_clipboard)
            {
                self.last_clipboard = text.clone();
                if let Err(error) = self.client.send(crate::remote::Request::Clipboard { text }) {
                    self.directory.error = Some(error);
                }
            }
            if focused {
                let layout = self
                    .keyboard
                    .layout()
                    .map(|(layout, variant)| (layout.to_owned(), variant.to_owned()));
                if layout != self.layout {
                    if let Some((layout, variant)) = &layout {
                        if let Err(error) =
                            self.client.send(crate::remote::Request::KeyboardLayout {
                                layout: layout.clone(),
                                variant: variant.clone(),
                            })
                        {
                            self.directory.error = Some(error);
                        }
                    }
                    self.layout = layout;
                }
            }
        }
        // Release the previous view before the new one injects held modifiers.
        // BTreeMap iteration order must not determine Ctrl/Shift state.
        for view in self.views.values_mut() {
            if !view.captures_keyboard() {
                view.release();
            }
        }
        if let Some((&id, _)) = self.views.iter().find(|(_, v)| v.captures_keyboard()) {
            self.last_window = Some(id);
        }
        for view in self.views.values_mut() {
            view.ui(ctx);
        }
        if let Some(error) = self.views.values().filter(|v| !v.is_open()).find_map(|v| {
            v.error
                .clone()
                .or_else(|| v.worker.as_ref().and_then(|w| w.state.lock().error.clone()))
        }) {
            self.directory.error = Some(error);
        }
        for (&id, _) in self.views.iter().filter(|(_, view)| !view.is_open()) {
            self.recently_closed.insert(id, std::time::Instant::now());
        }
        self.views.retain(|_, v| v.is_open());
    }
    pub(crate) fn captures_keyboard(&self) -> bool {
        self.views.values().any(Applications::captures_keyboard)
    }
    pub(crate) fn capture(&self) -> Option<crate::remote::Image> {
        let view = self
            .last_window
            .and_then(|id| self.views.get(&id))
            .or_else(|| self.views.values().next())?;
        let state = view.worker.as_ref()?.state.lock().clone();
        let image = state.image?;
        let sequence = image.sequence;
        let pixels = image.bgra();
        Some(crate::remote::Image {
            width: image.size[0] as u32,
            height: image.size[1] as u32,
            sequence,
            pixels: std::sync::Arc::new(pixels.into()),
            damage: None,
        })
    }
}
