//! Presentation windows, physical monitor placement and the saved control bar.
use super::*;
impl RemoteView {
    pub(super) fn set_mode(&mut self, mode: Mode) {
        self.release();
        self.mode = mode;
        self.sent_layout = None;
        self.pending_layout = None;
        self.pending_monitor_properties.clear();
        self.sent_monitor_properties.clear();
        self.resize_due = None;
        self.toolbar_drag = None;
        self.toolbar_until = Instant::now() + Duration::from_secs(2);
        if mode == Mode::Current {
            self.selected_monitor = crate::remote_geometry::cursor_monitor();
        }
        if mode == Mode::Tab {
            self.window = None;
            self.toolbar_rect = None;
        }
        // Validate against the newly selected monitor, after updating its ID.
        // A scale valid on one display may exceed limits across all monitors.
        if self.layout_for(self.resolution_scale).is_err() {
            self.resolution_scale = 1.0;
        }
    }
    pub(super) fn fullscreen_layout(&self) -> Option<crate::remote_geometry::Layout> {
        if self.mode == Mode::Tab {
            None
        } else {
            self.layout_for(self.resolution_scale).ok()
        }
    }
    pub(super) fn layout_for(&self, scale: f32) -> Result<crate::remote_geometry::Layout, String> {
        use crate::remote_geometry::LayoutTarget;
        let target = match self.mode {
            Mode::Tab => LayoutTarget::Tab(self.tab_target.0, self.tab_target.1),
            Mode::Current => LayoutTarget::Current(self.selected_monitor.as_deref()),
            Mode::All => LayoutTarget::All,
        };
        crate::remote_geometry::session_layout(&self.monitor_snapshot, target, scale)
    }
    fn toggle_resolution(&mut self) {
        self.resolution_settings = !self.resolution_settings;
        self.display_settings = false;
        self.release();
        self.keyboard_paused = true;
    }
    fn apply_resolution(&mut self, scale: f32) {
        if scale != self.resolution_scale && self.layout_for(scale).is_ok() {
            self.resolution_scale = scale;
            self.pending_layout = None;
            self.resize_due = None;
        }
    }
    fn resolution_preferences(&mut self, ctx: &egui::Context) {
        if !self.resolution_settings {
            return;
        }
        let mut open = true;
        let mut selected = self.resolution_scale;
        egui::Window::new("Remote resolution")
            .id(egui::Id::new(("remote-resolution-options", self.id)))
            .open(&mut open).resizable(false).collapsible(false)
            .default_width(330.0).max_width((ctx.content_rect().width() - 32.0).clamp(100.0, 330.0))
            .max_height((ctx.content_rect().height() - 48.0).max(80.0)).vscroll(true)
            .constrain_to(ctx.content_rect())
            .show(ctx, |ui| {
                ui.add(egui::Label::new("Choose the virtual desktop size. 100% follows this window or the selected monitors.").wrap());
                ui.separator();
                for scale in [0.5_f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
                    let layout = self.layout_for(scale);
                    let label = match &layout {
                        Ok(layout) => format!("{} × {} · {}%", layout.width, layout.height, (scale * 100.0).round()),
                        Err(_) => format!("{}% · exceeds supported size", (scale * 100.0).round()),
                    };
                    if ui.add_enabled(layout.is_ok(), egui::Button::new(label).selected(selected == scale)
                        .truncate().min_size(egui::vec2(ui.available_width(), 26.0))).clicked() {
                        selected = scale;
                    }
                }
                ui.separator();
                ui.add(egui::Label::new("All-monitor mode preserves the monitor arrangement. Your Windows display settings stay unchanged.").wrap());
            });
        if selected != self.resolution_scale {
            self.apply_resolution(selected);
            ctx.request_repaint();
        }
        self.resolution_settings = open;
    }
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        use crate::ui_icons::{button, Icon};
        if self.mode == Mode::Tab {
            if button(ui, Icon::Monitor, "Use this monitor", false).clicked() {
                self.set_mode(Mode::Current);
            }
            if button(ui, Icon::Monitors, "Use all monitors", false).clicked() {
                self.set_mode(Mode::All);
            }
        } else if button(ui, Icon::Tab, "Return to tab", false).clicked() {
            self.set_mode(Mode::Tab);
        }
        if button(ui, Icon::SecureKeys, "Send Ctrl+Alt+Delete", false).clicked() {
            self.secure_keys();
        }
        if button(
            ui,
            Icon::Resolution,
            "Change remote resolution",
            self.resolution_settings,
        )
        .clicked()
        {
            self.toggle_resolution();
        }
        if button(
            ui,
            Icon::Settings,
            "Remote display settings",
            self.display_settings,
        )
        .clicked()
        {
            self.display_settings = !self.display_settings;
            self.resolution_settings = false;
            self.release();
            self.keyboard_paused = true;
        }
    }
    fn display_preferences(&mut self, ctx: &egui::Context) {
        if !self.display_settings {
            return;
        }
        let mut open = self.display_settings;
        egui::Window::new("Remote display settings").open(&mut open).default_width(480.0).show(ctx,|ui| {
            ui.label("Detected client monitors are exposed as virtual monitors to this remote desktop only.");
            for monitor in &self.monitor_properties {
                ui.label(format!("{}: {} DPI, {} x {} mm, {} Hz",monitor.name,monitor.dpi,monitor.width_mm,monitor.height_mm,monitor.refresh_millihz/1000));
            }
            ui.separator();
            ui.label("Virtual desktop resolution (100% matches the monitor pixels)");
            let before=self.resolution_scale;
            egui::ComboBox::from_id_salt(("remote-resolution",self.id)).selected_text(format!("{}%",(self.resolution_scale*100.0).round())).show_ui(ui,|ui| {
                for scale in [0.5_f32,0.75,1.0,1.25,1.5,2.0] {
                    let supported=self.layout_for(scale).is_ok();
                    ui.add_enabled_ui(supported,|ui| {ui.selectable_value(&mut self.resolution_scale,scale,format!("{}%",(scale*100.0).round()));});
                }
            });
            ui.label("Lower resolutions make desktop items larger; higher resolutions show more workspace. This changes the virtual session, not the Windows monitors.");
            ui.label("Use the Linux desktop's appearance and desktop preferences to change icon and font sizes. AetherDesk does not overwrite those preferences.");
            if before!=self.resolution_scale {self.pending_layout=None;self.resize_due=None;ctx.request_repaint();}
        });
        self.display_settings = open;
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.monitor_tick.elapsed() > Duration::from_millis(500) {
            self.monitor_tick = Instant::now();
            self.monitor_snapshot = crate::remote_geometry::monitors();
            self.monitor_properties = self
                .monitor_snapshot
                .iter()
                .map(crate::remote_geometry::monitor_properties)
                .collect();
        }
        if self.mode == Mode::Tab {
            self.surface(ui);
            self.display_preferences(ui.ctx());
            self.resolution_preferences(ui.ctx());
            return;
        }
        let ctx = ui.ctx().clone();
        let mut builder = egui::ViewportBuilder::default()
            .with_icon(crate::theme::native_icon())
            .with_title(&self.window_title)
            .with_decorations(false);
        if let Some(layout) = self.fullscreen_layout() {
            if self.window.is_none() {
                let dpi = crate::remote_geometry::select(
                    &self.monitor_snapshot,
                    self.selected_monitor.as_deref(),
                )
                .map_or(96, |m| m.dpi) as f32
                    / 96.0;
                builder = builder
                    .with_position(egui::pos2(
                        layout.bounds.x as f32 / dpi,
                        layout.bounds.y as f32 / dpi,
                    ))
                    .with_inner_size(egui::vec2(
                        layout.bounds.width as f32 / dpi,
                        layout.bounds.height as f32 / dpi,
                    ));
            }
        } else if self.mode == Mode::Current {
            builder = builder.with_fullscreen(true);
        } else {
            self.error = Some("Cannot enumerate a supported monitor arrangement".into());
            self.set_mode(Mode::Tab);
            return;
        }
        ctx.show_viewport_immediate(self.id, builder, |ctx, _| {
            if ctx.input(|i| i.viewport().close_requested()) {
                self.set_mode(Mode::Tab);
                return;
            }
            if self.window.is_none() {
                self.window = crate::remote_geometry::window(&self.window_title);
            }
            if let (Some(hwnd), Some(layout)) = (self.window, self.fullscreen_layout()) {
                if let Err(error) =
                    crate::remote_geometry::place(hwnd, layout.bounds, &self.window_title)
                {
                    self.error = Some(error);
                    self.window = None;
                }
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| self.surface(ui));
            self.display_preferences(ctx);
            self.resolution_preferences(ctx);
            if self.mode != Mode::Tab {
                self.toolbar(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
    pub(super) fn secure_keys(&mut self) {
        self.release();
        for (scan_code, pressed) in [
            (29, true),
            (56, true),
            (111, true),
            (111, false),
            (56, false),
            (29, false),
        ] {
            self.input(InputMessage::Keyboard {
                scan_code,
                pressed,
                modifiers: lock_state(),
                repeat: false,
            });
        }
    }
    fn toolbar_scope(
        &self,
        ctx: &egui::Context,
        pointer: Option<egui::Pos2>,
    ) -> (egui::Rect, Option<String>) {
        let content = ctx.content_rect();
        if self.mode == Mode::Current {
            return (
                content,
                crate::remote_geometry::select(
                    &self.monitor_snapshot,
                    self.selected_monitor.as_deref(),
                )
                .map(|m| m.device),
            );
        }
        let Some(layout) = self.fullscreen_layout() else {
            return (content, None);
        };
        let monitors = self
            .monitor_snapshot
            .iter()
            .map(|m| {
                let rect = egui::Rect::from_min_size(
                    content.min
                        + egui::vec2(
                            (i64::from(m.rect.x) - i64::from(layout.bounds.x)) as f32
                                / layout.bounds.width as f32
                                * content.width(),
                            (i64::from(m.rect.y) - i64::from(layout.bounds.y)) as f32
                                / layout.bounds.height as f32
                                * content.height(),
                        ),
                    egui::vec2(
                        m.rect.width as f32 / layout.bounds.width as f32 * content.width(),
                        m.rect.height as f32 / layout.bounds.height as f32 * content.height(),
                    ),
                );
                (rect, Some(m.device.clone()))
            })
            .collect::<Vec<_>>();
        if let Some(pointer) = pointer {
            if let Some(m) = monitors.iter().find(|(rect, _)| rect.contains(pointer)) {
                return m.clone();
            }
        }
        monitors
            .iter()
            .find(|(_, device)| device == &self.placement.monitor)
            .or(monitors.first())
            .cloned()
            .unwrap_or((content, None))
    }
    pub(super) fn pointer_is_toolbar(&self, ctx: &egui::Context, pointer: egui::Pos2) -> bool {
        if self.mode == Mode::Tab {
            return false;
        }
        self.toolbar_drag.is_some()
            || self
                .toolbar_capture_rect(ctx)
                .is_some_and(|rect| rect.contains(pointer))
    }
    pub(super) fn toolbar_capture_rect(&self, ctx: &egui::Context) -> Option<egui::Rect> {
        if self.mode == Mode::Tab {
            return None;
        }
        let (scope, device) = self.toolbar_scope(ctx, None);
        let scale = self.toolbar_scale_for(ctx, device.as_deref());
        let size = self.toolbar_dimensions(scale);
        let predicted = self.placement.rect(scope, size).expand(6.0 * scale);
        Some(
            self.toolbar_rect
                .filter(|r| r.intersects(predicted))
                .map_or(predicted, |previous| previous.union(predicted)),
        )
    }
    fn toolbar_dimensions(&self, scale: f32) -> egui::Vec2 {
        crate::remote_toolbar::dimensions(self.placement.vertical(), scale)
    }

    fn toolbar_scale_for(&self, ctx: &egui::Context, device: Option<&str>) -> f32 {
        let monitor = crate::remote_geometry::select(&self.monitor_snapshot, device);
        (monitor.map_or(96, |m| m.dpi) as f32 / 96.0 / ctx.native_pixels_per_point().unwrap_or(1.0))
            .clamp(0.25, 4.0)
    }
    fn toolbar(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let (scope, device) = self.toolbar_scope(
            ctx,
            if self.toolbar_drag.is_some() {
                pointer
            } else {
                None
            },
        );
        let scale = self.toolbar_scale_for(ctx, device.as_deref());
        let size = self.toolbar_dimensions(scale);
        let predicted = self.placement.rect(scope, size);
        if self.toolbar_drag.is_some()
            || pointer.is_some_and(|p| {
                predicted.expand(6.0).contains(p) || self.placement.near_edge(p, scope)
            })
        {
            self.toolbar_until = Instant::now() + Duration::from_secs(2);
        }
        if Instant::now() > self.toolbar_until && self.toolbar_drag.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        let vertical = self.placement.vertical();
        let mut save = false;
        let response = crate::remote_toolbar::bar(
            ctx,
            egui::Id::new(("desktop-toolbar", self.id)),
            scope,
            predicted,
            vertical,
            scale,
            |ui| {
                let (r, handle) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0) * scale, egui::Sense::drag());
                for offset in [-5.0, 0.0, 5.0] {
                    ui.painter().line_segment(
                        [
                            r.center() + egui::vec2(-7.0, offset) * scale,
                            r.center() + egui::vec2(7.0, offset) * scale,
                        ],
                        egui::Stroke::new(1.5_f32 * scale, ui.visuals().text_color()),
                    );
                }
                if handle.drag_started() {
                    self.release();
                    if let Some(p) = handle.interact_pointer_pos() {
                        let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(p);
                        self.toolbar_drag = Some(origin - predicted.min);
                    }
                }
                if handle.dragged() {
                    if let (Some(p), Some(grab)) =
                        (handle.interact_pointer_pos(), self.toolbar_drag)
                    {
                        self.placement
                            .drag_scaled(p, grab, scope, size, 64.0 * scale);
                        self.placement.monitor = device.clone();
                        self.toolbar_until = Instant::now() + Duration::from_secs(2);
                    }
                }
                if handle.drag_stopped() {
                    self.toolbar_drag = None;
                    save = true;
                }
                handle.on_hover_text("Drag freely or dock within a screen edge; position is saved");
                use crate::ui_icons::{button_scaled, Icon};
                for (icon, label, mode) in [
                    (Icon::Tab, "Return to tab", Mode::Tab),
                    (Icon::Monitor, "Use this monitor", Mode::Current),
                    (Icon::Monitors, "Use all monitors", Mode::All),
                ] {
                    if button_scaled(ui, icon, label, self.mode == mode, scale).clicked() {
                        self.set_mode(mode);
                    }
                }
                if button_scaled(ui, Icon::SecureKeys, "Send Ctrl+Alt+Delete", false, scale)
                    .clicked()
                {
                    self.secure_keys();
                }
                if button_scaled(
                    ui,
                    Icon::Resolution,
                    "Change remote resolution",
                    self.resolution_settings,
                    scale,
                )
                .clicked()
                {
                    self.toggle_resolution();
                }
                if button_scaled(
                    ui,
                    Icon::Disconnect,
                    "Disconnect (keep remote desktop session)",
                    false,
                    scale,
                )
                .clicked()
                {
                    self.session_action(SessionAction::Disconnect);
                }
                if button_scaled(ui, Icon::EndSession, "End remote session", false, scale).clicked()
                {
                    self.session_action(SessionAction::End);
                }
            },
        );
        self.toolbar_rect = if vertical == self.placement.vertical() {
            Some(response.response.rect)
        } else {
            ctx.request_repaint();
            None
        };
        if save {
            if self.placement_valid {
                if let Err(error) = crate::remote_toolbar::save(&self.placement) {
                    self.error = Some(error);
                }
            } else {
                self.error=Some("Toolbar settings are damaged; saved file preserved. Current position works for this session.".into());
            }
        }
    }
}
