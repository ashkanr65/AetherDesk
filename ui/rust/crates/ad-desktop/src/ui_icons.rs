//! Small vector controls: no platform font or symbol fallback is required.
use eframe::egui::{self, Color32, Rect, Stroke};

#[derive(Clone, Copy)]
pub enum Icon {
    Tab,
    Monitor,
    Monitors,
    Resolution,
    SecureKeys,
    Disconnect,
    EndSession,
    TerminalSettings,
    Attach,
    Filter,
    Folder,
    File,
    Settings,
    NewChat,
    History,
    Send,
    Stop,
    Remove,
}
pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    button_scaled(ui, icon, label, selected, 1.0)
}
pub fn button_scaled(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    scale: f32,
) -> egui::Response {
    // Text-free controls must not inherit font height or button padding.
    // Those minimums distorted the dock on lower-DPI monitors.
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(28.0, 28.0) * scale, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        (3.0 * scale).round() as u8,
        if selected {
            ui.visuals().selection.bg_fill
        } else {
            visuals.bg_fill
        },
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else if !response.hovered() && !response.is_pointer_button_down_on() {
        crate::theme::colors(ui.visuals().dark_mode).icon
    } else {
        ui.style().interact(&response).fg_stroke.color
    };
    draw(
        ui.painter(),
        Rect::from_center_size(response.rect.center(), egui::vec2(18.0, 18.0) * scale),
        icon,
        color,
    );
    response.on_hover_text(label)
}
pub fn session_entry(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    color: Color32,
    fill: Color32,
    stroke: Stroke,
) -> egui::Response {
    let response = ui.add_sized(
        [ui.available_width(), 44.0],
        egui::Button::new("")
            .fill(fill)
            .stroke(stroke)
            .corner_radius(6),
    );
    let width = (response.rect.width() - 18.0).max(1.0);
    let galley = |text: &str| {
        egui::WidgetText::from(egui::RichText::new(text).color(color).size(12.0)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            width,
            egui::TextStyle::Button,
        )
    };
    let accessible_label = format!("{title} · {detail}");
    let title = galley(title);
    let detail = galley(detail);
    let title_height = title.size().y;
    let top = response.rect.center().y - (title_height + detail.size().y) / 2.0;
    let painter = ui.painter().with_clip_rect(response.rect.shrink(2.0));
    painter.galley(egui::pos2(response.rect.left() + 9.0, top), title, color);
    painter.galley(
        egui::pos2(response.rect.left() + 9.0, top + title_height),
        detail,
        color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            response.enabled(),
            &accessible_label,
        )
    });
    response.on_hover_text(accessible_label)
}
pub fn connection_entry(
    ui: &mut egui::Ui,
    name: &str,
    status: &str,
    selected: bool,
) -> egui::Response {
    let response = ui.add_sized(
        [ui.available_width(), 28.0],
        egui::Button::new("").selected(selected),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            response.enabled(),
            format!("{name} · {status}"),
        )
    });
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.style().interact(&response).fg_stroke.color
    };
    let content = response.rect.shrink2(egui::vec2(9.0, 2.0));
    let galley = |text: &str, width: f32| {
        egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            width.max(1.0),
            egui::TextStyle::Button,
        )
    };
    let status = galley(status, content.width() * 0.5);
    let name = galley(name, content.width() - status.size().x - 12.0);
    let painter = ui.painter().with_clip_rect(content);
    painter.galley(
        egui::pos2(content.left(), content.center().y - name.size().y / 2.0),
        name,
        color,
    );
    painter.galley(
        egui::pos2(
            content.right() - status.size().x,
            content.center().y - status.size().y / 2.0,
        ),
        status,
        color,
    );
    response
}
pub fn file_entry(
    ui: &mut egui::Ui,
    directory: bool,
    text: &str,
    selected: bool,
) -> egui::Response {
    let height = (ui.text_style_height(&egui::TextStyle::Button)
        + 2.0 * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y);
    let response = ui.add_sized(
        [ui.available_width(), height],
        egui::Button::new("").selected(selected),
    );
    let color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.style().interact(&response).fg_stroke.color
    };
    let icon = Rect::from_center_size(
        egui::pos2(response.rect.left() + 13.0, response.rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    draw(
        ui.painter(),
        icon,
        if directory { Icon::Folder } else { Icon::File },
        color,
    );
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (response.rect.width() - 30.0).max(1.0),
        egui::TextStyle::Button,
    );
    ui.painter().galley(
        egui::pos2(
            response.rect.left() + 27.0,
            response.rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), text)
    });
    response.on_hover_text(text)
}
pub fn draw(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new((rect.width() / 13.0).max(1.0), color);
    let at = |x: f32, y: f32| rect.min + rect.size() * egui::vec2(x, y);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    };
    let border = |a: (f32, f32), b: (f32, f32)| {
        p.rect_stroke(
            Rect::from_min_max(at(a.0, a.1), at(b.0, b.1)),
            1,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Resolution => {
            border((0.06, 0.13), (0.94, 0.78));
            line((0.35, 0.94), (0.65, 0.94));
            line((0.5, 0.78), (0.5, 0.94));
            line((0.25, 0.61), (0.75, 0.3));
            line((0.25, 0.61), (0.25, 0.42));
            line((0.25, 0.61), (0.45, 0.61));
            line((0.75, 0.3), (0.55, 0.3));
            line((0.75, 0.3), (0.75, 0.49));
        }
        Icon::Filter => {
            p.add(egui::Shape::closed_line(
                vec![
                    at(0.08, 0.15),
                    at(0.92, 0.15),
                    at(0.6, 0.52),
                    at(0.6, 0.82),
                    at(0.4, 0.94),
                    at(0.4, 0.52),
                ],
                stroke,
            ));
        }
        Icon::NewChat => {
            line((0.5, 0.15), (0.5, 0.85));
            line((0.15, 0.5), (0.85, 0.5));
        }
        Icon::History => {
            p.add(egui::Shape::line(
                (0..=24)
                    .map(|step| {
                        let angle = -std::f32::consts::FRAC_PI_2
                            + step as f32 / 24.0 * std::f32::consts::PI * 1.75;
                        rect.center() + egui::Vec2::angled(angle) * rect.width() * 0.38
                    })
                    .collect(),
                stroke,
            ));
            line((0.5, 0.12), (0.32, 0.12));
            line((0.5, 0.12), (0.5, 0.29));
            line((0.5, 0.3), (0.5, 0.52));
            line((0.5, 0.52), (0.66, 0.6));
        }
        Icon::Folder => {
            p.add(egui::Shape::closed_line(
                vec![
                    at(0.05, 0.22),
                    at(0.38, 0.22),
                    at(0.49, 0.36),
                    at(0.95, 0.36),
                    at(0.95, 0.86),
                    at(0.05, 0.86),
                ],
                stroke,
            ));
        }
        Icon::File => {
            p.add(egui::Shape::closed_line(
                vec![
                    at(0.2, 0.08),
                    at(0.62, 0.08),
                    at(0.82, 0.3),
                    at(0.82, 0.92),
                    at(0.2, 0.92),
                ],
                stroke,
            ));
            line((0.62, 0.08), (0.62, 0.3));
            line((0.62, 0.3), (0.82, 0.3));
            line((0.33, 0.5), (0.68, 0.5));
            line((0.33, 0.68), (0.68, 0.68));
        }
        Icon::Monitor => {
            border((0.04, 0.13), (0.96, 0.72));
            line((0.5, 0.72), (0.5, 0.92));
            line((0.27, 0.92), (0.73, 0.92));
        }
        Icon::Monitors => {
            border((0.34, 0.08), (0.97, 0.62));
            border((0.03, 0.36), (0.68, 0.88));
            line((0.35, 0.88), (0.35, 0.98));
        }
        Icon::Tab => {
            border((0.05, 0.12), (0.95, 0.9));
            line((0.05, 0.34), (0.95, 0.34));
            line((0.7, 0.6), (0.3, 0.6));
            line((0.3, 0.6), (0.45, 0.45));
            line((0.3, 0.6), (0.45, 0.75));
        }
        Icon::SecureKeys => {
            border((0.03, 0.21), (0.97, 0.84));
            for y in [0.4, 0.58] {
                for x in [0.2, 0.4, 0.6, 0.8] {
                    p.circle_filled(at(x, y), stroke.width, color);
                }
            }
            line((0.3, 0.73), (0.7, 0.73));
        }
        Icon::Disconnect => {
            line((0.32, 0.08), (0.32, 0.3));
            line((0.68, 0.08), (0.68, 0.3));
            p.add(egui::Shape::line(
                vec![
                    at(0.2, 0.3),
                    at(0.2, 0.55),
                    at(0.5, 0.75),
                    at(0.8, 0.55),
                    at(0.8, 0.3),
                ],
                stroke,
            ));
            line((0.5, 0.75), (0.5, 0.95));
            line((0.08, 0.88), (0.92, 0.12));
        }
        Icon::EndSession => {
            p.add(egui::Shape::line(
                (0..=32)
                    .map(|n| {
                        let angle = -std::f32::consts::FRAC_PI_2
                            + 0.65
                            + n as f32 * (std::f32::consts::TAU - 1.3) / 32.0;
                        rect.center() + egui::Vec2::angled(angle) * rect.width() * 0.37
                    })
                    .collect(),
                stroke,
            ));
            line((0.5, 0.04), (0.5, 0.5));
        }
        Icon::TerminalSettings => {
            border((0.04, 0.15), (0.96, 0.88));
            line((0.2, 0.34), (0.4, 0.5));
            line((0.4, 0.5), (0.2, 0.66));
            line((0.51, 0.68), (0.79, 0.68));
        }
        Icon::Attach => {
            // Camera silhouette distinguishes capture from the new-chat plus.
            p.add(egui::Shape::closed_line(
                vec![
                    at(0.06, 0.3),
                    at(0.28, 0.3),
                    at(0.38, 0.15),
                    at(0.65, 0.15),
                    at(0.75, 0.3),
                    at(0.94, 0.3),
                    at(0.94, 0.85),
                    at(0.06, 0.85),
                ],
                stroke,
            ));
            p.circle_stroke(at(0.5, 0.57), rect.width() * 0.18, stroke);
            line((0.78, 0.41), (0.85, 0.41));
        }
        Icon::Send => {
            line((0.5, 0.92), (0.5, 0.12));
            line((0.5, 0.12), (0.15, 0.47));
            line((0.5, 0.12), (0.85, 0.47));
        }
        Icon::Remove => {
            line((0.2, 0.25), (0.8, 0.25));
            line((0.35, 0.1), (0.65, 0.1));
            line((0.25, 0.25), (0.3, 0.9));
            line((0.3, 0.9), (0.7, 0.9));
            line((0.7, 0.9), (0.75, 0.25));
            line((0.43, 0.4), (0.43, 0.75));
            line((0.57, 0.4), (0.57, 0.75));
        }
        Icon::Stop => {
            p.rect_filled(rect.shrink(rect.width() * 0.2), 1, color);
        }
        Icon::Settings => {
            p.circle_stroke(rect.center(), rect.width() * 0.26, stroke);
            p.circle_stroke(rect.center(), rect.width() * 0.08, stroke);
            for n in 0..8 {
                let axis = egui::Vec2::angled(n as f32 * std::f32::consts::TAU / 8.0);
                p.line_segment(
                    [
                        rect.center() + axis * rect.width() * 0.29,
                        rect.center() + axis * rect.width() * 0.43,
                    ],
                    stroke,
                );
            }
        }
    }
}
