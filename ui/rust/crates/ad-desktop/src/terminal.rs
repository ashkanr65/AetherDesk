//! Native terminal screen: byte-stream parsing is independent of UI repaint.
use eframe::egui::{self, Color32, Event, Key, Modifiers};
#[path = "terminal_selection.rs"]
mod selection;

#[derive(Default)]
pub struct Replies(Vec<u8>, Option<String>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if self.0.len() > 4096 || i1.is_some() || i2.is_some() {
            return;
        }
        match (c, params) {
            ('n', [param]) if *param == [5] => self.0.extend_from_slice(b"\x1b[0n"),
            ('n', [param]) if *param == [6] => {
                let (row, col) = screen.cursor_position();
                self.0
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            ('c', _) => self.0.extend_from_slice(b"\x1b[?1;2c"),
            _ => {}
        }
    }
    // OSC 52 clipboard access remains disabled; host output is not permission
    // to read/write the Windows clipboard or run a local command.
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if params.len() == 2 && params[0] == b"7" {
            if let Some(path) = osc_directory(params[1]) {
                self.1 = Some(path);
            }
        }
    }
}

pub struct Terminal {
    parser: vt100::Parser<Replies>,
    selection: Option<(selection::Cell, selection::Cell)>,
    selection_drag: bool,
    selection_pointer: Option<egui::Pos2>,
    selection_scroll: f32,
    selection_tick: Option<f64>,
    settings: crate::terminal_settings::Settings,
    settings_version: u64,
    highlighter: crate::terminal_settings::Highlighter,
    settings_draft: Option<crate::terminal_settings::Settings>,
    settings_open: bool,
    settings_error: Option<String>,
    ime_preedit: String,
    mouse_button: Option<u8>,
    mouse_cell: Option<(u16, u16)>,
    history_limit: usize,
    scrollback_available: usize,
    scroll_grab: Option<f32>,
}
pub struct TerminalAction {
    pub input: Vec<u8>,
}
impl Default for Terminal {
    fn default() -> Self {
        let (_, settings) = crate::terminal_settings::current();
        Self {
            parser: vt100::Parser::new_with_callbacks(
                24,
                80,
                settings.scrollback_lines,
                Replies::default(),
            ),
            selection: None,
            selection_drag: false,
            selection_pointer: None,
            selection_scroll: 0.0,
            selection_tick: None,
            history_limit: settings.scrollback_lines,
            scrollback_available: 0,
            scroll_grab: None,
            highlighter: crate::terminal_settings::Highlighter::new(&settings),
            settings_draft: None,
            settings,
            settings_version: 0,
            settings_open: false,
            settings_error: None,
            ime_preedit: String::new(),
            mouse_button: None,
            mouse_cell: None,
        }
    }
}
impl Terminal {
    pub fn visible_text(&self) -> String {
        self.parser.screen().contents()
    }
    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (cols, rows)
    }
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let old_history = self.scrollback_available;
        let old_offset = self.parser.screen().scrollback();
        let alternate = self.parser.screen().alternate_screen();
        let tracking = self.selection.is_some() && !alternate;
        if tracking && old_history > 0 {
            // A one-row history probe counts newly scrolled rows even after the
            // retained history reaches capacity. It never changes host state.
            self.parser.screen_mut().set_scrollback(1);
        }
        self.parser.process(bytes);
        let probe = self.parser.screen().scrollback();
        self.refresh_history();
        if alternate != self.parser.screen().alternate_screen() {
            self.clear_selection();
        } else if tracking {
            let shift = if old_history == 0 {
                self.scrollback_available
            } else {
                probe.saturating_sub(1)
            };
            self.parser.screen_mut().set_scrollback(if old_offset > 0 {
                old_offset.saturating_add(shift)
            } else {
                0
            });
            if self.scrollback_available < old_history
                || (old_history > 0 && probe == 0)
                || (old_history == 0 && self.scrollback_available >= self.history_limit)
                || (old_history > 1
                    && self.scrollback_available == old_history
                    && probe == old_history)
            {
                // History was erased, or an entire retained window was evicted.
                self.clear_selection();
            } else if let Some((a, b)) = self.selection.as_mut() {
                a.0 -= shift as i64;
                b.0 -= shift as i64;
                let oldest = -(self.scrollback_available as i64);
                if a.0 < oldest && b.0 < oldest {
                    self.clear_selection();
                } else {
                    for point in [a, b] {
                        if point.0 < oldest {
                            *point = (oldest, 0);
                        }
                    }
                }
            }
        }
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }
    fn refresh_history(&mut self) {
        // vt100 clamps the offset to the real retained history, not its capacity.
        // Probe and restore without consuming output or changing the live screen.
        let old = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        self.scrollback_available = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(old);
    }
    pub fn take_directory(&mut self) -> Option<String> {
        // The session's file-window switch gates both directions. Always drain
        // OSC updates here so disabled synchronization cannot replay an old path.
        self.parser.callbacks_mut().1.take()
    }
    pub fn release_mouse(&mut self) -> Vec<u8> {
        self.selection_drag = false;
        self.selection_pointer = None;
        self.selection_scroll = 0.0;
        self.selection_tick = None;
        self.ime_preedit.clear();
        let Some(button) = self.mouse_button.take() else {
            return vec![];
        };
        if matches!(
            self.parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::None | vt100::MouseProtocolMode::Press
        ) {
            return vec![];
        }
        mouse_bytes(
            button,
            self.mouse_cell.unwrap_or((0, 0)),
            true,
            false,
            Modifiers::NONE,
            self.parser.screen().mouse_protocol_encoding(),
        )
    }
    fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_drag = false;
        self.selection_pointer = None;
        self.selection_scroll = 0.0;
        self.selection_tick = None;
    }
    pub fn selected_text(&mut self) -> Option<String> {
        self.selection.map(|range| {
            selection::copy(self.parser.screen_mut(), range, self.scrollback_available)
        })
    }
    pub fn settings_button(&mut self, ui: &mut egui::Ui) {
        if crate::ui_icons::button(
            ui,
            crate::ui_icons::Icon::TerminalSettings,
            "Terminal settings: fonts and colours; Shift + drag selects text",
            self.settings_open,
        )
        .clicked()
        {
            self.settings_open = true;
            self.settings_draft = Some(self.settings.clone());
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) -> TerminalAction {
        let (version, settings) = crate::terminal_settings::current();
        if version != self.settings_version {
            if settings.scrollback_lines != self.history_limit {
                let state = self.parser.screen().state_formatted();
                let (rows, cols) = self.parser.screen().size();
                self.parser = vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    settings.scrollback_lines,
                    Replies::default(),
                );
                self.parser.process(&state);
                self.clear_selection();
                self.history_limit = settings.scrollback_lines;
                self.refresh_history();
            }
            self.highlighter = crate::terminal_settings::Highlighter::new(&settings);
            self.settings = settings;
            self.settings_version = version;
            crate::terminal_settings::install_font(ui.ctx(), &self.settings);
        }
        if self.settings_open {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            let draft = self
                .settings_draft
                .get_or_insert_with(|| self.settings.clone());
            let size = crate::app_layout::dialog_size(ui.ctx(), egui::vec2(650.0, 580.0));
            egui::Window::new("Terminal settings")
                .id(ui.id().with("terminal-settings"))
                .open(&mut open)
                .fixed_size(size)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    let enter = crate::dialog::primary(ui);
                    egui::ScrollArea::both()
                        .id_salt("terminal-settings-content")
                        .max_height((size.y - 70.0).max(40.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(
                                (ui.text_style_height(&egui::TextStyle::Body) * 42.0).max(650.0),
                            );
                            crate::terminal_settings::edit(ui, draft);
                        });
                    if let Some(error) = &self.settings_error {
                        ui.label(error);
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                        |ui| {
                            cancel = ui.button("Cancel").clicked();
                            let response = ui.button("Apply and save");
                            crate::dialog::focus_on_open(ui, &response);
                            apply = response.clicked() || enter;
                        },
                    );
                });
            if apply {
                match crate::terminal_settings::save(draft.clone()) {
                    Ok(()) => {
                        open = false;
                        self.settings_error = None;
                    }
                    Err(error) => self.settings_error = Some(error),
                }
            }
            if !open || cancel {
                self.settings_open = false;
                self.settings_draft = None;
            }
        }
        let font = egui::FontId::monospace(self.settings.font_size);
        let cell_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M')).max(1.0);
        let cell_height = (self.settings.font_size * self.settings.line_height).ceil();
        let available = ui.available_size();
        let cols = ((available.x - 18.0).max(0.0) / cell_width)
            .floor()
            .clamp(2.0, 500.0) as u16;
        let rows = (available.y / cell_height).floor().clamp(2.0, 250.0) as u16;
        let mut action = TerminalAction { input: Vec::new() };
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.refresh_history();
            self.clear_selection();
        }
        let (surface, _) = ui.allocate_exact_size(
            egui::vec2(available.x, available.y.max(2.0)),
            egui::Sense::hover(),
        );
        let gutter = egui::Rect::from_min_max(
            egui::pos2((surface.right() - 18.0).max(surface.left()), surface.top()),
            surface.right_bottom(),
        );
        let rect =
            egui::Rect::from_min_max(surface.min, egui::pos2(gutter.left(), surface.bottom()));
        let response = ui.interact(
            rect,
            ui.id().with("terminal-surface"),
            egui::Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let cell_at = |pos: egui::Pos2| {
            (
                ((pos.y - rect.top()) / cell_height)
                    .floor()
                    .clamp(0.0, f32::from(rows - 1)) as u16,
                ((pos.x - rect.left()) / cell_width)
                    .floor()
                    .clamp(0.0, f32::from(cols - 1)) as u16,
            )
        };
        let mouse_mode = self.parser.screen().mouse_protocol_mode();
        let mouse_encoding = self.parser.screen().mouse_protocol_encoding();
        let local_selection_gesture = self.selection_drag;
        if !ui.input(|i| i.focused && i.pointer.primary_down()) {
            if self.selection_drag && ui.input(|i| i.focused) {
                if let (Some((_, end)), Some(pos)) = (
                    self.selection.as_mut(),
                    ui.input(|i| i.pointer.interact_pos()),
                ) {
                    *end = selection::cell(cell_at(pos), self.parser.screen().scrollback());
                }
            }
            self.selection_drag = false;
            self.selection_pointer = None;
            self.selection_scroll = 0.0;
            self.selection_tick = None;
        }
        let report_mouse = enabled
            && mouse_mode != vt100::MouseProtocolMode::None
            && !ui.input(|i| i.modifiers.shift)
            && !local_selection_gesture;
        if self.mouse_button.is_some()
            && (!report_mouse
                || !ui.input(|i| i.focused)
                || ui.input(|i| i.events.iter().any(|e| matches!(e, Event::PointerGone))))
        {
            action.input.extend(self.release_mouse());
        }
        if report_mouse {
            for event in ui.input(|i| i.events.clone()) {
                match event {
                    Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    } if rect.contains(pos) || (!pressed && self.mouse_button.is_some()) => {
                        let button = match button {
                            egui::PointerButton::Primary => 0,
                            egui::PointerButton::Middle => 1,
                            egui::PointerButton::Secondary => 2,
                            _ => continue,
                        };
                        if pressed {
                            self.mouse_button = Some(button);
                            response.request_focus();
                        }
                        if pressed || mouse_mode != vt100::MouseProtocolMode::Press {
                            action.input.extend(mouse_bytes(
                                button,
                                cell_at(pos),
                                !pressed,
                                false,
                                modifiers,
                                mouse_encoding,
                            ));
                        }
                        if !pressed {
                            self.mouse_button = None;
                        }
                        self.mouse_cell = Some(cell_at(pos));
                    }
                    Event::PointerMoved(pos)
                        if rect.contains(pos)
                            && (mouse_mode == vt100::MouseProtocolMode::AnyMotion
                                || (mouse_mode == vt100::MouseProtocolMode::ButtonMotion
                                    && self.mouse_button.is_some())) =>
                    {
                        let cell = cell_at(pos);
                        if self.mouse_cell != Some(cell) {
                            action.input.extend(mouse_bytes(
                                self.mouse_button.unwrap_or(3),
                                cell,
                                false,
                                true,
                                ui.input(|i| i.modifiers),
                                mouse_encoding,
                            ));
                            self.mouse_cell = Some(cell);
                        }
                    }
                    _ => {}
                }
            }
        }
        if !self.selection_drag
            && !report_mouse
            && (response.drag_started_by(egui::PointerButton::Primary)
                || (response.is_pointer_button_down_on()
                    && ui.input(|i| i.pointer.primary_pressed())))
        {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                let cell = selection::cell(cell_at(pos), self.parser.screen().scrollback());
                self.selection = Some((cell, cell));
                self.selection_drag = true;
                self.selection_scroll = 0.0;
                self.selection_tick = Some(ui.input(|i| i.time));
                response.request_focus();
            }
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && (report_mouse || self.selection.is_none_or(|(a, b)| a == b))
        {
            self.clear_selection();
        }
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if response.hovered() || self.selection_drag {
            if scroll.abs() >= 1.0 {
                if report_mouse {
                    let cell = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(cell_at)
                        .unwrap_or((0, 0));
                    for _ in 0..3 {
                        action.input.extend(mouse_bytes(
                            if scroll > 0.0 { 64 } else { 65 },
                            cell,
                            false,
                            false,
                            ui.input(|i| i.modifiers),
                            mouse_encoding,
                        ));
                    }
                } else if enabled
                    && self.parser.screen().alternate_screen()
                    && !ui.input(|i| i.modifiers.shift)
                    && !self.selection_drag
                {
                    for _ in 0..3 {
                        action.input.extend(
                            key_bytes(
                                if scroll > 0.0 {
                                    Key::ArrowUp
                                } else {
                                    Key::ArrowDown
                                },
                                Modifiers::NONE,
                                self.parser.screen().application_cursor(),
                            )
                            .unwrap(),
                        );
                    }
                } else {
                    let old = self.parser.screen().scrollback();
                    self.parser.screen_mut().set_scrollback(if scroll > 0.0 {
                        old.saturating_add(3)
                    } else {
                        old.saturating_sub(3)
                    });
                }
            }
        }
        if self.selection_drag {
            let now = ui.input(|i| i.time);
            let dt = self
                .selection_tick
                .replace(now)
                .map_or(0.0, |previous| (now - previous).max(0.0) as f32);
            if let Some(pos) = response.interact_pointer_pos() {
                self.selection_pointer = Some(pos);
            }
            if let Some(pos) = self.selection_pointer {
                let old = self.parser.screen().scrollback();
                let edge_drag =
                    response.dragged_by(egui::PointerButton::Primary) || !rect.contains(pos);
                let lines = if edge_drag
                    && scroll.abs() < 1.0
                    && !self.parser.screen().alternate_screen()
                {
                    selection::edge_lines(&mut self.selection_scroll, rect, pos, cell_height, dt)
                } else {
                    self.selection_scroll = 0.0;
                    0
                };
                let offset = if lines >= 0 {
                    old.saturating_add(lines as usize)
                } else {
                    old.saturating_sub((-lines) as usize)
                };
                self.parser.screen_mut().set_scrollback(offset);
                if edge_drag
                    && ((pos.y < rect.top() + cell_height && offset < self.scrollback_available)
                        || (pos.y > rect.bottom() - cell_height && offset > 0))
                {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(20));
                } else {
                    self.selection_scroll = 0.0;
                }
                if let Some((_, end)) = self.selection.as_mut() {
                    *end = selection::cell(cell_at(pos), self.parser.screen().scrollback());
                }
            }
        }
        if response.has_focus() && enabled {
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
            let modifiers = ui.input(|i| i.modifiers);
            let events = ui.input(|i| i.events.clone());
            let has_copy = events.iter().any(|e| matches!(e, Event::Copy));
            let has_cut = events.iter().any(|e| matches!(e, Event::Cut));
            let has_paste = events.iter().any(|e| matches!(e, Event::Paste(_)));
            let altgr_text = modifiers.ctrl
                && modifiers.alt
                && events.iter().any(|e| matches!(e, Event::Text(_)));
            let commits: Vec<String> = events
                .iter()
                .filter_map(|e| {
                    if let Event::Ime(egui::ImeEvent::Commit(text)) = e {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect();
            for event in events {
                match event {
                    Event::Text(text) if (!modifiers.ctrl && !modifiers.command) || altgr_text => {
                        // Alt-modified letters are encoded by Key below. AltGr
                        // printable text is handled by the platform Text event.
                        if (!modifiers.alt || altgr_text)
                            && !commits.contains(&text)
                            && self.ime_preedit.is_empty()
                        {
                            action.input.extend(text.as_bytes());
                        }
                    }
                    Event::Ime(egui::ImeEvent::Commit(text)) => {
                        self.ime_preedit.clear();
                        action.input.extend(text.as_bytes())
                    }
                    Event::Ime(egui::ImeEvent::Preedit(text)) => self.ime_preedit = text,
                    Event::Ime(egui::ImeEvent::Disabled) => self.ime_preedit.clear(),
                    Event::Paste(text) => action
                        .input
                        .extend(paste_bytes(&text, self.parser.screen().bracketed_paste())),
                    Event::Copy => {
                        if let Some(text) = self.selected_text().filter(|s| !s.is_empty()) {
                            ui.ctx().copy_text(text);
                        } else if !modifiers.shift {
                            action.input.push(3);
                        }
                    }
                    Event::Cut => action.input.push(24),
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if (key == Key::C && has_copy)
                            || (key == Key::X && has_cut)
                            || (key == Key::V && has_paste)
                        {
                            continue;
                        }
                        if altgr_text && key.name().len() == 1 {
                            continue;
                        }
                        if !self.ime_preedit.is_empty() {
                            continue;
                        }
                        if modifiers.shift
                            && !modifiers.ctrl
                            && matches!(key, Key::PageUp | Key::PageDown)
                        {
                            let old = self.parser.screen().scrollback();
                            self.parser
                                .screen_mut()
                                .set_scrollback(if key == Key::PageUp {
                                    old.saturating_add(usize::from(rows))
                                } else {
                                    old.saturating_sub(usize::from(rows))
                                });
                            self.clear_selection();
                        } else if modifiers.ctrl && modifiers.shift && key == Key::A {
                            self.selection = Some((
                                selection::cell((0, 0), self.parser.screen().scrollback()),
                                selection::cell(
                                    (rows - 1, cols - 1),
                                    self.parser.screen().scrollback(),
                                ),
                            ));
                        } else if modifiers.ctrl && modifiers.shift && key == Key::C {
                            if let Some(text) = self.selected_text() {
                                ui.ctx().copy_text(text);
                            }
                        } else if (modifiers.ctrl && modifiers.shift && key == Key::V)
                            || (modifiers.shift && key == Key::Insert)
                        {
                            if !has_paste {
                                if let Ok(text) = clipboard_text() {
                                    action.input.extend(paste_bytes(
                                        &text,
                                        self.parser.screen().bracketed_paste(),
                                    ));
                                }
                            }
                        } else if modifiers.ctrl && key == Key::Insert {
                            if let Some(text) = self.selected_text() {
                                ui.ctx().copy_text(text);
                            }
                        } else if let Some(bytes) =
                            key_bytes(key, modifiers, self.parser.screen().application_cursor())
                        {
                            action.input.extend(bytes);
                        }
                    }
                    _ => {}
                }
            }
            // Prevent Tab/arrows from navigating widgets after sending to PTY.
            ui.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        Event::Key { .. }
                            | Event::Text(_)
                            | Event::Paste(_)
                            | Event::Copy
                            | Event::Cut
                            | Event::Ime(_)
                    )
                })
            });
            if !action.input.is_empty() {
                self.parser.screen_mut().set_scrollback(0);
                self.clear_selection();
            }
        }
        if response.secondary_clicked() && !report_mouse {
            if let Some(text) = self.selected_text().filter(|s| !s.is_empty()) {
                ui.ctx().copy_text(text);
                self.clear_selection();
            } else if enabled {
                if let Ok(text) = clipboard_text() {
                    action
                        .input
                        .extend(paste_bytes(&text, self.parser.screen().bracketed_paste()));
                }
            }
        }
        let track = egui::Rect::from_center_size(
            gutter.center(),
            egui::vec2(
                (gutter.width() - 8.0).max(0.0),
                (gutter.height() - 4.0).max(0.0),
            ),
        );
        let history = if self.parser.screen().alternate_screen() {
            0
        } else {
            self.scrollback_available
        };
        let offset = self.parser.screen().scrollback();
        let thumb = scroll_thumb(track, usize::from(rows), history, offset);
        let rail = ui.interact(
            gutter,
            ui.id().with("terminal-history"),
            egui::Sense::click_and_drag(),
        );
        rail.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Slider, history > 0, "Terminal scrollback")
        });
        if history > 0 {
            if rail.drag_started() || rail.clicked() {
                response.request_focus();
                if let Some(pos) = rail.interact_pointer_pos() {
                    self.scroll_grab = Some(if thumb.contains(pos) {
                        pos.y - thumb.top()
                    } else {
                        thumb.height() / 2.0
                    });
                }
            }
            if rail.dragged() || rail.clicked() {
                if let Some(pos) = rail.interact_pointer_pos() {
                    let offset = scroll_offset(
                        track,
                        thumb.height(),
                        history,
                        pos.y - self.scroll_grab.unwrap_or(thumb.height() / 2.0),
                    );
                    self.parser.screen_mut().set_scrollback(offset);
                    self.clear_selection();
                }
            }
            if rail.hovered() && !self.selection_drag {
                let wheel = ui.input(|i| i.raw_scroll_delta.y);
                if wheel.abs() >= 1.0 {
                    let old = self.parser.screen().scrollback();
                    self.parser.screen_mut().set_scrollback(if wheel > 0.0 {
                        old.saturating_add(3)
                    } else {
                        old.saturating_sub(3)
                    });
                    self.clear_selection();
                }
            }
        }
        if rail.drag_stopped() {
            self.scroll_grab = None;
        }
        ui.painter()
            .rect_filled(gutter, 0, self.settings.color("background"));
        ui.painter()
            .rect_filled(track, 4, ui.visuals().extreme_bg_color);
        let thumb = scroll_thumb(
            track,
            usize::from(rows),
            history,
            self.parser.screen().scrollback(),
        );
        ui.painter().rect_filled(
            thumb,
            4,
            if history == 0 {
                ui.visuals().widgets.noninteractive.bg_fill
            } else {
                ui.style().interact(&rail).fg_stroke.color
            },
        );
        rail.on_hover_text("Terminal history · drag to scroll · bottom returns to live output");
        let painter = ui.painter().with_clip_rect(rect);
        let default_background = self.settings.color("background");
        painter.rect_filled(rect, 0.0, default_background);
        let screen = self.parser.screen();
        for row in 0..rows {
            let word_colours =
                row_word_colours(screen, row, cols, &self.settings, &self.highlighter);
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let point = rect.min
                    + egui::vec2(f32::from(col) * cell_width, f32::from(row) * cell_height);
                let width = if cell.is_wide() {
                    cell_width * 2.0
                } else {
                    cell_width
                };
                let cell_rect = egui::Rect::from_min_size(point, egui::vec2(width, cell_height));
                let mut foreground = self.screen_color(
                    cell.fgcolor(),
                    word_colours
                        .get(usize::from(col))
                        .copied()
                        .flatten()
                        .unwrap_or_else(|| self.settings.color("foreground")),
                );
                let mut background =
                    self.screen_color(cell.bgcolor(), self.settings.color("background"));
                if cell.inverse() {
                    std::mem::swap(&mut foreground, &mut background);
                }
                if let Some((a, b)) = self.selection {
                    let (a, b) = if a <= b { (a, b) } else { (b, a) };
                    let point = selection::cell((row, col), screen.scrollback());
                    if a <= point && point <= b {
                        background = self.settings.color("selection");
                    }
                }
                if cell.is_wide_continuation() {
                    continue;
                }
                // The surface already paints the default background. Empty
                // cells need neither another rectangle nor a font layout.
                if background != default_background {
                    painter.rect_filled(cell_rect, 0.0, background);
                }
                if cell.dim() {
                    foreground = foreground.gamma_multiply(0.65);
                }
                let text = cell.contents();
                if !text.is_empty() && text != " " {
                    painter.text(
                        point,
                        egui::Align2::LEFT_TOP,
                        text,
                        font.clone(),
                        foreground,
                    );
                    if cell.bold() {
                        painter.text(
                            point + egui::vec2(0.5, 0.0),
                            egui::Align2::LEFT_TOP,
                            text,
                            font.clone(),
                            foreground,
                        );
                    }
                }
                if cell.underline() {
                    painter.line_segment(
                        [cell_rect.left_bottom(), cell_rect.right_bottom()],
                        egui::Stroke::new(1.0_f32, foreground),
                    );
                }
            }
        }
        if response.has_focus() && !screen.hide_cursor() && screen.scrollback() == 0 {
            let (row, col) = screen.cursor_position();
            let point =
                rect.min + egui::vec2(f32::from(col) * cell_width, f32::from(row) * cell_height);
            painter.rect_stroke(
                egui::Rect::from_min_size(point, egui::vec2(cell_width, cell_height)),
                0.0,
                egui::Stroke::new(1.0_f32, self.settings.color("cursor")),
                egui::StrokeKind::Inside,
            );
            let cursor_rect = egui::Rect::from_min_size(point, egui::vec2(cell_width, cell_height));
            ui.output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect, cursor_rect }));
            if !self.ime_preedit.is_empty() {
                painter.text(
                    point,
                    egui::Align2::LEFT_TOP,
                    &self.ime_preedit,
                    font,
                    self.settings.color("foreground"),
                );
            }
        }
        action
    }
    fn screen_color(&self, color_value: vt100::Color, default: Color32) -> Color32 {
        if let vt100::Color::Idx(index) = color_value {
            if index < 16 {
                return self.settings.ansi(index);
            }
        }
        color(color_value, default)
    }
}
fn osc_directory(uri: &[u8]) -> Option<String> {
    if uri.len() > 4096 {
        return None;
    }
    let uri = std::str::from_utf8(uri).ok()?.strip_prefix("file://")?;
    let (_, path) = uri.split_once('/')?;
    if path.contains(['?', '#']) {
        return None;
    }
    let mut bytes = vec![b'/'];
    let input = path.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hex = std::str::from_utf8(input.get(i + 1..i + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(input[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if path.chars().any(char::is_control) {
        None
    } else {
        Some(path)
    }
}
fn mouse_bytes(
    button: u8,
    cell: (u16, u16),
    released: bool,
    motion: bool,
    m: Modifiers,
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let code = u16::from(button)
        + if motion { 32 } else { 0 }
        + if m.shift { 4 } else { 0 }
        + if m.alt { 8 } else { 0 }
        + if m.ctrl { 16 } else { 0 };
    let (y, x) = (cell.0 + 1, cell.1 + 1);
    if encoding == vt100::MouseProtocolEncoding::Sgr {
        return format!("\x1b[<{code};{x};{y}{}", if released { 'm' } else { 'M' }).into_bytes();
    }
    let code = if released { 3 + (code & 28) } else { code };
    let mut result = b"\x1b[M".to_vec();
    if encoding == vt100::MouseProtocolEncoding::Utf8 {
        for value in [code + 32, x + 32, y + 32] {
            if let Some(c) = char::from_u32(u32::from(value)) {
                let mut b = [0; 4];
                result.extend(c.encode_utf8(&mut b).as_bytes());
            }
        }
    } else {
        if x > 223 || y > 223 {
            return Vec::new();
        }
        result.extend([code as u8 + 32, x as u8 + 32, y as u8 + 32]);
    }
    result
}
fn clipboard_text() -> Result<String, arboard::Error> {
    arboard::Clipboard::new()?.get_text()
}
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Paste is text, not a source of escape sequences or bracket delimiters.
    let text = text.replace('\x1b', "").replace('\0', "");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
    }
}
fn key_bytes(key: Key, m: Modifiers, application: bool) -> Option<Vec<u8>> {
    if m.ctrl {
        let value = match key {
            Key::A => 1,
            Key::B => 2,
            Key::C => 3,
            Key::D => 4,
            Key::E => 5,
            Key::F => 6,
            Key::G => 7,
            Key::H => 8,
            Key::I => 9,
            Key::J => 10,
            Key::K => 11,
            Key::L => 12,
            Key::M => 13,
            Key::N => 14,
            Key::O => 15,
            Key::P => 16,
            Key::Q => 17,
            Key::R => 18,
            Key::S => 19,
            Key::T => 20,
            Key::U => 21,
            Key::V => 22,
            Key::W => 23,
            Key::X => 24,
            Key::Y => 25,
            Key::Z => 26,
            Key::Space | Key::Num2 => 0,
            Key::OpenBracket => 27,
            Key::Backslash => 28,
            Key::CloseBracket => 29,
            Key::Num6 => 30,
            Key::Minus => 31,
            _ => 255,
        };
        if value != 255 {
            let mut bytes = vec![value];
            if m.alt {
                bytes.insert(0, 27);
            }
            return Some(bytes);
        }
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let cursor = match key {
        Key::ArrowUp => Some('A'),
        Key::ArrowDown => Some('B'),
        Key::ArrowRight => Some('C'),
        Key::ArrowLeft => Some('D'),
        Key::Home => Some('H'),
        Key::End => Some('F'),
        _ => None,
    };
    if let Some(c) = cursor {
        return Some(
            if modifier != 1 {
                format!("\x1b[1;{modifier}{c}")
            } else if application {
                format!("\x1bO{c}")
            } else {
                format!("\x1b[{c}")
            }
            .into_bytes(),
        );
    }
    let tilde = match key {
        Key::Insert => Some(2),
        Key::Delete => Some(3),
        Key::PageUp => Some(5),
        Key::PageDown => Some(6),
        Key::F5 => Some(15),
        Key::F6 => Some(17),
        Key::F7 => Some(18),
        Key::F8 => Some(19),
        Key::F9 => Some(20),
        Key::F10 => Some(21),
        Key::F11 => Some(23),
        Key::F12 => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return Some(
            if modifier == 1 {
                format!("\x1b[{n}~")
            } else {
                format!("\x1b[{n};{modifier}~")
            }
            .into_bytes(),
        );
    }
    let mut bytes = match key {
        Key::Enter => vec![13],
        Key::Backspace => vec![127],
        Key::Tab => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![9]
            }
        }
        Key::Escape => vec![27],
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        _ if m.alt && !m.ctrl => {
            let name = key.name();
            if name.len() != 1 {
                return None;
            }
            if m.shift {
                name.to_uppercase().into_bytes()
            } else {
                name.to_lowercase().into_bytes()
            }
        }
        _ => return None,
    };
    if m.alt {
        bytes.insert(0, 27);
    }
    Some(bytes)
}
fn color(value: vt100::Color, default: Color32) -> Color32 {
    match value {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(index) => {
            const ANSI: [u32; 16] = [
                0x171717, 0xe24848, 0x33c266, 0xd9a72e, 0x4b8fe2, 0xc678dd, 0x46b8c7, 0xd8d8d8,
                0x73777e, 0xff5f5f, 0x55e182, 0xffc857, 0x76aeed, 0xd99ae8, 0x72d5df, 0xffffff,
            ];
            if index < 16 {
                let c = ANSI[usize::from(index)];
                Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
            } else if index < 232 {
                let n = index - 16;
                let part = |n| if n == 0 { 0 } else { 55 + 40 * n };
                Color32::from_rgb(part(n / 36), part(n / 6 % 6), part(n % 6))
            } else {
                Color32::from_gray(8 + (index - 232) * 10)
            }
        }
    }
}
/// Byte ranges are mapped through real terminal cells, including wide and
/// combining characters. Explicit host ANSI styling always takes precedence.
fn row_word_colours(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    settings: &crate::terminal_settings::Settings,
    highlighter: &crate::terminal_settings::Highlighter,
) -> Vec<Option<Color32>> {
    if !settings.auto_classify || screen.alternate_screen() {
        return Vec::new();
    }
    let mut line = String::new();
    let mut offsets = vec![None; usize::from(cols)];
    for col in 0..cols {
        if let Some(cell) = screen.cell(row, col) {
            if cell.is_wide_continuation() {
                continue;
            }
            offsets[usize::from(col)] = Some(line.len());
            if cell.contents().is_empty() {
                line.push(' ');
            } else {
                line.push_str(cell.contents());
            }
        } else {
            line.push(' ');
        }
    }
    if line.trim().is_empty() {
        return Vec::new();
    }
    let spans = highlighter.spans(&line, settings);
    offsets
        .into_iter()
        .enumerate()
        .map(|(col, offset)| {
            let cell = screen.cell(row, col as u16)?;
            if cell.fgcolor() != vt100::Color::Default
                || cell.bgcolor() != vt100::Color::Default
                || cell.inverse()
                || cell.bold()
                || cell.underline()
                || cell.dim()
            {
                return None;
            }
            let offset = offset?;
            spans
                .iter()
                .find(|(range, _)| range.contains(&offset))
                .map(|(_, color)| *color)
        })
        .collect()
}
fn scroll_thumb(track: egui::Rect, rows: usize, history: usize, offset: usize) -> egui::Rect {
    let height = (track.height() * rows as f32 / (rows + history).max(1) as f32)
        .max(18.0)
        .min(track.height());
    let fraction = if history == 0 {
        0.0
    } else {
        offset.min(history) as f32 / history as f32
    };
    egui::Rect::from_min_size(
        egui::pos2(
            track.left(),
            track.top() + (track.height() - height) * (1.0 - fraction),
        ),
        egui::vec2(track.width(), height),
    )
}
fn scroll_offset(track: egui::Rect, thumb_height: f32, history: usize, top: f32) -> usize {
    let travel = (track.height() - thumb_height).max(1.0);
    ((1.0 - (top - track.top()) / travel).clamp(0.0, 1.0) * history as f32).round() as usize
}
#[cfg(test)]
mod tests {
    use super::*;
    fn selection_frame(
        terminal: &mut Terminal,
        ctx: &egui::Context,
        time: f64,
        events: Vec<Event>,
        modifiers: Modifiers,
        focused: bool,
    ) -> egui::Rect {
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                time: Some(time),
                focused,
                modifiers,
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = ui.available_rect_before_wrap();
                    rect.max.x -= 18.0;
                    assert!(
                        terminal.ui(ui, true).input.is_empty(),
                        "local text selection sent input to the host"
                    );
                });
            },
        );
        rect
    }
    fn primary(pos: egui::Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }
    #[test]
    fn wheel_extends_held_selection_before_drag_threshold_and_keeps_it_on_release() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        let mut terminal = Terminal::default();
        let rect = selection_frame(&mut terminal, &ctx, 0.0, vec![], Modifiers::NONE, true);
        for _ in 0..100 {
            terminal.process(b"output\r\n");
        }
        let pos = rect.center();
        selection_frame(
            &mut terminal,
            &ctx,
            0.1,
            vec![
                Event::PointerMoved(pos),
                primary(pos, true, Modifiers::NONE),
            ],
            Modifiers::NONE,
            true,
        );
        let anchor = terminal.selection.unwrap().0;
        selection_frame(
            &mut terminal,
            &ctx,
            0.2,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 1.0),
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
            true,
        );
        assert_eq!(terminal.selection.unwrap().0, anchor);
        assert_ne!(terminal.selection.unwrap().0, terminal.selection.unwrap().1);
        selection_frame(
            &mut terminal,
            &ctx,
            0.3,
            vec![primary(pos, false, Modifiers::NONE)],
            Modifiers::NONE,
            true,
        );
        assert!(!terminal.selection_drag);
        assert!(terminal.selected_text().unwrap().contains("output"));
    }
    #[test]
    fn dragging_terminal_selection_scrolls_both_edges_and_wheel_without_losing_its_anchor() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        let mut terminal = Terminal::default();
        let rect = selection_frame(&mut terminal, &ctx, 0.0, vec![], Modifiers::NONE, true);
        for line in 0..200 {
            terminal.process(format!("line-{line:03}\r\n").as_bytes());
        }
        let height = (terminal.settings.font_size * terminal.settings.line_height).ceil();
        let start = rect.min + egui::vec2(24.0, height * 4.5);
        let inside = rect.min + egui::vec2(24.0, height * 2.5);
        selection_frame(
            &mut terminal,
            &ctx,
            0.1,
            vec![
                Event::PointerMoved(start),
                primary(start, true, Modifiers::NONE),
            ],
            Modifiers::NONE,
            true,
        );
        selection_frame(
            &mut terminal,
            &ctx,
            0.2,
            vec![Event::PointerMoved(inside)],
            Modifiers::NONE,
            true,
        );
        let anchor = terminal.selection.unwrap().0;
        assert_eq!(
            anchor.0, 4,
            "drag must start at the press origin, not the later drag threshold"
        );
        selection_frame(
            &mut terminal,
            &ctx,
            0.3,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 1.0),
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
            true,
        );
        assert!(terminal.parser.screen().scrollback() >= 3);
        assert_eq!(terminal.selection.unwrap().0, anchor);
        let above = egui::pos2(inside.x, rect.top() - height * 3.0);
        for tick in 0..20 {
            selection_frame(
                &mut terminal,
                &ctx,
                0.4 + tick as f64 * 0.05,
                vec![Event::PointerMoved(above)],
                Modifiers::NONE,
                true,
            );
        }
        let offset = terminal.parser.screen().scrollback();
        selection_frame(
            &mut terminal,
            &ctx,
            1.35,
            vec![Event::PointerMoved(above)],
            Modifiers::NONE,
            true,
        );
        assert_eq!(
            terminal.parser.screen().scrollback(),
            offset,
            "an extra layout pass advanced time-based scrolling twice"
        );
        assert!(offset > usize::from(terminal.parser.screen().size().0));
        assert!(
            terminal.selected_text().unwrap().lines().count()
                > usize::from(terminal.parser.screen().size().0)
        );
        let below = egui::pos2(inside.x, rect.bottom() + height * 3.0);
        for tick in 0..10 {
            selection_frame(
                &mut terminal,
                &ctx,
                1.4 + tick as f64 * 0.05,
                vec![Event::PointerMoved(below)],
                Modifiers::NONE,
                true,
            );
        }
        assert!(terminal.parser.screen().scrollback() < offset);
        assert_eq!(terminal.selection.unwrap().0, anchor);
        selection_frame(
            &mut terminal,
            &ctx,
            2.0,
            vec![primary(below, false, Modifiers::NONE)],
            Modifiers::NONE,
            true,
        );
        let stopped = terminal.parser.screen().scrollback();
        assert!(!terminal.selection_drag);
        for tick in 0..5 {
            selection_frame(
                &mut terminal,
                &ctx,
                2.1 + tick as f64 * 0.1,
                vec![],
                Modifiers::NONE,
                true,
            );
        }
        assert_eq!(
            terminal.parser.screen().scrollback(),
            stopped,
            "scrolling continued after release"
        );
        assert!(terminal.selection.is_some());
    }
    #[test]
    fn shift_selection_stays_local_after_shift_release_and_stops_on_focus_loss() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        let mut terminal = Terminal::default();
        let rect = selection_frame(&mut terminal, &ctx, 0.0, vec![], Modifiers::NONE, true);
        for _ in 0..100 {
            terminal.process(b"output\r\n");
        }
        terminal.process(b"\x1b[?1000h\x1b[?1006h");
        let start = rect.min + egui::vec2(30.0, 90.0);
        let end = rect.min + egui::vec2(30.0, 40.0);
        selection_frame(
            &mut terminal,
            &ctx,
            0.1,
            vec![
                Event::PointerMoved(start),
                primary(start, true, Modifiers::SHIFT),
            ],
            Modifiers::SHIFT,
            true,
        );
        selection_frame(
            &mut terminal,
            &ctx,
            0.2,
            vec![Event::PointerMoved(end)],
            Modifiers::SHIFT,
            true,
        );
        assert!(terminal.selection_drag);
        selection_frame(
            &mut terminal,
            &ctx,
            0.3,
            vec![Event::PointerMoved(end)],
            Modifiers::NONE,
            true,
        );
        assert!(terminal.selection_drag);
        selection_frame(
            &mut terminal,
            &ctx,
            0.4,
            vec![primary(end, false, Modifiers::NONE)],
            Modifiers::NONE,
            true,
        );
        assert!(!terminal.selection_drag);
        selection_frame(
            &mut terminal,
            &ctx,
            0.5,
            vec![
                Event::PointerMoved(start),
                primary(start, true, Modifiers::SHIFT),
            ],
            Modifiers::SHIFT,
            true,
        );
        selection_frame(
            &mut terminal,
            &ctx,
            0.6,
            vec![Event::PointerMoved(egui::pos2(30.0, rect.top() - 80.0))],
            Modifiers::SHIFT,
            true,
        );
        assert!(terminal.selection_drag);
        selection_frame(&mut terminal, &ctx, 0.7, vec![], Modifiers::SHIFT, false);
        assert!(!terminal.selection_drag);
        let stopped = terminal.parser.screen().scrollback();
        selection_frame(&mut terminal, &ctx, 0.8, vec![], Modifiers::SHIFT, false);
        assert_eq!(terminal.parser.screen().scrollback(), stopped);
    }
    #[test]
    fn selected_history_tracks_new_output_without_copying_the_wrong_rows() {
        let mut terminal = Terminal::default();
        terminal.parser = vt100::Parser::new_with_callbacks(3, 12, 5, Replies::default());
        terminal.process(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven");
        terminal.selection = Some(((-2, 0), (0, 4)));
        let before = terminal.selected_text().unwrap();
        terminal.parser.screen_mut().set_scrollback(2);
        terminal.process(b"\r\neight");
        assert_eq!(terminal.selected_text().unwrap(), before);
        assert_eq!(terminal.parser.screen().scrollback(), 3);
        terminal.process(b"\x1b[?1049h");
        assert!(terminal.selection.is_none());
        let mut short = Terminal::default();
        short.parser = vt100::Parser::new_with_callbacks(3, 12, 100, Replies::default());
        short.process(b"zero\r\none\r\ntwo\r\nthree");
        assert_eq!(short.scrollback_available, 1);
        short.selection = Some(((0, 0), (0, 2)));
        short.process(b"\rTHREE");
        assert_eq!(
            short.selected_text().as_deref(),
            Some("one"),
            "a harmless redraw must not erase selection with one history row"
        );
    }
    #[test]
    fn sparse_terminal_draws_visible_ink_without_thousands_of_empty_cells() {
        let mut terminal = Terminal::default();
        terminal.process(b"\x1b[41m   \x1b[0mA\x1b[4m \x1b[0m");
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1800.0, 900.0),
            )),
            ..Default::default()
        };
        let mut output = egui::FullOutput::default();
        let started = std::time::Instant::now();
        for _ in 0..3 {
            output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    terminal.ui(ui, true);
                });
            });
        }
        let empty_text = output
            .shapes
            .iter()
            .filter(|s| {
                matches!(&s.shape,
            egui::Shape::Text(t) if t.galley.job.text.trim().is_empty())
            })
            .count();
        eprintln!(
            "sparse terminal: {} shapes, {} blank text shapes, {:?} for 3 frames",
            output.shapes.len(),
            empty_text,
            started.elapsed()
        );
        assert!(output.shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Text(t) if t.galley.job.text == "A")));
        assert!(output.shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Rect(r) if r.fill == terminal.settings.ansi(1))));
        assert!(output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape, egui::Shape::LineSegment { .. })));
        assert_eq!(
            empty_text, 0,
            "blank terminal cells should not create text layouts"
        );
        assert!(
            output.shapes.len() < 100,
            "sparse-screen drawing work should follow visible content"
        );
    }
    #[test]
    fn terminal_scrollbar_tracks_real_history_and_keeps_gutter_out_of_pty_input() {
        let mut t = Terminal::default();
        t.parser.screen_mut().set_size(3, 20);
        t.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
        assert!(t.scrollback_available > 0);
        let retained = t.scrollback_available;
        t.parser.screen_mut().set_scrollback(1);
        t.refresh_history();
        assert_eq!(t.parser.screen().scrollback(), 1);
        let track = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 200.0));
        let bottom = scroll_thumb(track, 3, retained, 0);
        let top = scroll_thumb(track, 3, retained, retained);
        assert_eq!(bottom.bottom(), track.bottom());
        assert_eq!(top.top(), track.top());
        assert_eq!(
            scroll_offset(track, top.height(), retained, track.top()),
            retained
        );
        assert_eq!(
            scroll_offset(track, bottom.height(), retained, bottom.top()),
            0
        );
        t.process(b"\x1b[?1049h");
        assert_eq!(t.scrollback_available, 0);
        t.process(b"\x1b[?1049l");
        assert_eq!(t.scrollback_available, retained);
        // Exercise the production UI with mouse reporting enabled. A click on
        // the rail must only move history; no escape/mouse bytes reach the PTY.
        let ctx = egui::Context::default();
        for _ in 0..80 {
            t.process(b"line\r\n");
        }
        t.process(b"\x1b[?1000h\x1b[?1006h");
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 240.0),
            )),
            ..Default::default()
        };
        let mut gutter = egui::Pos2::ZERO;
        let _ = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                gutter = egui::pos2(ui.max_rect().right() - 9.0, ui.max_rect().top() + 4.0);
                assert!(t.ui(ui, true).input.is_empty());
            });
        });
        for pressed in [true, false] {
            input.events = vec![
                egui::Event::PointerMoved(gutter),
                egui::Event::PointerButton {
                    pos: gutter,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(t.ui(ui, true).input.is_empty());
                });
            });
        }
        assert!(t.parser.screen().scrollback() > 0);
    }
    #[test]
    fn word_colours_map_wide_cells_and_preserve_host_ansi() {
        let settings = crate::terminal_settings::Settings::default();
        let highlighter = crate::terminal_settings::Highlighter::new(&settings);
        let mut terminal = Terminal::default();
        terminal.process("界 café error \x1b[32merror\x1b[0m normal".as_bytes());
        let before = terminal.visible_text();
        let screen = terminal.parser.screen();
        let colors = row_word_colours(screen, 0, 80, &settings, &highlighter);
        assert_eq!(colors[0], None);
        assert_eq!(colors[1], None);
        assert_eq!(colors[7], None);
        assert_eq!(colors[8], Some(settings.color("error")));
        assert_eq!(colors[12], Some(settings.color("error")));
        assert_eq!(colors[13], None);
        assert_eq!(colors[14], None);
        assert_eq!(screen.cell(0, 14).unwrap().fgcolor(), vt100::Color::Idx(2));
        assert_eq!(colors[20], None);
        assert_eq!(terminal.visible_text(), before);
    }
    #[test]
    fn split_osc7_reports_unicode_paths_without_executing_output() {
        let mut t = Terminal::default();
        for byte in b"\x1b]7;file://server/home/a%20b/%C3%B1\x1b\\" {
            assert!(t.process(&[*byte]).is_empty());
        }
        assert_eq!(t.take_directory().as_deref(), Some("/home/a b/ñ"));
        assert!(t.visible_text().is_empty());
        for bad in [
            b"file://server/a%0aevil".as_slice(),
            b"https://server/a",
            b"file://server/a%ZZ",
        ] {
            assert!(osc_directory(bad).is_none());
        }
    }
    #[test]
    fn mouse_protocols_encode_modifiers_release_wide_coordinates() {
        assert_eq!(
            mouse_bytes(
                0,
                (3, 299),
                false,
                false,
                Modifiers::CTRL,
                vt100::MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<16;300;4M"
        );
        assert_eq!(
            mouse_bytes(
                2,
                (0, 0),
                true,
                false,
                Modifiers::NONE,
                vt100::MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<2;1;1m"
        );
        assert!(mouse_bytes(
            0,
            (0, 299),
            false,
            false,
            Modifiers::NONE,
            vt100::MouseProtocolEncoding::Default
        )
        .is_empty());
        assert!(std::str::from_utf8(&mouse_bytes(
            0,
            (0, 299),
            false,
            false,
            Modifiers::NONE,
            vt100::MouseProtocolEncoding::Utf8
        ))
        .is_ok());
    }
    #[test]
    fn byte_boundaries_and_ansi_colours_survive_output_chunks() {
        let mut terminal = Terminal::default();
        for b in "\x1b[31mé\x1b[0m".as_bytes() {
            terminal.process(&[*b]);
        }
        assert_eq!(terminal.parser.screen().cell(0, 0).unwrap().contents(), "é");
        assert_eq!(
            terminal.parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
    }
    #[test]
    fn readline_and_fullscreen_application_keys_are_sent_to_host() {
        assert_eq!(key_bytes(Key::A, Modifiers::CTRL, false), Some(vec![1]));
        assert_eq!(key_bytes(Key::W, Modifiers::CTRL, false), Some(vec![23]));
        assert_eq!(key_bytes(Key::Minus, Modifiers::CTRL, true), Some(vec![31]));
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(Key::F, Modifiers::ALT, false),
            Some(b"\x1bf".to_vec())
        );
    }
    #[test]
    fn terminal_queries_are_answered_without_reading_clipboard() {
        let mut terminal = Terminal::default();
        assert_eq!(terminal.process(b"\x1b[6n"), b"\x1b[1;1R");
        assert!(terminal.process(b"\x1b]52;c;?\x07").is_empty());
        assert_eq!(
            paste_bytes("a\x1b[201~b", true),
            b"\x1b[200~a[201~b\x1b[201~"
        );
    }
}
