use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

pub const ROLES: [&str; 12] = [
    "comment",
    "error",
    "warning",
    "success",
    "information",
    "debug",
    "connection",
    "disconnection",
    "fileTransfer",
    "authentication",
    "prompt",
    "systemMessage",
];
pub const PRESETS: [(&str, &str); 4] = [
    ("mobaXterm", "MobaXterm"),
    ("dark", "Aether Dark"),
    ("light", "Aether Light"),
    ("colorBlind", "Color-blind friendly"),
];
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub preset: String,
    pub font: String,
    pub font_size: f32,
    pub line_height: f32,
    pub scrollback_lines: usize,
    pub auto_classify: bool,
    pub follow_cwd: bool,
    pub semantic_colors: BTreeMap<String, u32>,
    pub custom_words: Vec<WordColour>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct WordColour {
    pub word: String,
    pub color: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            preset: "mobaXterm".into(),
            font: "Bundled monospace".into(),
            font_size: 13.0,
            line_height: 1.2,
            scrollback_lines: 100_000,
            auto_classify: true,
            follow_cwd: true,
            semantic_colors: BTreeMap::new(),
            custom_words: Vec::new(),
        }
    }
}
impl Settings {
    fn normalize(&mut self) {
        if !PRESETS.iter().any(|(id, _)| *id == self.preset) {
            self.preset = "mobaXterm".into();
        }
        if !["Bundled monospace", "Cascadia Mono", "Consolas"].contains(&self.font.as_str()) {
            self.font = "Bundled monospace".into();
        }
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(10.0, 28.0)
        } else {
            13.0
        };
        self.line_height = if self.line_height.is_finite() {
            self.line_height.clamp(1.0, 2.0)
        } else {
            1.2
        };
        self.scrollback_lines = self.scrollback_lines.clamp(20_000, 250_000);
        self.semantic_colors
            .retain(|name, _| ROLES.contains(&name.as_str()));
        self.custom_words.truncate(128);
        for rule in &mut self.custom_words {
            rule.word = rule.word.trim().chars().take(100).collect();
        }
        self.custom_words.retain(|r| !r.word.is_empty());
    }
    pub fn color(&self, key: &str) -> Color32 {
        static PALETTES: OnceLock<serde_json::Value> = OnceLock::new();
        let all = PALETTES.get_or_init(|| {
            serde_json::from_str(include_str!("terminal_presets.json"))
                .expect("frozen terminal palettes")
        });
        let value = self
            .semantic_colors
            .get(key)
            .copied()
            .map(u64::from)
            .or_else(|| {
                all[&self.preset]["semanticColors"][if key == "comment" {
                    "systemMessage"
                } else {
                    key
                }]
                .as_u64()
            })
            .or_else(|| all[&self.preset][key].as_u64())
            .unwrap_or(0xffe7e7e7) as u32;
        Color32::from_rgba_unmultiplied(
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
            (value >> 24) as u8,
        )
    }
    pub fn ansi(&self, index: u8) -> Color32 {
        const NAMES: [&str; 16] = [
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "white",
            "brightBlack",
            "brightRed",
            "brightGreen",
            "brightYellow",
            "brightBlue",
            "brightMagenta",
            "brightCyan",
            "brightWhite",
        ];
        self.color(NAMES[usize::from(index.min(15))])
    }
    pub fn classify(&self, line: &str) -> Option<&'static str> {
        static RULES: OnceLock<Vec<(String, regex::Regex)>> = OnceLock::new();
        let rules = RULES.get_or_init(|| {
            let raw: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("terminal_rules.json")).expect("semantic rules");
            raw.into_iter()
                .map(|r| {
                    (
                        r["role"].as_str().unwrap().into(),
                        regex::RegexBuilder::new(r["pattern"].as_str().unwrap())
                            .case_insensitive(true)
                            .build()
                            .expect("semantic regex"),
                    )
                })
                .collect()
        });
        let role = rules
            .iter()
            .find(|(_, re)| re.is_match(line))
            .and_then(|(role, _)| ROLES.iter().copied().find(|r| *r == role.as_str()));
        static PROMPT: OnceLock<regex::Regex> = OnceLock::new();
        role.or_else(||PROMPT.get_or_init(||regex::Regex::new(r"(\[[^\]\r\n]+@[^\]\r\n]+\s+[^\]\r\n]+\][#$]\s*|@innovus\s+\d+>\s*|[A-Za-z0-9._-]+@[A-Za-z0-9._-]+:[^\r\n]*[#$]\s*)$").unwrap()).is_match(line).then_some("prompt"))
    }
}
struct Store {
    version: u64,
    settings: Settings,
    error: Option<String>,
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let result: Result<Option<Settings>, String> =
            crate::storage::read_json("terminal-settings.json").and_then(|v| {
                v.map(serde_json::from_value)
                    .transpose()
                    .map_err(|_| "Terminal settings are damaged; file preserved".into())
            });
        let error = result.as_ref().err().cloned();
        let mut settings = result.ok().flatten().unwrap_or_default();
        settings.normalize();
        Mutex::new(Store {
            version: 1,
            settings,
            error,
        })
    })
}
pub fn current() -> (u64, Settings) {
    let store = store().lock().unwrap_or_else(|e| e.into_inner());
    (store.version, store.settings.clone())
}
pub fn save(mut settings: Settings) -> Result<(), String> {
    settings.normalize();
    let mut store = store().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(error) = &store.error {
        return Err(error.clone());
    }
    crate::storage::write_json(
        "terminal-settings.json",
        &serde_json::to_value(&settings).map_err(|e| e.to_string())?,
    )?;
    store.settings = settings;
    store.version += 1;
    Ok(())
}
pub fn install_font(ctx: &egui::Context, settings: &Settings) {
    let mut fonts = crate::theme::font_definitions();
    let file = match settings.font.as_str() {
        "Consolas" => Some("consola.ttf"),
        "Cascadia Mono" => Some("CascadiaMono.ttf"),
        _ => None,
    };
    if let (Some(file), Some(root)) = (file, std::env::var_os("SystemRoot")) {
        let path = std::path::PathBuf::from(root).join("Fonts").join(file);
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() < 20 * 1024 * 1024 {
                fonts.font_data.insert(
                    "terminal-user-font".into(),
                    egui::FontData::from_owned(bytes).into(),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .insert(0, "terminal-user-font".into());
            }
        }
    }
    ctx.set_fonts(fonts);
}
fn packed(color: Color32) -> u32 {
    (u32::from(color.a()) << 24)
        | (u32::from(color.r()) << 16)
        | (u32::from(color.g()) << 8)
        | u32::from(color.b())
}
fn unpacked(value: u32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
        (value >> 24) as u8,
    )
}
/// Regexes are compiled when preferences change, never per painted cell.
pub struct Highlighter {
    custom: Vec<(regex::Regex, Color32)>,
}
impl Highlighter {
    pub fn new(settings: &Settings) -> Self {
        let custom = settings
            .custom_words
            .iter()
            .take(128)
            .filter_map(|r| {
                if r.word.is_empty() {
                    return None;
                }
                let word_char = |c: char| c.is_alphanumeric() || c == '_';
                let start = if r.word.chars().next().is_some_and(word_char) {
                    r"\b"
                } else {
                    ""
                };
                let end = if r.word.chars().last().is_some_and(word_char) {
                    r"\b"
                } else {
                    ""
                };
                regex::RegexBuilder::new(&format!("{start}{}{end}", regex::escape(&r.word)))
                    .case_insensitive(true)
                    .build()
                    .ok()
                    .map(|re| (re, unpacked(r.color)))
            })
            .collect();
        Self { custom }
    }
    pub fn spans(&self, line: &str, settings: &Settings) -> Vec<(std::ops::Range<usize>, Color32)> {
        if !settings.auto_classify {
            return vec![];
        }
        if line.starts_with('#') {
            return vec![(0..line.len(), settings.color("comment"))];
        }
        static RULES: OnceLock<Vec<(String, regex::Regex)>> = OnceLock::new();
        static WORD: OnceLock<regex::Regex> = OnceLock::new();
        let words = WORD.get_or_init(|| regex::Regex::new(r"[\p{L}\p{N}_]+").unwrap());
        let rules = RULES.get_or_init(|| {
            let mut raw: Vec<serde_json::Value> =
                serde_json::from_str(include_str!("terminal_rules.json")).unwrap();
            raw.push(serde_json::json!({"role":"prompt", "pattern":r"(\[[^\]\r\n]+@[^\]\r\n]+\s+[^\]\r\n]+\][#$]\s*|@innovus\s+\d+>\s*|[A-Za-z0-9._-]+@[A-Za-z0-9._-]+:[^\r\n]*[#$]\s*)$"}));
            raw.into_iter()
                .map(|r| {
                    (
                        r["role"].as_str().unwrap().into(),
                        regex::RegexBuilder::new(r["pattern"].as_str().unwrap())
                            .case_insensitive(true)
                            .build()
                            .unwrap(),
                    )
                })
                .collect()
        });
        let mut spans = Vec::new();
        for (re, color) in &self.custom {
            for m in re.find_iter(line) {
                spans.push((m.range(), *color));
            }
        }
        for (role, re) in rules {
            for m in re.find_iter(line) {
                for word in words.find_iter(m.as_str()) {
                    let range = (m.start() + word.start())..(m.start() + word.end());
                    if !spans
                        .iter()
                        .any(|(r, _)| r.start < range.end && range.start < r.end)
                    {
                        spans.push((range, settings.color(role)));
                    }
                }
            }
        }
        spans
    }
}
fn tracked_slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.scope(|ui| {
        let colors = crate::theme::colors(ui.visuals().dark_mode);
        ui.visuals_mut().widgets.inactive.bg_fill = colors.border;
        ui.spacing_mut().slider_rail_height = 6.0;
        ui.add(
            egui::Slider::new(value, range)
                .fixed_decimals(1)
                .trailing_fill(true),
        );
    });
}
pub fn edit(ui: &mut egui::Ui, settings: &mut Settings) {
    // Fixed columns and independently scrolling groups prevent expanding custom
    // rules or changing a font from shifting neighbouring settings.
    ui.columns(2, |columns| {
        columns[0].group(|ui| {
            ui.set_min_width(220.0);
            let mut added = false;
            ui.horizontal(|ui| {
                ui.heading("Colours");
                if ui
                    .add_enabled(settings.custom_words.len() < 128, egui::Button::new("+"))
                    .on_hover_text("Add a custom word and colour")
                    .clicked()
                {
                    settings.custom_words.push(WordColour {
                        word: String::new(),
                        color: 0xffd6ac79,
                    });
                    added = true;
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("terminal-colours")
                .max_height(410.0)
                .min_scrolled_height(410.0)
                .show(ui, |ui| {
                    ui.group(|ui| {
                        egui::ComboBox::from_id_salt("terminal-preset")
                            .width(190.0)
                            .selected_text(&settings.preset)
                            .show_ui(ui, |ui| {
                                for (id, label) in PRESETS {
                                    if ui
                                        .selectable_value(&mut settings.preset, id.into(), label)
                                        .changed()
                                    {
                                        settings.semantic_colors.clear();
                                    }
                                }
                            });
                    });
                    ui.group(|ui| {
                        ui.checkbox(&mut settings.auto_classify, "Highlight matching words");
                    });
                    for role in ROLES {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                let mut color = settings.color(role);
                                if ui.color_edit_button_srgba(&mut color).changed() {
                                    settings.semantic_colors.insert(role.into(), packed(color));
                                }
                                ui.label(if role == "comment" {
                                    "Lines starting with #"
                                } else {
                                    role
                                });
                            });
                        });
                    }
                    if ui.button("Reset preset colours").clicked() {
                        settings.semantic_colors.clear();
                    }
                    ui.separator();
                    ui.label("Custom words / phrases (ignore case)");
                    let mut remove = None;
                    for (index, rule) in settings.custom_words.iter_mut().enumerate() {
                        ui.push_id(("custom-word", index), |ui| {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    let mut color = unpacked(rule.color);
                                    if ui.color_edit_button_srgba(&mut color).changed() {
                                        rule.color = packed(color);
                                    }
                                    ui.add(
                                        egui::TextEdit::singleline(&mut rule.word)
                                            .desired_width(130.0)
                                            .char_limit(100),
                                    );
                                    if ui.small_button("×").on_hover_text("Remove word").clicked()
                                    {
                                        remove = Some(index);
                                    }
                                });
                            });
                        });
                    }
                    if let Some(index) = remove {
                        settings.custom_words.remove(index);
                    }
                    if added {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
        });
        columns[1].group(|ui| {
            ui.set_min_width(220.0);
            ui.heading("Font and behaviour");
            egui::ScrollArea::vertical()
                .id_salt("terminal-layout")
                .max_height(410.0)
                .min_scrolled_height(410.0)
                .show(ui, |ui| {
                    ui.group(|ui| {
                        ui.label("Font");
                        egui::ComboBox::from_id_salt("terminal-font")
                            .width(190.0)
                            .selected_text(&settings.font)
                            .show_ui(ui, |ui| {
                                for font in ["Bundled monospace", "Cascadia Mono", "Consolas"] {
                                    ui.selectable_value(&mut settings.font, font.into(), font);
                                }
                            });
                    });
                    ui.group(|ui| {
                        ui.label("Font size");
                        tracked_slider(ui, &mut settings.font_size, 10.0..=28.0);
                    });
                    ui.group(|ui| {
                        ui.label("Line height");
                        tracked_slider(ui, &mut settings.line_height, 1.0..=2.0);
                    });
                    ui.group(|ui| {
                        ui.label("Scrollback lines");
                        ui.add(
                            egui::DragValue::new(&mut settings.scrollback_lines)
                                .range(20_000..=250_000),
                        );
                        ui.label(
                        "Changing history size clears scrollback, preserving the visible screen.",
                    );
                    });
                    ui.group(|ui| {
                        ui.checkbox(
                            &mut settings.follow_cwd,
                            "Synchronize terminal and file folders in new SSH tabs",
                        );
                    });
                });
        });
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn highlights_only_words_with_unicode_offsets_and_comment_line_exception() {
        let mut settings = Settings::default();
        settings.custom_words.push(WordColour {
            word: "Wafer".into(),
            color: 0xff123456,
        });
        settings.custom_words.push(WordColour {
            word: "error".into(),
            color: 0xff654321,
        });
        let highlighter = Highlighter::new(&settings);
        let line = "界 café error with WAFER and wafers";
        let spans = highlighter.spans(line, &settings);
        let texts: Vec<_> = spans.iter().map(|(r, _)| &line[r.clone()]).collect();
        assert_eq!(texts, vec!["WAFER", "error"]);
        assert_eq!(spans[1].1, unpacked(0xff654321));
        let line = "normal failed output";
        let spans = highlighter.spans(line, &settings);
        assert_eq!(spans.len(), 1);
        assert_eq!(&line[spans[0].0.clone()], "failed");
        let comment = "# whole comment including normal output";
        assert_eq!(
            highlighter.spans(comment, &settings),
            vec![(0..comment.len(), settings.color("comment"))]
        );
        assert!(highlighter
            .spans("echo # ordinary text", &settings)
            .is_empty());
        settings.auto_classify = false;
        assert!(highlighter.spans(comment, &settings).is_empty());
        let serialized = serde_json::to_value(&settings).unwrap();
        let copy: Settings = serde_json::from_value(serialized).unwrap();
        assert_eq!(copy.custom_words[0].word, "Wafer");
        assert_eq!(copy.custom_words[1].color, 0xff654321);
    }
    #[test]
    fn every_original_preset_and_role_is_present() {
        for (id, _) in PRESETS {
            let s = Settings {
                preset: id.into(),
                ..Default::default()
            };
            for key in ROLES {
                assert_ne!(s.color(key).a(), 0);
            }
            assert_ne!(s.color("foreground"), s.color("background"));
        }
    }
    #[test]
    fn semantic_precedence_matches_original() {
        let s = Settings::default();
        assert_eq!(s.classify("connection failed"), Some("error"));
        assert_eq!(
            s.classify("Disconnected successfully"),
            Some("disconnection")
        );
        assert_eq!(s.classify("**WARN deprecated"), Some("warning"));
        assert_eq!(s.classify("normal text"), None);
    }
    #[test]
    fn saved_preferences_round_trip_and_reject_unbounded_values() {
        let mut s: Settings = serde_json::from_value(
            serde_json::json!({"scrollbackLines":999999999,"fontSize":99,"lineHeight":5}),
        )
        .unwrap();
        s.normalize();
        assert_eq!(s.scrollback_lines, 250000);
        assert_eq!(s.font_size, 28.0);
        let copy: Settings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(copy.scrollback_lines, s.scrollback_lines);
    }
}
