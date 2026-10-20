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
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        let mut apply = false;
        let mut cancel = false;
        let size = egui::vec2(900.0, 650.0).min(
            (ctx.content_rect().size() - egui::vec2(40.0, 70.0)).max(egui::vec2(400.0, 300.0)),
        );
        egui::Window::new("Preferences").id(egui::Id::new("app-preferences")).open(&mut open).fixed_size(size).resizable(false).collapsible(false).show(ctx,|ui|{
            let enter=crate::dialog::primary(ui);
            let footer=72.0;let height=(ui.available_height()-footer).max(140.0);
            ui.horizontal_top(|ui|{
                ui.allocate_ui_with_layout(egui::vec2(150.0,height),egui::Layout::top_down(egui::Align::Min),|ui|{
                    ui.set_min_height(height);
                    for section in Section::ALL {if ui.selectable_label(self.section==section,section.label()).clicked(){self.section=section;}}
                });ui.separator();
                let width=ui.available_width();
                egui::ScrollArea::both().id_salt(("preferences-content",self.section.label())).auto_shrink([false,false]).min_scrolled_width(0.0).max_height(height).max_width(width).show(ui,|ui|{
                    ui.set_width(width);ui.heading(self.section.label());ui.add_space(10.0);
                    match self.section {
                        Section::General=>{
                            ui.checkbox(&mut self.draft.start_maximized,"Start with the application window maximized");
                            ui.checkbox(&mut self.draft.restore_tabs,"Reopen saved connection tabs on startup");
                            ui.small("Tabs reopen disconnected. Passwords are never stored in this preference.");
                            ui.checkbox(&mut self.draft.confirm_quit,"Confirm quitting while connections are active");
                            ui.small("Unsaved editor work is always protected. Startup options take effect at the next launch.");
                        }
                        Section::Display=>self.display(ui),
                        Section::Connections=>{
                            egui::Grid::new("connection-preferences").spacing([18.0,12.0]).show(ui,|ui|{
                                ui.label("Default SSH port");ui.add(egui::DragValue::new(&mut self.draft.default_port).range(1..=65535));ui.end_row();
                                ui.label("Connection timeout");ui.add(egui::DragValue::new(&mut self.draft.connect_timeout).range(10..=300).suffix(" s"));ui.end_row();
                            });
                            ui.checkbox(&mut self.draft.remember_username,"Remember usernames by default for new saved connections");
                            ui.label("New defaults do not modify saved connections. Timeout changes apply to future connection attempts.");
                            ui.label("Reconnect from the connection tab when needed. Connections are not silently retried with saved credentials.");
                            ui.small("WireGuard configuration remains isolated to each connection tab.");
                        }
                        Section::Files=>{
                            ui.checkbox(&mut self.follow_cwd,"Synchronize terminal and file directories in new SSH tabs");
                            ui.small("The Files window can override synchronization for its own tab.");
                            ui.checkbox(&mut self.draft.system_editor,"Open files with the Windows default application by default");
                            ui.label("When disabled, supported text files use the built-in editor. Explicit Open with and Open with text editor actions take priority.");
                            ui.small("Deletion and replacement keep their existing confirmation dialogs. Terminal font and syntax colours remain in Terminal settings.");
                        }
                        Section::Assistant=>{
                            ui.label("Manage services, models, workspace, message limits and tool permissions in the assistant's own preferences.");
                            if ui.button("Open AI Assistant preferences").clicked(){action=Some(Action::Assistant);}
                            ui.small("API keys stay in the protected credential store; themes and application preferences do not export them.");
                        }
                        Section::Keyboard=>{
                            ui.label("Application shortcuts use Ctrl. Choose a key and optionally add Shift.");
                            egui::Grid::new("application-shortcuts").spacing([16.0,10.0]).show(ui,|ui|{
                                for (label,binding) in self.draft.shortcuts.rows_mut(){
                                    ui.label(label);egui::ComboBox::from_id_salt(label).width(110.0).selected_text(&binding.key).show_ui(ui,|ui|{for name in KEYS{ui.selectable_value(&mut binding.key,name.to_owned(),name);}});
                                    ui.checkbox(&mut binding.shift,"Shift");ui.end_row();
                                }
                            });
                            ui.small("These apply to AetherDesk navigation. Remote desktop and terminal shortcuts keep their own input handling.");
                            if ui.button("Restore default shortcuts").clicked(){self.draft.shortcuts=Shortcuts::default();}
                        }
                        Section::Updates=>{
                            ui.label("Configure automatic checks and downloads, review releases and install verified updates.");
                            if ui.button("Open Updates").clicked(){action=Some(Action::Updates);}
                            ui.small("Installation always asks before restarting. Preview releases are excluded from stable updates.");
                        }
                        Section::Logs=>{
                            egui::Grid::new("logging-preferences").spacing([18.0,12.0]).show(ui,|ui|{
                                ui.label("Capture level");egui::ComboBox::from_id_salt("capture-level").selected_text(&self.draft.log_level).show_ui(ui,|ui|{for level in ["error","warn","info","debug","trace"]{ui.selectable_value(&mut self.draft.log_level,level.to_owned(),level);}});ui.end_row();
                                ui.label("Size per log file");ui.add(egui::DragValue::new(&mut self.draft.log_file_mb).range(1..=100).suffix(" MiB"));ui.end_row();
                                ui.label("Rotated files to retain");ui.add(egui::DragValue::new(&mut self.draft.log_generations).range(1..=20));ui.end_row();
                            });
                            ui.label("Limits apply per application instance. Capture level changes take effect after restart; AETHERDESK_LOG_LEVEL overrides the selected level.");
                            if ui.button("Open Log files").clicked(){action=Some(Action::Logs);}
                        }
                    }
                });
            });
            ui.separator();
            if !self.message.is_empty(){ui.add(egui::Label::new(&self.message).truncate()).on_hover_text(&self.message);}
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                cancel=ui.button("Cancel").clicked();
                if ui.button("Reset to defaults").clicked(){self.draft=Settings::default();self.follow_cwd=true;self.hex.clear();}
                let response=ui.add_enabled(self.valid,egui::Button::new("Apply and save"));
                crate::dialog::focus_on_open(ui,&response);
                apply=self.valid && (response.clicked() || enter);
            });
        });
        if apply {
            if self
                .hex
                .values()
                .any(|text| theme_palette::parse_hex(text).is_none())
            {
                self.message = "Correct the invalid hexadecimal colour before applying.".into();
                return action;
            }
            self.draft.follow_cwd = Some(self.follow_cwd);
            let result = self.save(self.draft.clone());
            match result {
                Ok(()) => {
                    self.open = false;
                    action = Some(Action::Applied);
                    self.message.clear();
                }
                Err(error) => self.message = error,
            }
        } else if cancel || !open {
            self.open = false;
            self.draft = self.settings.clone();
            self.hex.clear();
        }
        action
    }
    fn display(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Theme mode");
            for (mode, label) in [
                (Mode::System, "System"),
                (Mode::Light, "Light"),
                (Mode::Dark, "Dark"),
            ] {
                ui.selectable_value(&mut self.draft.mode, mode, label);
            }
        });
        ui.add(
            egui::Slider::new(&mut self.draft.ui_scale, 0.8..=1.6)
                .text("Application scale")
                .show_value(true),
        );
        ui.add(
            egui::Slider::new(&mut self.draft.font_size, 11.0..=20.0).text("Application font size"),
        );
        ui.small("Application colours and scaling do not alter remote monitor resolution, remote pixels or terminal syntax colours.");
        ui.separator();
        ui.horizontal(|ui| {
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
        let mut group = "";
        for (section, label, rgb) in palette.roles_mut() {
            if section != group {
                ui.add_space(8.0);
                ui.strong(section);
                group = section;
            }
            ui.push_id((self.dark_tab, label), |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([190.0, 24.0], egui::Label::new(label));
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
                });
            });
        }
        ui.separator();
        ui.label("Theme file (.json)");
        ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(ui.available_width()));
        ui.horizontal(|ui| {
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
    let width = ui.available_width().clamp(220.0, 620.0);
    let height = width * 0.48;
    let (r, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let p = ui.painter();
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
        crate::theme::icon(p, rect(0.014, y, 0.030, 0.055), 1, c.icon);
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
#[cfg(test)]
mod tests {
    use super::*;
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
