//! Solid bronze/black and cream/gold native themes. No background textures.
use ad_desktop_model::profiles::ConnectionType;
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

pub fn native_icon() -> std::sync::Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let image = image::load_from_memory_with_format(
            include_bytes!("../assets/theme/aetherdesk.png"),
            image::ImageFormat::Png,
        )
        .expect("embedded product icon")
        .into_rgba8();
        std::sync::Arc::new(egui::IconData {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    })
    .clone()
}
pub fn font_definitions() -> egui::FontDefinitions {
    static FONTS: std::sync::OnceLock<egui::FontDefinitions> = std::sync::OnceLock::new();
    FONTS.get_or_init(build_font_definitions).clone()
}
fn build_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(root) = std::env::var_os("SystemRoot") {
        if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(root).join("Fonts/segoeui.ttf")) {
            if bytes.len() < 20 * 1024 * 1024 {
                fonts.font_data.insert(
                    "windows-ui".into(),
                    egui::FontData::from_owned(bytes)
                        .tweak(egui::FontTweak {
                            y_offset: -1.0,
                            ..Default::default()
                        })
                        .into(),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "windows-ui".into());
            }
        }
    }
    let mut heading = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some(root) = std::env::var_os("SystemRoot") {
        if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(root).join("Fonts/seguisb.ttf")) {
            if bytes.len() < 20 * 1024 * 1024 {
                fonts.font_data.insert(
                    "windows-heading".into(),
                    egui::FontData::from_owned(bytes)
                        .tweak(egui::FontTweak {
                            y_offset: -1.0,
                            ..Default::default()
                        })
                        .into(),
                );
                heading.insert(0, "windows-heading".into());
            }
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("ui-heading".into()), heading);
    fonts
}
pub fn heading_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("ui-heading".into()))
}
pub use crate::theme_palette::colors;
fn visuals(dark: bool) -> egui::Visuals {
    let c = colors(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = c.panel;
    v.window_fill = c.popup;
    v.window_stroke = Stroke::new(1.0_f32, c.border);
    v.warn_fg_color = c.warning;
    v.error_fg_color = c.error;
    v.extreme_bg_color = c.input;
    v.faint_bg_color = c.canvas;
    v.override_text_color = None;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, c.text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(0.8_f32, c.separator);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, c.button_text);
    v.widgets.inactive.bg_fill = c.button;
    v.widgets.inactive.weak_bg_fill = c.button;
    v.widgets.inactive.bg_stroke = Stroke::new(0.8_f32, c.border.gamma_multiply(0.7));
    v.widgets.hovered.bg_fill = c.hover;
    v.widgets.hovered.weak_bg_fill = c.hover;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, c.hover_text);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, c.border);
    v.widgets.active.bg_fill = c.active;
    v.widgets.active.weak_bg_fill = c.active;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, c.active_text);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, c.border);
    v.widgets.open.bg_fill = c.titlebar;
    v.widgets.open.weak_bg_fill = c.titlebar;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, c.selection_text);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, c.border);
    v.selection.bg_fill = c.selection;
    v.selection.stroke = Stroke::new(1.0_f32, c.selection_text);
    v.hyperlink_color = c.information;
    v
}
pub fn apply(ctx: &egui::Context, dark: bool) {
    // set_visuals() only changes whichever egui style is currently selected.
    // Install both palettes before selecting the saved application mode so a
    // previous/system theme can never redirect the saved colours into the other slot.
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.set_theme(match crate::preferences::current().mode {
        crate::preferences::Mode::System => egui::ThemePreference::System,
        crate::preferences::Mode::Dark => egui::ThemePreference::Dark,
        crate::preferences::Mode::Light => egui::ThemePreference::Light,
    });
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    }));
    let font_size = crate::preferences::current().font_size;
    ctx.all_styles_mut(|style| {
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(font_size));
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::proportional(font_size),
        );
    });
}
#[derive(Default)]
pub struct Theme;
impl Theme {
    pub fn initialize(&mut self, _ctx: &egui::Context) {}
    pub fn toolbar(&self, ui: &egui::Ui, dark: bool) {
        let r = ui.max_rect().expand2(egui::vec2(8.0, 2.0));
        ui.painter().rect_filled(r, 0, colors(dark).toolbar);
        ui.painter().hline(
            r.x_range(),
            r.bottom(),
            Stroke::new(1.0_f32, colors(dark).border),
        );
    }
    pub fn sidebar(&self, ui: &egui::Ui, dark: bool) {
        ui.painter().rect_filled(
            ui.max_rect().expand2(egui::vec2(8.0, 2.0)),
            0,
            colors(dark).sidebar,
        );
    }
    pub fn rail(&self, ui: &egui::Ui, dark: bool) {
        ui.painter().rect_filled(
            ui.max_rect().expand2(egui::vec2(5.0, 2.0)),
            0,
            colors(dark).rail,
        );
    }
    pub fn logo(&self, ui: &egui::Ui, dark: bool, rect: Rect) {
        icon(ui.painter(), rect.shrink(3.0), 8, colors(dark).heading);
    }
    fn card_border(&self, p: &egui::Painter, dark: bool, rect: Rect) {
        p.rect_stroke(
            rect,
            5,
            Stroke::new(1.0_f32, colors(dark).border),
            egui::StrokeKind::Inside,
        );
    }
    pub fn home(&self, ui: &mut egui::Ui, dark: bool) -> Option<ConnectionType> {
        let c = colors(dark);
        ui.painter()
            .rect_filled(ui.available_rect_before_wrap(), 0, c.canvas);
        let mut chosen = None;
        egui::ScrollArea::vertical().id_salt("welcome-scroll").auto_shrink([false, false]).show(ui, |ui| {
            let margin = (ui.available_width() * 0.045).clamp(12.0, 46.0);
            egui::Frame::new().inner_margin(egui::Margin::symmetric(margin as i8, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let font = egui::TextStyle::Body.resolve(ui.style()).size;
            egui::Frame::new().fill(c.panel).inner_margin(12.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    icon(ui.painter(), rect, 8, c.muted);
                    ui.label(egui::RichText::new("SECURE REMOTE WORKSPACE").color(c.muted));
                });
            });
            ui.add_space(12.0);
            let heading_size = (font * 2.25).min((ui.available_width() / 8.0).max(font));
            ui.add(egui::Label::new(egui::RichText::new("Everything remote.\nOne focused workspace.").font(heading_font(heading_size)).color(c.heading)).wrap());
            ui.add_space(8.0);
            ui.add(egui::Label::new("Open every connection in its own independent tab, keep credentials protected by Windows, and move between terminal, files, applications, and desktops without losing context.").wrap());
            ui.add_space(12.0);
            // A wrapped label inside a wrapping horizontal row can paint back
            // over the preceding button. Choose the row direction before
            // placing either widget, using their full unwrapped text widths.
            let shortcut = format!("Shortcut: {}", crate::preferences::current().shortcuts.new_connection.label());
            let button_width = ui.painter().layout_no_wrap("+  New Connection".into(), egui::TextStyle::Button.resolve(ui.style()), c.text).size().x
                + ui.spacing().button_padding.x * 2.0;
            let shortcut_width = ui.painter().layout_no_wrap(shortcut.clone(), egui::TextStyle::Body.resolve(ui.style()), c.muted).size().x;
            let inline = button_width + shortcut_width + ui.spacing().item_spacing.x + 4.0 <= ui.available_width();
            let mut actions = |ui: &mut egui::Ui| {
                if ui.add(egui::Button::new("+  New Connection").corner_radius(18)).clicked() {
                    chosen = Some(ConnectionType::SshTerminal);
                }
                ui.add(egui::Label::new(egui::RichText::new(&shortcut).color(c.muted)).wrap());
            };
            // horizontal/vertical size to their contents. with_layout would
            // inherit the scroll area's remaining height and centre a huge row.
            if inline {
                ui.horizontal(&mut actions);
            } else {
                ui.vertical(&mut actions);
            }
            ui.add_space(16.0);
            let cards = [
                (ConnectionType::SshTerminal, "SSH Terminal", "Fast interactive shells with verified host keys.", 0),
                (ConnectionType::SftpOnly, "SFTP Workspace", "Local and remote files beside every live session.", 1),
                (ConnectionType::RemoteDesktop, "Remote Desktop", "Linux desktops streamed inside AetherDesk.", 2),
                (ConnectionType::SshX11, "SSH + X11", "Launch individual Linux GUI applications securely.", 3),
            ];
            let columns = if ui.available_width() >= font * 36.0 { 2 } else { 1 };
            for row in cards.chunks(columns) {
                ui.columns(columns, |columns| {
                    for (column, &(kind, title, description, index)) in columns.iter_mut().zip(row) {
                        if self.home_card(column, dark, title, description, index).clicked() {
                            chosen = Some(kind);
                        }
                    }
                });
                ui.add_space(12.0);
            }
            });
        });
        chosen
    }
    fn home_card(
        &self,
        ui: &mut egui::Ui,
        dark: bool,
        title: &str,
        description: &str,
        index: usize,
    ) -> egui::Response {
        let c = colors(dark);
        let width = ui.available_width().max(1.0);
        let font = egui::TextStyle::Body.resolve(ui.style()).size;
        let padding = 16.0;
        let text_x = if width >= 220.0 { 48.0 } else { padding };
        let ready_width = if width > font * 28.0 { 60.0 } else { 0.0 };
        let title_galley = ui.painter().layout(
            title.into(),
            egui::FontId::proportional(font + 1.0),
            Color32::PLACEHOLDER,
            (width - text_x - padding - ready_width).max(1.0),
        );
        let description_galley = ui.painter().layout(
            description.into(),
            egui::FontId::proportional(font),
            Color32::PLACEHOLDER,
            (width - text_x - padding).max(1.0),
        );
        let height = padding * 2.0 + title_galley.size().y + 8.0 + description_galley.size().y;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title)
        });
        let foreground = if response.hovered() {
            c.selection_text
        } else {
            c.text
        };
        ui.painter().rect_filled(
            rect,
            5,
            if response.hovered() {
                c.selection
            } else {
                c.panel
            },
        );
        self.card_border(ui.painter(), dark, rect);
        if width >= 220.0 {
            icon(
                ui.painter(),
                Rect::from_min_size(
                    rect.min + egui::vec2(padding, padding),
                    egui::vec2(18.0, 18.0),
                ),
                index,
                foreground,
            );
        }
        ui.painter().galley(
            rect.min + egui::vec2(text_x, padding),
            title_galley.clone(),
            foreground,
        );
        ui.painter().galley(
            rect.min + egui::vec2(text_x, padding + title_galley.size().y + 8.0),
            description_galley,
            foreground,
        );
        if ready_width > 0.0 {
            let dot = egui::pos2(
                rect.right() - 52.0,
                rect.top() + padding + title_galley.size().y * 0.5,
            );
            ui.painter().circle_filled(dot, 3.0, c.information);
            ui.painter().text(
                dot + egui::vec2(7.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "Ready",
                egui::FontId::proportional(font * 0.75),
                foreground,
            );
        }
        response
    }
}
pub fn mode_toggle(ui: &mut egui::Ui, dark: bool) -> bool {
    let (r, response) = ui.allocate_exact_size(egui::vec2(65.0, 28.0), egui::Sense::click());
    let c = colors(dark);
    let centre = r.left_center() + egui::vec2(9.0, 0.0);
    if dark {
        ui.painter()
            .circle_stroke(centre, 5.0, Stroke::new(1.0_f32, c.text));
        ui.painter()
            .circle_filled(centre + egui::vec2(3.0, -2.5), 4.5, c.panel);
    } else {
        ui.painter()
            .circle_stroke(centre, 3.0, Stroke::new(1.0_f32, c.text));
        for i in 0..8 {
            let axis = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.0);
            ui.painter().line_segment(
                [centre + axis * 5.0, centre + axis * 7.0],
                Stroke::new(1.0_f32, c.text),
            );
        }
    }
    let pill = Rect::from_center_size(r.center() + egui::vec2(10.0, 0.0), egui::vec2(36.0, 16.0));
    ui.painter()
        .rect_filled(pill, 8, if dark { c.selection } else { c.panel });
    ui.painter().rect_stroke(
        pill,
        8,
        Stroke::new(0.7_f32, c.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().circle_filled(
        egui::pos2(
            if dark {
                pill.right() - 8.0
            } else {
                pill.left() + 8.0
            },
            pill.center().y,
        ),
        5.5,
        c.heading,
    );
    response
        .on_hover_text(if dark {
            "Switch to light mode"
        } else {
            "Switch to dark mode"
        })
        .clicked()
}
pub fn sidebar_icon(sidebar: ad_desktop_model::state::Sidebar) -> usize {
    use ad_desktop_model::state::Sidebar;
    match sidebar {
        Sidebar::Sessions => 1,
        Sidebar::Bookmarks => 9,
        Sidebar::Macros => 10,
        Sidebar::Tools => 11,
        Sidebar::Connections => 3,
        Sidebar::Monitoring => 2,
        Sidebar::Logs => 6,
        Sidebar::Assistant => 7,
    }
}
pub fn rail_button(
    ui: &mut egui::Ui,
    index: usize,
    label: &str,
    selected: bool,
    dark: bool,
) -> egui::Response {
    let (r, response) = ui.allocate_exact_size(egui::vec2(34.0, 36.0), egui::Sense::click());
    let c = colors(dark);
    if selected || response.hovered() {
        ui.painter().rect_filled(r, 3, c.selection);
    }
    if selected {
        ui.painter()
            .vline(r.left(), r.y_range(), Stroke::new(3.0_f32, c.border));
    }
    icon(
        ui.painter(),
        r.shrink2(egui::vec2(9.0, 10.0)),
        index,
        if selected || response.hovered() {
            c.selection_text
        } else {
            c.icon
        },
    );
    response.on_hover_text(label)
}
pub fn icon(p: &egui::Painter, rect: Rect, index: usize, color: Color32) {
    let stroke = Stroke::new(1.1_f32, color);
    let point = |x: f32, y: f32| rect.min + rect.size() * egui::vec2(x, y);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([point(a.0, a.1), point(b.0, b.1)], stroke);
    };
    match index {
        11 => {
            // An open wrench head and diagonal handle, distinct from AI sparkle.
            p.add(egui::Shape::line(
                vec![
                    point(0.8, 0.05),
                    point(0.58, 0.28),
                    point(0.72, 0.42),
                    point(0.95, 0.2),
                ],
                stroke,
            ));
            p.add(egui::Shape::line(
                vec![
                    point(0.95, 0.2),
                    point(0.96, 0.43),
                    point(0.76, 0.58),
                    point(0.58, 0.55),
                    point(0.21, 0.95),
                    point(0.04, 0.78),
                    point(0.43, 0.4),
                    point(0.4, 0.22),
                    point(0.56, 0.03),
                    point(0.8, 0.05),
                ],
                stroke,
            ));
        }
        0 | 5 => {
            p.rect_stroke(rect, 1, stroke, egui::StrokeKind::Inside);
            line((0.2, 0.3), (0.4, 0.5));
            line((0.4, 0.5), (0.2, 0.7));
            line((0.5, 0.7), (0.8, 0.7));
        }
        1 => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(0.0, 0.2),
                    point(0.35, 0.2),
                    point(0.45, 0.35),
                    point(1.0, 0.35),
                    point(1.0, 1.0),
                    point(0.0, 1.0),
                ],
                stroke,
            ));
        }
        2 => {
            p.rect_stroke(
                Rect::from_min_max(point(0.0, 0.0), point(1.0, 0.75)),
                1,
                stroke,
                egui::StrokeKind::Inside,
            );
            line((0.5, 0.75), (0.5, 1.0));
            line((0.25, 1.0), (0.75, 1.0));
        }
        3 => {
            p.rect_stroke(
                Rect::from_min_max(point(0.0, 0.2), point(0.8, 1.0)),
                1,
                stroke,
                egui::StrokeKind::Inside,
            );
            line((0.35, 0.65), (1.0, 0.0));
            line((0.6, 0.0), (1.0, 0.0));
            line((1.0, 0.0), (1.0, 0.4));
        }
        4 => {
            for y in [0.2, 0.5, 0.8] {
                line((0.0, y), (1.0, y));
            }
        }
        6 => {
            p.rect_stroke(rect, 1, stroke, egui::StrokeKind::Inside);
            for y in [0.3, 0.5, 0.7] {
                line((0.2, y), (0.8, y));
            }
        }
        7 => {
            for angle in [0.0_f32, 1.047, 2.094] {
                let axis = Vec2::angled(angle) * rect.width() * 0.5;
                p.line_segment([rect.center() - axis, rect.center() + axis], stroke);
            }
            p.circle_filled(rect.center(), 3.0, color);
        }
        8 => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(0.5, 0.0),
                    point(0.9, 0.2),
                    point(0.85, 0.65),
                    point(0.5, 1.0),
                    point(0.15, 0.65),
                    point(0.1, 0.2),
                ],
                stroke,
            ));
        }
        9 => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(0.2, 0.0),
                    point(0.8, 0.0),
                    point(0.8, 1.0),
                    point(0.5, 0.75),
                    point(0.2, 1.0),
                ],
                stroke,
            ));
        }
        10 => {
            p.add(egui::Shape::closed_line(
                vec![
                    point(0.55, 0.0),
                    point(0.15, 0.6),
                    point(0.5, 0.55),
                    point(0.4, 1.0),
                    point(0.9, 0.35),
                    point(0.55, 0.4),
                ],
                stroke,
            ));
        }
        _ => {
            p.circle_stroke(rect.center(), rect.width() * 0.3, stroke);
            for i in 0..8 {
                let axis = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.0);
                p.line_segment(
                    [
                        rect.center() + axis * rect.width() * 0.3,
                        rect.center() + axis * rect.width() * 0.5,
                    ],
                    stroke,
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn welcome_actions_and_all_cards_fit_normal_desktop_viewports_without_scrolling() {
        for dark in [false, true] {
            for (width, height, font) in [
                (767.0, 600.0, 13.0),
                (1120.0, 760.0, 20.0),
                (1600.0, 900.0, 13.0),
            ] {
                let ctx = egui::Context::default();
                ctx.set_fonts(font_definitions());
                let mut output = None;
                for _ in 0..4 {
                    output = Some(ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            apply(ctx, dark);
                            ctx.style_mut(|style| {
                                for role in [egui::TextStyle::Body, egui::TextStyle::Button] {
                                    style
                                        .text_styles
                                        .insert(role, egui::FontId::proportional(font));
                                }
                            });
                            egui::CentralPanel::default().show(ctx, |ui| {
                                Theme.home(ui, dark);
                            });
                        },
                    ));
                }
                let texts: Vec<_> = output
                    .unwrap()
                    .shapes
                    .into_iter()
                    .filter_map(|shape| {
                        if let egui::epaint::Shape::Text(text) = shape.shape {
                            Some((
                                text.galley.job.text.clone(),
                                Rect::from_min_size(text.pos, text.galley.size()),
                                shape.clip_rect,
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                for title in [
                    "SSH Terminal",
                    "SFTP Workspace",
                    "Remote Desktop",
                    "SSH + X11",
                ] {
                    let (_, rect, clip) = texts.iter().find(|(text, ..)| text == title).unwrap();
                    assert!(
                        clip.contains_rect(*rect),
                        "{title} requires scrolling at {width}x{height}"
                    );
                }
                let (_, introduction, _) = texts
                    .iter()
                    .find(|(text, ..)| text.starts_with("Open every connection"))
                    .unwrap();
                let (_, button, _) = texts
                    .iter()
                    .find(|(text, ..)| text == "+  New Connection")
                    .unwrap();
                assert!(
                    button.top() - introduction.bottom() < 48.0,
                    "action row consumed the remaining window height"
                );
                // Include descriptions, not just card headings, in the fit check.
                for (text, rect, clip) in &texts {
                    assert!(
                        clip.contains_rect(*rect),
                        "Welcome content clipped: {text} at {width}x{height}"
                    );
                }
            }
        }
    }
    #[test]
    fn welcome_text_reflows_without_overlap_in_narrow_windows_and_large_fonts() {
        for dark in [false, true] {
            for font in [13.0, 20.0] {
                for width in [280.0, 420.0, 640.0, 1000.0] {
                    let ctx = egui::Context::default();
                    ctx.set_fonts(font_definitions());
                    let mut output = None;
                    for _ in 0..3 {
                        output = Some(ctx.run(
                            egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 3000.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                apply(ctx, dark);
                                ctx.style_mut(|s| {
                                    s.text_styles.insert(
                                        egui::TextStyle::Body,
                                        egui::FontId::proportional(font),
                                    );
                                    s.text_styles.insert(
                                        egui::TextStyle::Button,
                                        egui::FontId::proportional(font),
                                    );
                                });
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    Theme.home(ui, dark);
                                });
                            },
                        ));
                    }
                    let texts: Vec<_> = output
                        .unwrap()
                        .shapes
                        .into_iter()
                        .filter_map(|shape| {
                            if let egui::epaint::Shape::Text(text) = shape.shape {
                                Some((
                                    text.galley.job.text.clone(),
                                    Rect::from_min_size(text.pos, text.galley.size()),
                                    shape.clip_rect,
                                ))
                            } else {
                                None
                            }
                        })
                        .collect();
                    for (index, (text, rect, clip)) in texts.iter().enumerate() {
                        assert!(rect.left() >= clip.left() - 1.0 && rect.right() <= clip.right() + 1.0,
                            "Welcome text overflows at width {width}, font {font}: {text}: {rect:?} / {clip:?}");
                        for (other, other_rect, _) in &texts[index + 1..] {
                            assert!(!rect.shrink(0.5).intersects(other_rect.shrink(0.5)),
                                "Welcome text overlaps at width {width}, font {font}: {text} / {other}");
                        }
                    }
                    for name in [
                        "SSH Terminal",
                        "SFTP Workspace",
                        "Remote Desktop",
                        "SSH + X11",
                    ] {
                        assert!(texts.iter().any(|(text, ..)| text == name));
                    }
                }
            }
        }
    }
    #[test]
    fn solid_themes_keep_welcome_controls_and_product_icon() {
        assert_ne!(
            sidebar_icon(ad_desktop_model::state::Sidebar::Tools),
            sidebar_icon(ad_desktop_model::state::Sidebar::Assistant)
        );
        let ctx = egui::Context::default();
        ctx.set_fonts(font_definitions());
        let theme = Theme;
        for dark in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(767.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    apply(ctx, dark);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        theme.home(ui, dark);
                    });
                },
            );
        }
        let icon = native_icon();
        assert_eq!(
            icon.rgba.len(),
            icon.width as usize * icon.height as usize * 4
        );
    }
    #[test]
    fn selection_has_visible_foreground_in_both_modes() {
        fn luminance(c: Color32) -> f32 {
            let channels = [c.r(), c.g(), c.b()].map(|v| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            });
            channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
        }
        for dark in [true, false] {
            let c = colors(dark);
            let a = luminance(c.selection);
            let b = luminance(c.selection_text);
            assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
        }
    }
}
