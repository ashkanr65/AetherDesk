//! Application preferences, deliberately separate from remote desktop geometry.
use crate::theme_palette::{self, Palette, Themes};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, path::Path};

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    System,
    Light,
    #[default]
    Dark,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Binding {
    pub key: String,
    pub shift: bool,
}
impl Default for Binding {
    fn default() -> Self {
        Self {
            key: "K".into(),
            shift: true,
        }
    }
}
impl Binding {
    pub fn label(&self) -> String {
        format!(
            "Ctrl+{}{}",
            if self.shift { "Shift+" } else { "" },
            self.key
        )
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shortcuts {
    pub new_connection: Binding,
    pub sidebar: Binding,
    pub next_tab: Binding,
    pub previous_tab: Binding,
}
impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            new_connection: Binding::default(),
            sidebar: Binding {
                key: "B".into(),
                shift: true,
            },
            next_tab: Binding {
                key: "Tab".into(),
                shift: false,
            },
            previous_tab: Binding {
                key: "Tab".into(),
                shift: true,
            },
        }
    }
}
impl Shortcuts {
    pub fn rows_mut(&mut self) -> [(&'static str, &mut Binding); 4] {
        [
            ("New connection", &mut self.new_connection),
            ("Toggle sidebar", &mut self.sidebar),
            ("Next tab", &mut self.next_tab),
            ("Previous tab", &mut self.previous_tab),
        ]
    }
    fn validate(&self) -> Result<(), String> {
        let rows = [
            &self.new_connection,
            &self.sidebar,
            &self.next_tab,
            &self.previous_tab,
        ];
        let mut used = std::collections::HashSet::new();
        for binding in rows {
            if key(&binding.key).is_none() || !used.insert((&binding.key, binding.shift)) {
                return Err("Choose a supported, different shortcut for each action".into());
            }
        }
        Ok(())
    }
}
pub const KEYS: [&str; 14] = [
    "B", "J", "K", "L", "M", "N", "Tab", "PageUp", "PageDown", "F6", "F7", "F8", "F9", "F10",
];
pub fn key(name: &str) -> Option<egui::Key> {
    Some(match name {
        "B" => egui::Key::B,
        "J" => egui::Key::J,
        "K" => egui::Key::K,
        "L" => egui::Key::L,
        "M" => egui::Key::M,
        "N" => egui::Key::N,
        "Tab" => egui::Key::Tab,
        "PageUp" => egui::Key::PageUp,
        "PageDown" => egui::Key::PageDown,
        "F6" => egui::Key::F6,
        "F7" => egui::Key::F7,
        "F8" => egui::Key::F8,
        "F9" => egui::Key::F9,
        "F10" => egui::Key::F10,
        _ => return None,
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub schema: u32,
    pub mode: Mode,
    pub ui_scale: f32,
    pub font_size: f32,
    pub themes: Themes,
    pub start_maximized: bool,
    pub restore_tabs: bool,
    pub confirm_quit: bool,
    pub default_port: u16,
    pub remember_username: bool,
    pub connect_timeout: u64,
    pub system_editor: bool,
    pub follow_cwd: Option<bool>,
    pub log_level: String,
    pub log_file_mb: u64,
    pub log_generations: usize,
    pub shortcuts: Shortcuts,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: 1,
            mode: Mode::Dark,
            ui_scale: 1.0,
            font_size: 13.0,
            themes: Themes::default(),
            start_maximized: false,
            restore_tabs: false,
            confirm_quit: true,
            default_port: 22,
            remember_username: false,
            connect_timeout: 75,
            system_editor: false,
            follow_cwd: None,
            log_level: "info".into(),
            log_file_mb: 10,
            log_generations: 5,
            shortcuts: Shortcuts::default(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err("Unsupported preferences format; the file was preserved".into());
        }
        if !self.ui_scale.is_finite()
            || !(0.8..=1.6).contains(&self.ui_scale)
            || !self.font_size.is_finite()
            || !(11.0..=20.0).contains(&self.font_size)
        {
            return Err(
                "Choose an application scale from 80–160% and a font size from 11–20".into(),
            );
        }
        if self.default_port == 0 || !(10..=300).contains(&self.connect_timeout) {
            return Err(
                "Choose a valid SSH port and a connection timeout from 10–300 seconds".into(),
            );
        }
        if !["error", "warn", "info", "debug", "trace"].contains(&self.log_level.as_str())
            || !(1..=100).contains(&self.log_file_mb)
            || !(1..=20).contains(&self.log_generations)
        {
            return Err("Invalid log level or storage limit".into());
        }
        self.shortcuts.validate()
    }
    pub fn dark(&self, ctx: &egui::Context, fallback: bool) -> bool {
        match self.mode {
            Mode::Dark => true,
            Mode::Light => false,
            Mode::System => ctx
                .system_theme()
                .map(|t| t == egui::Theme::Dark)
                .unwrap_or(fallback),
        }
    }
}
pub fn load() -> Result<Option<Settings>, String> {
    crate::storage::read_json("preferences.json")?
        .map(|value| {
            let settings: Settings = serde_json::from_value(value).map_err(|_| {
                "Preferences could not be read; the original file was preserved".to_owned()
            })?;
            settings.validate()?;
            Ok(settings)
        })
        .transpose()
}
#[cfg(not(test))]
fn store() -> &'static std::sync::RwLock<Settings> {
    static STORE: std::sync::OnceLock<std::sync::RwLock<Settings>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::RwLock::new(Settings::default()))
}
#[cfg(test)]
thread_local! {static TEST_SETTINGS:std::cell::RefCell<Settings>=std::cell::RefCell::new(Settings::default());}
pub fn current() -> Settings {
    #[cfg(not(test))]
    {
        store().read().map(|v| v.clone()).unwrap_or_default()
    }
    #[cfg(test)]
    {
        TEST_SETTINGS.with(|v| v.borrow().clone())
    }
}
fn install(settings: Settings) {
    theme_palette::install(settings.themes);
    #[cfg(not(test))]
    if let Ok(mut v) = store().write() {
        *v = settings;
    }
    #[cfg(test)]
    TEST_SETTINGS.with(|v| *v.borrow_mut() = settings);
}
#[derive(Clone, Copy, PartialEq)]
pub enum Section {
    General,
    Display,
    Connections,
    Files,
    Assistant,
    Keyboard,
    Updates,
    Logs,
}
impl Section {
    const ALL: [Self; 8] = [
        Self::General,
        Self::Display,
        Self::Connections,
        Self::Files,
        Self::Assistant,
        Self::Keyboard,
        Self::Updates,
        Self::Logs,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Display => "Display",
            Self::Connections => "Connections",
            Self::Files => "Files and Editor",
            Self::Assistant => "AI Assistant",
            Self::Keyboard => "Keyboard",
            Self::Updates => "Updates",
            Self::Logs => "Logs",
        }
    }
}
pub enum Action {
    Applied,
    Assistant,
    Updates,
    Logs,
}
pub struct Panel {
    pub settings: Settings,
    pub open: bool,
    draft: Settings,
    section: Section,
    dark_tab: bool,
    hex: HashMap<String, String>,
    path: String,
    message: String,
    valid: bool,
    follow_cwd: bool,
}
impl Panel {
    pub fn load(legacy_dark: bool) -> Self {
        let loaded = load();
        let valid = loaded.is_ok();
        let message = loaded.as_ref().err().cloned().unwrap_or_default();
        let settings = loaded.ok().flatten().unwrap_or_else(|| Settings {
            mode: if legacy_dark { Mode::Dark } else { Mode::Light },
            ..Default::default()
        });
        install(settings.clone());
        Self {
            draft: settings.clone(),
            settings,
            open: false,
            section: Section::General,
            dark_tab: legacy_dark,
            hex: HashMap::new(),
            path: String::new(),
            message,
            valid,
            follow_cwd: current()
                .follow_cwd
                .unwrap_or_else(|| crate::terminal_settings::current().1.follow_cwd),
        }
    }
    pub fn show(&mut self) {
        self.draft = self.settings.clone();
        self.hex.clear();
        self.follow_cwd = self
            .settings
            .follow_cwd
            .unwrap_or_else(|| crate::terminal_settings::current().1.follow_cwd);
        self.open = true;
    }
    pub fn toggle_mode(&mut self, dark: bool) -> Result<(), String> {
        let mut settings = self.settings.clone();
        settings.mode = if dark { Mode::Dark } else { Mode::Light };
        self.save(settings)
    }
    fn save(&mut self, settings: Settings) -> Result<(), String> {
        if !self.valid {
            return Err(
                "The damaged preferences file has been preserved. Repair or move it before saving."
                    .into(),
            );
        }
        settings.validate()?;
        crate::storage::write_json(
            "preferences.json",
            &serde_json::to_value(&settings).map_err(|e| e.to_string())?,
        )?;
        install(settings.clone());
        self.settings = settings;
        Ok(())
    }
    fn restore_section(&mut self, defaults: bool) {
        let source = if defaults {
            Settings::default()
        } else {
            self.settings.clone()
        };
        copy_section(&mut self.draft, &source, self.section);
        if self.section == Section::Files {
            self.follow_cwd = if defaults {
                true
            } else {
                source
                    .follow_cwd
                    .unwrap_or_else(|| crate::terminal_settings::current().1.follow_cwd)
            };
        }
        if self.section == Section::Display {
            self.hex.clear();
        }
        self.message = if defaults {
            "Defaults previewed for this section. Apply and save to keep them.".into()
        } else {
            "Unapplied changes in this section were discarded.".into()
        };
    }
    fn apply_section(&mut self) -> Result<(), String> {
        if self.section == Section::Display
            && self
                .hex
                .values()
                .any(|text| theme_palette::parse_hex(text).is_none())
        {
            return Err(
                "Correct the invalid hexadecimal colour before applying Display preferences."
                    .into(),
            );
        }
        if self.section == Section::Files {
            self.draft.follow_cwd = Some(self.follow_cwd);
        }
        let mut settings = self.settings.clone();
        copy_section(&mut settings, &self.draft, self.section);
        self.save(settings)?;
        self.message = format!("{} preferences saved.", self.section.label());
        Ok(())
    }
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        let size = crate::app_layout::dialog_size(ctx, egui::vec2(900.0, 650.0));
        egui::Window::new("Preferences")
            .id(egui::Id::new("app-preferences"))
            .open(&mut open)
            .fixed_size(size)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let enter = crate::dialog::primary(ui);
                // Allocate the window once. Child content cannot increase its
                // height or scroll the navigation/footer out of the window.
                let (bounds, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let compact = size.x < 660.0;
                let mut navigation = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("preferences-navigation")
                        .max_rect(if compact {
                            egui::Rect::from_min_size(bounds.min, egui::vec2(bounds.width(), 34.0))
                        } else {
                            egui::Rect::from_min_size(
                                bounds.min,
                                egui::vec2(150.0, bounds.height()),
                            )
                        })
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                navigation.set_clip_rect(navigation.max_rect().intersect(ui.clip_rect()));
                if compact {
                    egui::ComboBox::from_id_salt("compact-preferences-section")
                        .selected_text(self.section.label())
                        .show_ui(&mut navigation, |ui| {
                            for section in Section::ALL {
                                ui.selectable_value(&mut self.section, section, section.label());
                            }
                        });
                } else {
                    for section in Section::ALL {
                        if navigation
                            .selectable_label(self.section == section, section.label())
                            .clicked()
                        {
                            self.section = section;
                            self.message.clear();
                        }
                    }
                }
                if !compact {
                    ui.painter().vline(
                        bounds.left() + 156.0,
                        bounds.y_range(),
                        ui.visuals().widgets.noninteractive.bg_stroke,
                    );
                }
                let section_rect = egui::Rect::from_min_max(
                    bounds.min
                        + if compact {
                            egui::vec2(0.0, 42.0)
                        } else {
                            egui::vec2(162.0, 0.0)
                        },
                    bounds.max,
                );
                // Reserve the status line and actions before laying out the
                // scroll viewport. All scroll painting is clipped above them.
                let footer_height = (ui.spacing().interact_size.y + 12.0).max(40.0);
                let footer_rect = egui::Rect::from_min_max(
                    egui::pos2(section_rect.left(), section_rect.bottom() - footer_height),
                    section_rect.max,
                );
                let message_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
                let content_rect = egui::Rect::from_min_max(
                    section_rect.min,
                    egui::pos2(
                        section_rect.right(),
                        footer_rect.top() - message_height - 8.0,
                    ),
                );
                let mut content = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("preferences-section")
                        .max_rect(content_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                content.set_clip_rect(content_rect.intersect(ui.clip_rect()));
                form_style(&mut content);
                action = egui::ScrollArea::both()
                    .id_salt(("preferences-content", self.section.label()))
                    .auto_shrink([false, false])
                    .max_height(content_rect.height())
                    .max_width(content_rect.width())
                    .show(&mut content, |ui| {
                        ui.set_width(ui.available_width().max(360.0));
                        self.section_ui(ui)
                    })
                    .inner;
                if !self.message.is_empty() {
                    let message_rect = egui::Rect::from_min_max(
                        egui::pos2(section_rect.left(), content_rect.bottom() + 4.0),
                        egui::pos2(section_rect.right(), footer_rect.top()),
                    );
                    let mut message = ui.new_child(egui::UiBuilder::new().max_rect(message_rect));
                    message.set_clip_rect(message_rect.intersect(ui.clip_rect()));
                    message
                        .add(egui::Label::new(&self.message).truncate())
                        .on_hover_text(&self.message);
                }
                ui.painter().hline(
                    section_rect.x_range(),
                    footer_rect.top(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                let mut footer = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("preferences-section-actions")
                        .max_rect(footer_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                footer.set_clip_rect(footer_rect.intersect(ui.clip_rect()));
                let editable = !matches!(self.section, Section::Assistant | Section::Updates);
                if footer
                    .button("Cancel")
                    .on_hover_text("Discard changes in this section")
                    .clicked()
                {
                    self.restore_section(false);
                }
                if footer
                    .add_enabled(editable, egui::Button::new("Reset to defaults"))
                    .clicked()
                {
                    self.restore_section(true);
                }
                let response =
                    footer.add_enabled(self.valid && editable, egui::Button::new("Apply and save"));
                crate::dialog::focus_on_open(&mut footer, &response);
                if self.valid && editable && (response.clicked() || enter) {
                    match self.apply_section() {
                        Ok(()) => action = Some(Action::Applied),
                        Err(error) => self.message = error,
                    }
                }
            });
        if !open {
            self.open = false;
            self.draft = self.settings.clone();
            self.hex.clear();
        }
        action
    }
    fn section_ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        ui.strong(self.section.label());
        ui.add_space(8.0);
        match self.section {
            Section::General => {
                ui.checkbox(
                    &mut self.draft.start_maximized,
                    "Start with the application window maximized",
                );
                ui.checkbox(
                    &mut self.draft.restore_tabs,
                    "Reopen saved connection tabs on startup",
                );
                ui.label(
                    "Tabs reopen disconnected. Passwords are never stored in this preference.",
                );
                ui.checkbox(
                    &mut self.draft.confirm_quit,
                    "Confirm quitting while connections are active",
                );
                ui.label("Unsaved editor work is always protected. Startup options take effect at the next launch.");
            }
            Section::Display => self.display(ui),
            Section::Connections => {
                field_grid(ui, "connection-preferences", |ui| {
                    ui.label("Default SSH port");
                    ui.add(egui::DragValue::new(&mut self.draft.default_port).range(1..=65535));
                    ui.end_row();
                    ui.label("Connection timeout");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.connect_timeout)
                            .range(10..=300)
                            .suffix(" s"),
                    );
                    ui.end_row();
                });
                ui.checkbox(
                    &mut self.draft.remember_username,
                    "Remember usernames by default for new saved connections",
                );
                ui.label("New defaults do not modify saved connections. Timeout changes apply to future connection attempts.");
                ui.label("Reconnect from the connection tab when needed. Connections are not silently retried with saved credentials.");
                ui.label("WireGuard configuration remains isolated to each connection tab.");
            }
            Section::Files => {
                ui.checkbox(
                    &mut self.follow_cwd,
                    "Synchronize terminal and file directories in new SSH tabs",
                );
                ui.label("The Files window can override synchronization for its own tab.");
                ui.checkbox(
                    &mut self.draft.system_editor,
                    "Open files with the Windows default application by default",
                );
                ui.label("When disabled, supported text files use the built-in editor. Explicit Open with and Open with text editor actions take priority.");
                ui.label("Deletion and replacement keep their existing confirmation dialogs. Terminal font and syntax colours remain in Terminal settings.");
            }
            Section::Assistant => {
                ui.label("Manage services, models, workspace, message limits and tool permissions in the assistant's own preferences.");
                if ui.button("Open AI Assistant preferences").clicked() {
                    return Some(Action::Assistant);
                }
                ui.label("API keys stay in the protected credential store; themes and application preferences do not export them.");
            }
            Section::Keyboard => {
                ui.label("Application shortcuts use Ctrl. Choose a key and optionally add Shift.");
                field_grid(ui, "application-shortcuts", |ui| {
                    for (label, binding) in self.draft.shortcuts.rows_mut() {
                        ui.label(label);
                        ui.horizontal_wrapped(|ui| {
                            egui::ComboBox::from_id_salt(label)
                                .width(100.0)
                                .selected_text(&binding.key)
                                .show_ui(ui, |ui| {
                                    for name in KEYS {
                                        ui.selectable_value(
                                            &mut binding.key,
                                            name.to_owned(),
                                            name,
                                        );
                                    }
                                });
                            ui.checkbox(&mut binding.shift, "Shift");
                        });
                        ui.end_row();
                    }
                });
                ui.label("These apply to AetherDesk navigation. Remote desktop and terminal shortcuts keep their own input handling.");
                if ui.button("Restore default shortcuts").clicked() {
                    self.draft.shortcuts = Shortcuts::default();
                }
            }
            Section::Updates => {
                ui.label("Configure automatic checks and downloads, review releases and install verified updates.");
                if ui.button("Open Updates").clicked() {
                    return Some(Action::Updates);
                }
                ui.label("Installation always asks before restarting. Preview releases are excluded from stable updates.");
            }
            Section::Logs => {
                field_grid(ui, "logging-preferences", |ui| {
                    ui.label("Capture level");
                    egui::ComboBox::from_id_salt("capture-level")
                        .width(100.0)
                        .selected_text(&self.draft.log_level)
                        .show_ui(ui, |ui| {
                            for level in ["error", "warn", "info", "debug", "trace"] {
                                ui.selectable_value(
                                    &mut self.draft.log_level,
                                    level.to_owned(),
                                    level,
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Size per log file");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.log_file_mb)
                            .range(1..=100)
                            .suffix(" MiB"),
                    );
                    ui.end_row();
                    ui.label("Rotated files to retain");
                    ui.add(egui::DragValue::new(&mut self.draft.log_generations).range(1..=20));
                    ui.end_row();
                });
                ui.label("Limits apply per application instance. Capture level changes take effect after restart; AETHERDESK_LOG_LEVEL overrides the selected level.");
                if ui.button("Open Log files").clicked() {
                    return Some(Action::Logs);
                }
            }
        }
        None
    }
    fn display(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().slider_width = (ui.available_width() * 0.5 - 100.0).clamp(48.0, 180.0);
        field_grid(ui, "display-preferences", |ui| {
            ui.label("Theme mode");
            ui.horizontal_wrapped(|ui| {
                for (mode, label) in [
                    (Mode::System, "System"),
                    (Mode::Light, "Light"),
                    (Mode::Dark, "Dark"),
                ] {
                    ui.selectable_value(&mut self.draft.mode, mode, label);
                }
            });
            ui.end_row();
            ui.label("Application scale");
            application_slider(ui, &mut self.draft.ui_scale, 0.8..=1.6, 2);
            ui.end_row();
            ui.label("Application font size");
            application_slider(ui, &mut self.draft.font_size, 11.0..=20.0, 1);
            ui.end_row();
        });
        ui.label("Application colours and scaling do not alter remote monitor resolution, remote pixels or terminal syntax colours.");
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.dark_tab, false, "Light colours");
            ui.selectable_value(&mut self.dark_tab, true, "Dark colours");
        });
        let palette = if self.dark_tab {
            &mut self.draft.themes.dark
        } else {
            &mut self.draft.themes.light
        };
        preview(ui, *palette);
        let warnings = palette.contrast_warnings();
        if !warnings.is_empty() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "Low contrast: {}. You can still save these colours.",
                    warnings.join(", ")
                ),
            );
        }
        if ui.button("Reset this palette").clicked() {
            *palette = Palette::defaults(self.dark_tab);
            self.hex.clear();
        }
        let label_width = (ui.available_width() - 160.0).clamp(80.0, 240.0);
        egui::Grid::new(("palette-fields", self.dark_tab))
            .num_columns(3)
            .max_col_width(label_width)
            .min_row_height(ui.spacing().interact_size.y)
            .spacing([12.0, 10.0])
            .show(ui, |ui| {
                let mut group = "";
                for (section, label, rgb) in palette.roles_mut() {
                    if section != group {
                        ui.strong(section);
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                        group = section;
                    }
                    ui.label(label);
                    let picker = ui.color_edit_button_srgb(rgb);
                    let text = self
                        .hex
                        .entry(format!("{}:{label}", self.dark_tab))
                        .or_insert_with(|| theme_palette::hex(*rgb));
                    if picker.changed() {
                        *text = theme_palette::hex(*rgb);
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(text)
                            .font(egui::TextStyle::Body)
                            .desired_width(90.0)
                            .char_limit(7),
                    );
                    if let Some(color) = theme_palette::parse_hex(text) {
                        if response.changed() {
                            *rgb = color;
                        }
                    } else {
                        response.on_hover_text("Enter six hexadecimal digits, for example #C7A2B3");
                    }
                    ui.end_row();
                }
            });
        ui.separator();
        ui.label("Theme file (.json)");
        ui.add_sized(
            [ui.available_width(), ui.spacing().interact_size.y],
            egui::TextEdit::singleline(&mut self.path).font(egui::TextStyle::Body),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Import palettes").clicked() {
                match import_themes(Path::new(&self.path)) {
                    Ok(themes) => {
                        self.draft.themes = themes;
                        self.hex.clear();
                        self.message =
                            "Palettes imported into the preview. Apply and save to keep them."
                                .into();
                    }
                    Err(error) => self.message = error,
                }
            }
            if ui.button("Export palettes").clicked() {
                self.message = match export_themes(Path::new(&self.path), self.draft.themes) {
                    Ok(()) => {
                        "Palettes exported. No profiles, credentials or chat data included.".into()
                    }
                    Err(error) => error,
                };
            }
        });
    }
}
// Keep sections independent: an unrelated draft (including invalid colour
// text) must never be saved or discarded by another section's actions.
fn copy_section(target: &mut Settings, source: &Settings, section: Section) {
    match section {
        Section::General => {
            target.start_maximized = source.start_maximized;
            target.restore_tabs = source.restore_tabs;
            target.confirm_quit = source.confirm_quit;
        }
        Section::Display => {
            target.mode = source.mode;
            target.ui_scale = source.ui_scale;
            target.font_size = source.font_size;
            target.themes = source.themes;
        }
        Section::Connections => {
            target.default_port = source.default_port;
            target.remember_username = source.remember_username;
            target.connect_timeout = source.connect_timeout;
        }
        Section::Files => {
            target.system_editor = source.system_editor;
            target.follow_cwd = source.follow_cwd;
        }
        Section::Keyboard => target.shortcuts = source.shortcuts.clone(),
        Section::Logs => {
            target.log_level = source.log_level.clone();
            target.log_file_mb = source.log_file_mb;
            target.log_generations = source.log_generations;
        }
        Section::Assistant | Section::Updates => {}
    }
}
// Apply only to the Preferences content column: preserve the chosen body font,
// use it for labels and controls, and wrap descriptions inside the viewport.
fn form_style(ui: &mut egui::Ui) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let height = (ui.text_style_height(&egui::TextStyle::Body) + 10.0).max(28.0);
    let style = ui.style_mut();
    style.override_font_id = Some(font);
    style.override_text_style = Some(egui::TextStyle::Body);
    style.drag_value_text_style = egui::TextStyle::Body;
    style.wrap_mode = Some(egui::TextWrapMode::Wrap);
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.interact_size.y = height;
}
fn field_grid<R>(ui: &mut egui::Ui, id: &str, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let column = ((ui.available_width() - 16.0) * 0.5).max(1.0);
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(column)
        .max_col_width(column)
        .min_row_height(ui.spacing().interact_size.y)
        .spacing([16.0, 10.0])
        .show(ui, contents)
        .inner
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    schema: u32,
    themes: Themes,
}
fn import_themes(path: &Path) -> Result<Themes, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return Err("Choose a theme JSON file up to 256 KiB".into());
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let file: ThemeFile = serde_json::from_slice(&data)
        .map_err(|_| "Invalid theme file; current colours were preserved".to_owned())?;
    if file.schema != 1 {
        return Err("Unsupported theme format".into());
    }
    Ok(file.themes)
}
fn export_themes(path: &Path, themes: Themes) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(&ThemeFile { schema: 1, themes }).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Choose a new theme filename: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn preview(ui: &mut egui::Ui, palette: Palette) {
    let c = palette.colors();
    let width = ui.available_width().clamp(1.0, 620.0);
    let height = width * 0.48;
    let (r, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let p = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
    let at = |x: f32, y: f32| r.min + egui::vec2(x * width, y * height);
    let rect = |x: f32, y: f32, w: f32, h: f32| {
        egui::Rect::from_min_size(at(x, y), egui::vec2(w * width, h * height))
    };
    p.rect_filled(r, 4, c.canvas);
    p.rect_filled(rect(0.0, 0.0, 1.0, 0.13), 4, c.toolbar);
    p.rect_filled(rect(0.0, 0.13, 0.06, 0.87), 0, c.rail);
    p.rect_filled(rect(0.06, 0.13, 0.23, 0.87), 0, c.sidebar);
    p.rect_filled(rect(0.30, 0.15, 0.20, 0.10), 2, c.tab_active);
    p.rect_filled(rect(0.51, 0.15, 0.20, 0.10), 2, c.tab);
    let font = egui::FontId::proportional((width / 48.0).clamp(9.0, 13.0));
    let text = |x: f32, y: f32, t: &str, color| {
        p.text(at(x, y), egui::Align2::LEFT_CENTER, t, font.clone(), color);
    };
    text(0.025, 0.065, "AetherDesk", c.text);
    text(0.32, 0.20, "Terminal  ×", c.tab_active_text);
    text(0.53, 0.20, "Files  ×", c.tab_text);
    for y in [0.23, 0.40, 0.57, 0.74] {
        crate::theme::icon(&p, rect(0.014, y, 0.030, 0.055), 1, c.icon);
    }
    p.rect_filled(rect(0.08, 0.20, 0.18, 0.10), 2, c.input);
    text(0.09, 0.25, "Search", c.muted);
    p.rect_filled(rect(0.08, 0.35, 0.18, 0.12), 2, c.selection);
    text(0.09, 0.41, "My connection", c.selection_text);
    text(0.33, 0.37, "Everything remote.", c.heading);
    text(0.33, 0.48, "Your workspace", c.text);
    p.rect_filled(rect(0.33, 0.59, 0.25, 0.12), 3, c.button);
    text(0.35, 0.65, "New connection", c.button_text);
    p.rect_filled(rect(0.65, 0.55, 0.29, 0.34), 3, c.popup);
    p.rect_filled(rect(0.65, 0.55, 0.29, 0.08), 3, c.titlebar);
    text(0.68, 0.59, "AI Assistant", c.selection_text);
    text(0.68, 0.70, "Hello!", c.text);
    text(0.68, 0.82, "Message…", c.muted);
    p.hline(
        r.x_range(),
        at(0.0, 0.94).y,
        egui::Stroke::new(1.0_f32, c.separator),
    );
    text(0.08, 0.97, "Connected", c.success);
    p.rect_stroke(
        r,
        4,
        egui::Stroke::new(1.0_f32, c.border),
        egui::StrokeKind::Inside,
    );
}
fn application_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    decimals: usize,
) {
    ui.scope(|ui| {
        let colors = crate::theme::colors(ui.visuals().dark_mode);
        ui.visuals_mut().widgets.inactive.bg_fill = colors.border;
        ui.visuals_mut().selection.bg_fill = colors.heading;
        ui.spacing_mut().slider_rail_height = 6.0;
        ui.add(
            egui::Slider::new(value, range)
                .fixed_decimals(decimals)
                .trailing_fill(true),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RestoreSettings(Settings);
    impl Drop for RestoreSettings {
        fn drop(&mut self) {
            install(self.0.clone());
        }
    }
    type PaintedText = (String, egui::Rect, f32, egui::Rect);
    fn rendered_section(section: Section, font_size: f32) -> Vec<PaintedText> {
        rendered_section_at(section, font_size, egui::vec2(1200.0, 900.0))
    }
    fn rendered_section_at(
        section: Section,
        font_size: f32,
        screen: egui::Vec2,
    ) -> Vec<PaintedText> {
        let _restore = RestoreSettings(current());
        let directory = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(directory.path(), || {
            let ctx = egui::Context::default();
            ctx.style_mut(|style| {
                style
                    .text_styles
                    .insert(egui::TextStyle::Body, egui::FontId::proportional(font_size));
                style.text_styles.insert(
                    egui::TextStyle::Button,
                    egui::FontId::proportional(font_size),
                );
            });
            let mut panel = Panel::load(true);
            panel.section = section;
            panel.show();
            let mut output = None;
            // Let egui settle its window/grid measurements before checking geometry.
            for _ in 0..8 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                        ..Default::default()
                    },
                    |ctx| {
                        let _ = panel.ui(ctx);
                    },
                ));
            }
            output
                .unwrap()
                .shapes
                .into_iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = shape.shape {
                        let font_size = text.galley.job.sections.first()?.format.font_id.size;
                        Some((
                            text.galley.job.text.clone(),
                            egui::Rect::from_min_size(text.pos, text.galley.size()),
                            font_size,
                            shape.clip_rect,
                        ))
                    } else {
                        None
                    }
                })
                .collect()
        })
    }
    fn find_text(texts: &[PaintedText], label: &str) -> (egui::Rect, f32, egui::Rect) {
        // Category names appear in both navigation and content. Select the title
        // in the right column, rather than its navigation counterpart.
        texts
            .iter()
            .filter(|(text, ..)| text == label)
            .max_by(|a, b| a.1.left().total_cmp(&b.1.left()))
            .map(|(_, rect, size, clip)| (*rect, *size, *clip))
            .unwrap_or_else(|| panic!("Missing Preferences text: {label}"))
    }
    #[test]
    fn section_actions_save_reset_and_cancel_only_the_selected_category() {
        let _restore = RestoreSettings(current());
        let directory = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(directory.path(), || {
            let mut panel = Panel::load(true);
            panel.show();
            let original = panel.settings.clone();
            panel.draft.start_maximized = !original.start_maximized;
            panel.draft.default_port = 2222;
            panel.draft.themes.dark.canvas = [1, 2, 3];
            panel.hex.insert("true:Canvas".into(), "invalid".into());
            panel.section = Section::General;
            panel.apply_section().unwrap();
            let saved = load().unwrap().unwrap();
            assert_eq!(saved.start_maximized, !original.start_maximized);
            assert_eq!(saved.default_port, original.default_port);
            assert_eq!(saved.themes, original.themes);
            assert_eq!(
                panel.draft.default_port, 2222,
                "other drafts must survive Apply"
            );
            panel.section = Section::Display;
            assert!(panel.apply_section().is_err());
            panel.restore_section(false);
            assert_eq!(panel.draft.themes, original.themes);
            assert!(panel.hex.is_empty());
            assert_eq!(
                panel.draft.default_port, 2222,
                "Cancel must only discard this section"
            );
            panel.section = Section::Connections;
            panel.apply_section().unwrap();
            panel.restore_section(true);
            assert_eq!(panel.draft.default_port, Settings::default().default_port);
            assert_eq!(
                load().unwrap().unwrap().default_port,
                2222,
                "Reset stays a draft"
            );
            assert_eq!(panel.draft.start_maximized, !original.start_maximized);
            panel.apply_section().unwrap();
            let saved = load().unwrap().unwrap();
            assert_eq!(saved.default_port, Settings::default().default_port);
            assert_eq!(saved.start_maximized, !original.start_maximized);
            assert!(
                panel.open,
                "section actions must leave navigation available"
            );
        });
    }
    #[test]
    fn display_scrolling_keeps_navigation_and_section_actions_fixed_and_uncovered() {
        let _restore = RestoreSettings(current());
        let directory = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(directory.path(), || {
            for screen in [egui::vec2(1200.0, 900.0), egui::vec2(640.0, 500.0)] {
                let ctx = egui::Context::default();
                let mut panel = Panel::load(true);
                panel.show();
                panel.section = Section::Display;
                let mut before = Vec::new();
                let mut after = Vec::new();
                let mut pointer = egui::Pos2::ZERO;
                for frame in 0..32 {
                    let mut events = Vec::new();
                    if frame >= 8 {
                        events.push(egui::Event::PointerMoved(pointer));
                    }
                    if frame == 8 {
                        events.push(egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -10000.0),
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                            time: Some(frame as f64 * 0.1),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            panel.ui(ctx);
                        },
                    );
                    let texts: Vec<PaintedText> = output
                        .shapes
                        .into_iter()
                        .filter_map(|shape| {
                            if let egui::epaint::Shape::Text(text) = shape.shape {
                                Some((
                                    text.galley.job.text.clone(),
                                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                                    text.galley.job.sections.first()?.format.font_id.size,
                                    shape.clip_rect,
                                ))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if frame == 7 {
                        pointer = find_text(&texts, "Theme mode").0.center();
                        before = texts;
                    } else if frame == 31 {
                        after = texts;
                    }
                }
                let (first, _, first_clip) = find_text(&before, "Theme mode");
                let (_, _, last_clip) = find_text(&after, "Import palettes");
                assert!(first_clip.contains_rect(first));
                assert!(
                    after.iter().all(
                        |(text, rect, _, clip)| text != "Theme mode" || !clip.intersects(*rect)
                    ),
                    "test must really scroll Display"
                );
                assert_eq!(first_clip, last_clip, "content viewport must remain fixed");
                for label in ["Apply and save", "Reset to defaults", "Cancel"] {
                    let (a, _, _) = find_text(&before, label);
                    let (b, _, clip) = find_text(&after, label);
                    assert_eq!(a, b, "{label} moved with Display content");
                    assert!(clip.contains_rect(b), "{label} was clipped");
                    assert!(
                        last_clip.bottom() < b.top(),
                        "Display content covers the footer"
                    );
                    assert!(b.bottom() <= screen.y, "{label} escaped the window");
                }
                // General is a fixed left-navigation item in the wide layout.
                if screen.x >= 660.0 {
                    assert_eq!(
                        find_text(&before, "General").0,
                        find_text(&after, "General").0
                    );
                }
                assert!(
                    after
                        .iter()
                        .any(|(text, rect, _, clip)| text == "Import palettes"
                            && clip.contains_rect(*rect)),
                    "the bottom of Display must be reachable without a covering blank region"
                );
            }
        });
    }
    #[test]
    fn compact_preferences_keep_visible_text_separate_and_actions_reachable() {
        for font in [13.0, 20.0] {
            for width in [480.0, 640.0] {
                for section in Section::ALL {
                    let texts = rendered_section_at(section, font, egui::vec2(width, 500.0));
                    let visible: Vec<_> = texts
                        .iter()
                        .filter_map(|(text, rect, _, clip)| {
                            let painted = rect.intersect(*clip);
                            (painted.width() > 1.0 && painted.height() > 1.0)
                                .then_some((text, painted))
                        })
                        .collect();
                    assert!(
                        visible
                            .iter()
                            .any(|(text, _)| text.as_str() == "Apply and save"),
                        "Preferences actions missing: {}",
                        section.label()
                    );
                    for (index, (text, rect)) in visible.iter().enumerate() {
                        assert!(
                            rect.left() >= -1.0 && rect.right() <= width + 1.0,
                            "Preferences overflows at {width}: {text}"
                        );
                        for (other, other_rect) in &visible[index + 1..] {
                            assert!(!rect.shrink(0.5).intersects(other_rect.shrink(0.5)), "Preferences text overlaps at {width}, font {font}: {text} / {other}");
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn preference_categories_stack_options_below_titles_with_consistent_fonts() {
        let cases = [
            (Section::General, "Start with the application window maximized"),
            (Section::Display, "Theme mode"),
            (Section::Connections, "Default SSH port"),
            (Section::Files, "Synchronize terminal and file directories in new SSH tabs"),
            (Section::Assistant, "Manage services, models, workspace, message limits and tool permissions in the assistant's own preferences."),
            (Section::Keyboard, "Application shortcuts use Ctrl. Choose a key and optionally add Shift."),
            (Section::Updates, "Configure automatic checks and downloads, review releases and install verified updates."),
            (Section::Logs, "Capture level"),
        ];
        for font_size in [13.0, 20.0] {
            for (section, first_option) in cases {
                let texts = rendered_section(section, font_size);
                let (title, title_size, _) = find_text(&texts, section.label());
                let (option, option_size, clip) = find_text(&texts, first_option);
                assert!(
                    option.top() >= title.bottom() + 4.0,
                    "{} title and option overlap: {title:?} / {option:?}",
                    section.label()
                );
                assert!(
                    (title_size - font_size).abs() < 0.1 && (option_size - font_size).abs() < 0.1,
                    "{} text sizes differ",
                    section.label()
                );
                assert!(
                    option.left() >= clip.left() - 1.0 && option.right() <= clip.right() + 1.0,
                    "{} option overflows its column: {option:?} / {clip:?}",
                    section.label()
                );
                if section == Section::General {
                    let (second, size, _) =
                        find_text(&texts, "Reopen saved connection tabs on startup");
                    let (third, _, _) =
                        find_text(&texts, "Confirm quitting while connections are active");
                    assert!(second.top() > option.bottom() && third.top() > second.bottom());
                    assert!((size - font_size).abs() < 0.1);
                }
            }
        }
    }
    #[test]
    fn preference_field_rows_align_labels_and_controls_inside_the_content_column() {
        for font_size in [13.0, 20.0] {
            for (section, label, value) in [
                (Section::Display, "Theme mode", "System"),
                (Section::Connections, "Default SSH port", "22"),
                (Section::Keyboard, "New connection", "K"),
                (Section::Logs, "Capture level", "info"),
            ] {
                let texts = rendered_section(section, font_size);
                let (label, _, _) = find_text(&texts, label);
                let (value, size, clip) = find_text(&texts, value);
                assert!(
                    (label.center().y - value.center().y).abs() <= 2.0,
                    "{} row is misaligned: {label:?} / {value:?}",
                    section.label()
                );
                assert!(
                    value.left() > label.right() && value.right() <= clip.right() + 1.0,
                    "{} control escapes its column",
                    section.label()
                );
                assert!((size - font_size).abs() < 0.1);
            }
        }
    }
    #[test]
    fn applied_preferences_round_trip_without_modifying_profiles_or_update_settings() {
        struct Restore(Settings);
        impl Drop for Restore {
            fn drop(&mut self) {
                install(self.0.clone());
            }
        }
        let _restore = Restore(current());
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let profiles = b"[profile fixture unchanged]";
            let updates = b"{\"automatic\":false}";
            std::fs::write(dir.path().join("sessions.json"), profiles).unwrap();
            std::fs::write(dir.path().join("updates.json"), updates).unwrap();
            let mut panel = Panel::load(false);
            panel.show();
            let before = panel.settings.themes;
            panel.draft.themes.dark.canvas = [7, 8, 9];
            panel.draft.themes.light.sidebar = [240, 230, 220];
            assert_eq!(
                current().themes,
                before,
                "draft changes must not alter the active palette"
            );
            panel.save(panel.draft.clone()).unwrap();
            let loaded = load().unwrap().unwrap();
            assert_eq!(loaded.themes, panel.draft.themes);
            assert_eq!(
                std::fs::read(dir.path().join("sessions.json")).unwrap(),
                profiles
            );
            assert_eq!(
                std::fs::read(dir.path().join("updates.json")).unwrap(),
                updates
            );
        });
    }
    #[test]
    fn preferences_preserve_legacy_mode_and_damaged_settings_until_explicit_repair() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let panel = Panel::load(false);
            assert!(panel.settings.mode == Mode::Light);
            std::fs::write(dir.path().join("preferences.json"), b"damaged {").unwrap();
            let mut panel = Panel::load(true);
            assert!(panel.save(Settings::default()).is_err());
            assert_eq!(
                std::fs::read(dir.path().join("preferences.json")).unwrap(),
                b"damaged {"
            );
        });
    }
    #[test]
    fn exported_themes_exclude_secrets_reject_bad_schema_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        let themes = Themes::default();
        export_themes(&path, themes).unwrap();
        assert_eq!(import_themes(&path).unwrap(), themes);
        assert!(export_themes(&path, themes).is_err());
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        std::fs::write(&path, b"{\"schema\":2,\"themes\":{}}").unwrap();
        assert!(import_themes(&path).is_err());
        let mut settings = Settings::default();
        settings.shortcuts.sidebar = settings.shortcuts.new_connection.clone();
        assert!(settings.validate().is_err());
        settings = Settings::default();
        settings.ui_scale = f32::NAN;
        assert!(settings.validate().is_err());
    }
}
