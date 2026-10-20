//! Native terminal screen: byte-stream parsing is independent of UI repaint.
use eframe::egui::{self, Color32, Event, Key, Modifiers};

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
    selection: Option<((u16, u16), (u16, u16))>,
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
            history_limit: settings.scrollback_lines,
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
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }
    pub fn take_directory(&mut self) -> Option<String> {
        // The session's file-window switch gates both directions. Always drain
        // OSC updates here so disabled synchronization cannot replay an old path.
        self.parser.callbacks_mut().1.take()
    }
    pub fn release_mouse(&mut self) -> Vec<u8> {
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
    pub fn selected_text(&self) -> Option<String> {
        self.selection.map(|(a, b)| {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            self.parser
                .screen()
                .contents_between(a.0, a.1, b.0, b.1.saturating_add(1))
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
                self.selection = None;
                self.history_limit = settings.scrollback_lines;
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
        let cols = (available.x / cell_width).floor().clamp(2.0, 500.0) as u16;
        let rows = (available.y / cell_height).floor().clamp(2.0, 250.0) as u16;
        let mut action = TerminalAction { input: Vec::new() };
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.selection = None;
        }
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, available.y.max(2.0)),
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
        let report_mouse = enabled
            && mouse_mode != vt100::MouseProtocolMode::None
            && !ui.input(|i| i.modifiers.shift);
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
        if response.drag_started() && !report_mouse {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = cell_at(pos);
                self.selection = Some((cell, cell));
            }
        }
        if response.dragged() && !report_mouse {
            if let (Some((_, end)), Some(pos)) =
                (self.selection.as_mut(), response.interact_pointer_pos())
            {
                *end = cell_at(pos);
            }
        }
        if response.clicked() {
            self.selection = None;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
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
                    let offset = if scroll > 0.0 {
                        old.saturating_add(3)
                    } else {
                        old.saturating_sub(3)
                    };
                    self.parser.screen_mut().set_scrollback(offset);
                    self.selection = None;
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
                            self.selection = None;
                        } else if modifiers.ctrl && modifiers.shift && key == Key::A {
                            self.selection = Some(((0, 0), (rows - 1, cols - 1)));
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
                self.selection = None;
            }
        }
        if response.secondary_clicked() && !report_mouse {
            if let Some(text) = self.selected_text().filter(|s| !s.is_empty()) {
                ui.ctx().copy_text(text);
                self.selection = None;
            } else if enabled {
                if let Ok(text) = clipboard_text() {
                    action
                        .input
                        .extend(paste_bytes(&text, self.parser.screen().bracketed_paste()));
                }
            }
        }
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, self.settings.color("background"));
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
                    if a <= (row, col) && (row, col) <= b {
                        background = self.settings.color("selection");
                    }
                }
                if cell.is_wide_continuation() {
                    continue;
                }
                painter.rect_filled(cell_rect, 0.0, background);
                if cell.dim() {
                    foreground = foreground.gamma_multiply(0.65);
                }
                let text = cell.contents();
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
#[cfg(test)]
mod tests {
    use super::*;
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
