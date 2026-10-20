//! User-editable application colours. Remote pixels and terminal syntax are separate.
use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    pub panel: [u8; 3],
    pub canvas: [u8; 3],
    pub toolbar: [u8; 3],
    pub sidebar: [u8; 3],
    pub rail: [u8; 3],
    pub popup: [u8; 3],
    pub input: [u8; 3],
    pub titlebar: [u8; 3],
    pub tab: [u8; 3],
    pub tab_text: [u8; 3],
    pub tab_active: [u8; 3],
    pub tab_active_text: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub heading: [u8; 3],
    pub icon: [u8; 3],
    pub border: [u8; 3],
    pub separator: [u8; 3],
    pub button: [u8; 3],
    pub button_text: [u8; 3],
    pub hover: [u8; 3],
    pub hover_text: [u8; 3],
    pub active: [u8; 3],
    pub active_text: [u8; 3],
    pub selection: [u8; 3],
    pub selection_text: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub error: [u8; 3],
    pub information: [u8; 3],
}
#[derive(Clone, Copy)]
pub struct Colors {
    pub panel: Color32,
    pub canvas: Color32,
    pub toolbar: Color32,
    pub sidebar: Color32,
    pub rail: Color32,
    pub popup: Color32,
    pub input: Color32,
    pub titlebar: Color32,
    pub tab: Color32,
    pub tab_text: Color32,
    pub tab_active: Color32,
    pub tab_active_text: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub heading: Color32,
    pub icon: Color32,
    pub border: Color32,
    pub separator: Color32,
    pub button: Color32,
    pub button_text: Color32,
    pub hover: Color32,
    pub hover_text: Color32,
    pub active: Color32,
    pub active_text: Color32,
    pub selection: Color32,
    pub selection_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub information: Color32,
}
impl Palette {
    pub fn defaults(dark: bool) -> Self {
        if dark {
            Self {
                panel: [19, 20, 19],
                canvas: [2, 17, 24],
                toolbar: [19, 20, 19],
                sidebar: [19, 20, 19],
                rail: [19, 20, 19],
                popup: [19, 20, 19],
                input: [9, 12, 12],
                titlebar: [99, 80, 55],
                tab: [19, 20, 19],
                tab_text: [222, 213, 195],
                tab_active: [99, 80, 55],
                tab_active_text: [255, 244, 222],
                text: [222, 213, 195],
                muted: [179, 168, 146],
                heading: [201, 186, 155],
                icon: [222, 213, 195],
                border: [136, 116, 82],
                separator: [85, 74, 55],
                button: [19, 20, 19],
                button_text: [222, 213, 195],
                hover: [99, 80, 55],
                hover_text: [255, 244, 222],
                active: [119, 96, 68],
                active_text: [255, 244, 222],
                selection: [99, 80, 55],
                selection_text: [255, 244, 222],
                success: [54, 168, 99],
                warning: [230, 182, 90],
                error: [240, 138, 135],
                information: [124, 168, 206],
            }
        } else {
            Self {
                panel: [244, 237, 220],
                canvas: [247, 245, 231],
                toolbar: [244, 237, 220],
                sidebar: [244, 237, 220],
                rail: [244, 237, 220],
                popup: [244, 237, 220],
                input: [255, 250, 238],
                titlebar: [194, 169, 122],
                tab: [244, 237, 220],
                tab_text: [48, 42, 30],
                tab_active: [194, 169, 122],
                tab_active_text: [40, 31, 16],
                text: [48, 42, 30],
                muted: [105, 89, 60],
                heading: [111, 81, 32],
                icon: [48, 42, 30],
                border: [154, 129, 83],
                separator: [214, 199, 170],
                button: [244, 237, 220],
                button_text: [48, 42, 30],
                hover: [194, 169, 122],
                hover_text: [40, 31, 16],
                active: [174, 145, 95],
                active_text: [40, 31, 16],
                selection: [194, 169, 122],
                selection_text: [40, 31, 16],
                success: [24, 115, 57],
                warning: [133, 90, 13],
                error: [176, 45, 50],
                information: [50, 106, 151],
            }
        }
    }
    pub fn colors(self) -> Colors {
        let c = |v: [u8; 3]| Color32::from_rgb(v[0], v[1], v[2]);
        Colors {
            panel: c(self.panel),
            canvas: c(self.canvas),
            toolbar: c(self.toolbar),
            sidebar: c(self.sidebar),
            rail: c(self.rail),
            popup: c(self.popup),
            input: c(self.input),
            titlebar: c(self.titlebar),
            tab: c(self.tab),
            tab_text: c(self.tab_text),
            tab_active: c(self.tab_active),
            tab_active_text: c(self.tab_active_text),
            text: c(self.text),
            muted: c(self.muted),
            heading: c(self.heading),
            icon: c(self.icon),
            border: c(self.border),
            separator: c(self.separator),
            button: c(self.button),
            button_text: c(self.button_text),
            hover: c(self.hover),
            hover_text: c(self.hover_text),
            active: c(self.active),
            active_text: c(self.active_text),
            selection: c(self.selection),
            selection_text: c(self.selection_text),
            success: c(self.success),
            warning: c(self.warning),
            error: c(self.error),
            information: c(self.information),
        }
    }
    pub fn roles_mut(&mut self) -> Vec<(&'static str, &'static str, &mut [u8; 3])> {
        vec![
            ("Surfaces", "General panels", &mut self.panel),
            ("Surfaces", "Main workspace", &mut self.canvas),
            ("Surfaces", "Top toolbar", &mut self.toolbar),
            ("Surfaces", "Sidebar", &mut self.sidebar),
            ("Surfaces", "Icon rail", &mut self.rail),
            ("Surfaces", "Dialogs and menus", &mut self.popup),
            ("Surfaces", "Input fields", &mut self.input),
            ("Surfaces", "Dialog title bar", &mut self.titlebar),
            ("Tabs", "Inactive tab background", &mut self.tab),
            ("Tabs", "Inactive tab text", &mut self.tab_text),
            ("Tabs", "Active tab background", &mut self.tab_active),
            ("Tabs", "Active tab text", &mut self.tab_active_text),
            ("Text and icons", "Main text", &mut self.text),
            ("Text and icons", "Secondary text", &mut self.muted),
            ("Text and icons", "Headings and links", &mut self.heading),
            ("Text and icons", "Icons", &mut self.icon),
            ("Lines", "Borders", &mut self.border),
            ("Lines", "Separators", &mut self.separator),
            ("Controls", "Button background", &mut self.button),
            ("Controls", "Button text", &mut self.button_text),
            ("Controls", "Hover background", &mut self.hover),
            ("Controls", "Hover text", &mut self.hover_text),
            ("Controls", "Pressed control", &mut self.active),
            ("Controls", "Pressed control text", &mut self.active_text),
            ("Controls", "Selection background", &mut self.selection),
            ("Controls", "Selection text", &mut self.selection_text),
            ("Status", "Success", &mut self.success),
            ("Status", "Warning", &mut self.warning),
            ("Status", "Error", &mut self.error),
            ("Status", "Information", &mut self.information),
        ]
    }
    pub fn contrast_warnings(&self) -> Vec<&'static str> {
        [
            ("Main text", self.text, self.panel),
            ("Sidebar text", self.text, self.sidebar),
            ("Dialog text", self.text, self.popup),
            ("Input text", self.text, self.input),
            ("Button text", self.button_text, self.button),
            ("Hover text", self.hover_text, self.hover),
            ("Selection text", self.selection_text, self.selection),
            ("Active tab text", self.tab_active_text, self.tab_active),
        ]
        .into_iter()
        .filter_map(|(label, a, b)| (contrast(a, b) < 4.5).then_some(label))
        .collect()
    }
}
fn luminance(c: [u8; 3]) -> f32 {
    let channel = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
}
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&t[..2], 16).ok()?,
        u8::from_str_radix(&t[2..4], 16).ok()?,
        u8::from_str_radix(&t[4..], 16).ok()?,
    ])
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Themes {
    pub dark: Palette,
    pub light: Palette,
}
impl Default for Themes {
    fn default() -> Self {
        Self {
            dark: Palette::defaults(true),
            light: Palette::defaults(false),
        }
    }
}
#[cfg(not(test))]
fn store() -> &'static std::sync::RwLock<Themes> {
    static STORE: std::sync::OnceLock<std::sync::RwLock<Themes>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::RwLock::new(Themes::default()))
}
#[cfg(test)]
thread_local! {static TEST_THEMES:std::cell::RefCell<Themes>=std::cell::RefCell::new(Themes::default());}
pub fn install(themes: Themes) {
    #[cfg(not(test))]
    if let Ok(mut value) = store().write() {
        *value = themes;
    }
    #[cfg(test)]
    TEST_THEMES.with(|value| *value.borrow_mut() = themes);
}
pub fn colors(dark: bool) -> Colors {
    #[cfg(not(test))]
    let themes = store().read().map(|v| *v).unwrap_or_default();
    #[cfg(test)]
    let themes = TEST_THEMES.with(|v| *v.borrow());
    if dark {
        themes.dark.colors()
    } else {
        themes.light.colors()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn light_and_dark_theme_roles_round_trip_without_cross_contamination() {
        let mut themes = Themes::default();
        let light = themes.light;
        for (_, _, color) in themes.dark.roles_mut() {
            *color = [12, 34, 56];
        }
        let loaded: Themes = serde_json::from_slice(&serde_json::to_vec(&themes).unwrap()).unwrap();
        assert_eq!(loaded.light, light);
        assert_eq!(loaded.dark, themes.dark);
        assert_eq!(loaded.dark.roles_mut_for_test(), 30);
        assert_eq!(parse_hex("#0C2238"), Some([12, 34, 56]));
        assert_eq!(parse_hex("#12345"), None);
    }
    #[test]
    fn contrast_warning_detects_invisible_controls_without_rejecting_user_colours() {
        let mut palette = Palette::defaults(true);
        palette.selection_text = palette.selection;
        assert!(palette.contrast_warnings().contains(&"Selection text"));
        assert!((contrast([255; 3], [0; 3]) - 21.0).abs() < 0.01);
        assert!(serde_json::to_string(&palette).is_ok());
    }
    impl Palette {
        fn roles_mut_for_test(mut self) -> usize {
            self.roles_mut().len()
        }
    }
}
