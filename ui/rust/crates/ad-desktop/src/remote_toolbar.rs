//! DPI-independent, validated toolbar placement. Store ratios rather than
//! obsolete desktop coordinates, so reconnect and monitor removal stay visible.
use eframe::egui;
use serde::{Deserialize, Serialize};
pub fn dimensions(vertical: bool, scale: f32) -> egui::Vec2 {
    let margin = (6.0 * scale).round() * 2.0;
    let short = 28.0 * scale + margin;
    let long = 8.0 * 28.0 * scale + 7.0 * 4.0 * scale + margin;
    if vertical {
        egui::vec2(short, long)
    } else {
        egui::vec2(long, short)
    }
}
pub fn bar<R>(
    ctx: &egui::Context,
    id: egui::Id,
    scope: egui::Rect,
    rect: egui::Rect,
    vertical: bool,
    scale: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    // Reset both dimensions each frame while retaining the same Area and widget
    // identities, so a held drag survives changing orientation or DPI.
    egui::Area::new(id)
        .fade_in(false)
        .fixed_pos(rect.min)
        .default_size(rect.size())
        .movable(false)
        .constrain_to(scope)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_width(rect.width());
            ui.set_height(rect.height());
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0) * scale;
            ui.spacing_mut().interact_size = egui::vec2(28.0, 28.0) * scale;
            let frame = egui::Frame::popup(ui.style())
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::same((6.0 * scale).round() as i8))
                .show(ui, |ui| {
                    ui.with_layout(
                        if vertical {
                            egui::Layout::top_down(egui::Align::Center)
                        } else {
                            egui::Layout::left_to_right(egui::Align::Center)
                        },
                        contents,
                    )
                    .inner
                });
            ui.painter().rect_stroke(
                frame.response.rect,
                4,
                ui.visuals().window_stroke,
                egui::StrokeKind::Inside,
            );
            frame.inner
        })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub edge: Option<Edge>,
    pub along: f32,
    pub x: f32,
    pub y: f32,
    pub monitor: Option<String>,
}
impl Default for Placement {
    fn default() -> Self {
        Self {
            edge: Some(Edge::Top),
            along: 0.5,
            x: 0.5,
            y: 0.0,
            monitor: None,
        }
    }
}
impl Placement {
    pub fn valid(&self) -> bool {
        [self.along, self.x, self.y]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && self
                .monitor
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
    }
    pub fn vertical(&self) -> bool {
        matches!(self.edge, Some(Edge::Left | Edge::Right))
    }
    pub fn rect(&self, bounds: egui::Rect, size: egui::Vec2) -> egui::Rect {
        let size = size.min(bounds.size()).max(egui::vec2(1.0, 1.0));
        let room = (bounds.size() - size).max(egui::Vec2::ZERO);
        let relative = match self.edge {
            Some(Edge::Top) => egui::vec2(room.x * self.along, 0.0),
            Some(Edge::Bottom) => egui::vec2(room.x * self.along, room.y),
            Some(Edge::Left) => egui::vec2(0.0, room.y * self.along),
            Some(Edge::Right) => egui::vec2(room.x, room.y * self.along),
            None => egui::vec2(room.x * self.x, room.y * self.y),
        };
        egui::Rect::from_min_size(bounds.min + relative, size)
    }
    #[cfg(test)]
    pub fn drag(
        &mut self,
        pointer: egui::Pos2,
        grab: egui::Vec2,
        bounds: egui::Rect,
        size: egui::Vec2,
    ) {
        self.drag_scaled(pointer, grab, bounds, size, 64.0);
    }
    pub fn drag_scaled(
        &mut self,
        pointer: egui::Pos2,
        grab: egui::Vec2,
        bounds: egui::Rect,
        size: egui::Vec2,
        margin: f32,
    ) {
        let distances = [
            (pointer.y - bounds.top(), Edge::Top),
            (bounds.bottom() - pointer.y, Edge::Bottom),
            (pointer.x - bounds.left(), Edge::Left),
            (bounds.right() - pointer.x, Edge::Right),
        ];
        let (distance, edge) = distances
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        self.edge = if distance <= margin { Some(edge) } else { None };
        let room = (bounds.size() - size).max(egui::Vec2::ZERO);
        let origin = pointer - grab - bounds.min;
        self.x = if room.x > 0.0 {
            (origin.x / room.x).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.y = if room.y > 0.0 {
            (origin.y / room.y).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.along = if self.vertical() { self.y } else { self.x };
    }
    pub fn near_edge(&self, pointer: egui::Pos2, bounds: egui::Rect) -> bool {
        bounds.contains(pointer)
            && match self.edge {
                Some(Edge::Top) => pointer.y <= bounds.top() + 12.0,
                Some(Edge::Bottom) => pointer.y >= bounds.bottom() - 12.0,
                Some(Edge::Left) => pointer.x <= bounds.left() + 12.0,
                Some(Edge::Right) => pointer.x >= bounds.right() - 12.0,
                None => false,
            }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    schema_version: u32,
    placement: Placement,
}
pub fn load() -> Result<Placement, String> {
    let Some(value) = crate::storage::read_json("remote-toolbar.json")? else {
        return Ok(Placement::default());
    };
    let saved: Saved = serde_json::from_value(value)
        .map_err(|_| "Saved remote toolbar position is invalid; file preserved".to_owned())?;
    if saved.schema_version != 1 || !saved.placement.valid() {
        return Err("Saved remote toolbar position is invalid; file preserved".into());
    }
    Ok(saved.placement)
}
pub fn save(placement: &Placement) -> Result<(), String> {
    if !placement.valid() {
        return Err("Cannot save an invalid toolbar position".into());
    }
    crate::storage::write_json(
        "remote-toolbar.json",
        &serde_json::to_value(Saved {
            schema_version: 1,
            placement: placement.clone(),
        })
        .map_err(|e| e.to_string())?,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dock_area_shrinks_after_switching_between_vertical_and_horizontal_edges() {
        let ctx = egui::Context::default();
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        for edge in [
            Edge::Left,
            Edge::Top,
            Edge::Right,
            Edge::Bottom,
            Edge::Left,
            Edge::Bottom,
        ] {
            let placement = Placement {
                edge: Some(edge),
                ..Default::default()
            };
            for scale in [0.5_f32, 0.75, 1.0, 1.5, 2.0] {
                for pass in 0..3 {
                    let mut actual = egui::Vec2::ZERO;
                    let expected = dimensions(placement.vertical(), scale);
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(bounds),
                            ..Default::default()
                        },
                        |ctx| {
                            actual = bar(
                                ctx,
                                egui::Id::new("dock-resize-fixture"),
                                bounds,
                                placement.rect(bounds, expected),
                                placement.vertical(),
                                scale,
                                |ui| {
                                    ui.allocate_exact_size(
                                        egui::vec2(28.0, 28.0) * scale,
                                        egui::Sense::drag(),
                                    );
                                    for icon in [
                                        crate::ui_icons::Icon::Tab,
                                        crate::ui_icons::Icon::Monitor,
                                        crate::ui_icons::Icon::Monitors,
                                        crate::ui_icons::Icon::SecureKeys,
                                        crate::ui_icons::Icon::Resolution,
                                        crate::ui_icons::Icon::Disconnect,
                                        crate::ui_icons::Icon::EndSession,
                                    ] {
                                        let response = crate::ui_icons::button_scaled(
                                            ui, icon, "Control", false, scale,
                                        );
                                        assert!((response.rect.width() - 28.0 * scale).abs() < 0.1);
                                        assert!(
                                            (response.rect.height() - 28.0 * scale).abs() < 0.1
                                        );
                                    }
                                },
                            )
                            .response
                            .rect
                            .size();
                        },
                    );
                    if pass == 2 {
                        assert!(
                        (actual.x - expected.x).abs() <= 1.0
                            && (actual.y - expected.y).abs() <= 1.0,
                        "dock retained a prior orientation: {edge:?}: {actual:?} vs {expected:?}"
                    );
                    }
                }
            }
        }
    }
    #[test]
    fn drag_tracks_pointer_and_snaps_all_four_orientations() {
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let size = egui::vec2(420.0, 40.0);
        let mut p = Placement::default();
        p.drag(
            egui::pos2(500.0, 300.0),
            egui::vec2(10.0, 5.0),
            bounds,
            size,
        );
        assert_eq!(p.edge, None);
        assert!(p.rect(bounds, size).min.distance(egui::pos2(490.0, 295.0)) < 0.01);
        for (point, edge) in [
            (egui::pos2(500.0, 2.0), Edge::Top),
            (egui::pos2(500.0, 798.0), Edge::Bottom),
            (egui::pos2(2.0, 400.0), Edge::Left),
            (egui::pos2(1198.0, 400.0), Edge::Right),
        ] {
            p.drag(point, egui::Vec2::ZERO, bounds, size);
            assert_eq!(p.edge, Some(edge));
            assert_eq!(p.vertical(), matches!(edge, Edge::Left | Edge::Right));
        }
    }
    #[test]
    fn stored_ratios_keep_every_edge_and_floating_bar_inside_resized_monitor() {
        let bounds = egui::Rect::from_min_size(egui::pos2(-600.0, 20.0), egui::vec2(200.0, 100.0));
        let mut p = Placement {
            edge: None,
            along: 1.0,
            x: 1.0,
            y: 1.0,
            monitor: Some("DISPLAY2".into()),
        };
        for edge in [
            None,
            Some(Edge::Top),
            Some(Edge::Bottom),
            Some(Edge::Left),
            Some(Edge::Right),
        ] {
            p.edge = edge;
            assert!(bounds.contains_rect(p.rect(bounds, egui::vec2(420.0, 180.0))));
        }
    }
    #[test]
    fn persisted_positions_reject_nonfinite_or_out_of_range_values() {
        let p = Placement {
            x: f32::NAN,
            ..Default::default()
        };
        assert!(!p.valid());
        let p = Placement {
            along: 2.0,
            ..Default::default()
        };
        assert!(!p.valid());
        let p = Placement::default();
        let value = serde_json::to_value(&p).unwrap();
        let restored: Placement = serde_json::from_value(value).unwrap();
        assert!(restored.valid());
        assert_eq!(restored.edge, Some(Edge::Top));
    }
    #[test]
    fn docking_margin_scales_and_other_monitor_edges_do_not_reveal_the_bar() {
        let bounds = egui::Rect::from_min_size(egui::pos2(-1200.0, 0.0), egui::vec2(1200.0, 800.0));
        let mut p = Placement::default();
        let size = egui::vec2(420.0, 40.0);
        p.drag_scaled(
            egui::pos2(-600.0, 100.0),
            egui::Vec2::ZERO,
            bounds,
            size,
            128.0,
        );
        assert_eq!(p.edge, Some(Edge::Top));
        p.drag_scaled(
            egui::pos2(-600.0, 100.0),
            egui::Vec2::ZERO,
            bounds,
            size,
            64.0,
        );
        assert_eq!(p.edge, None);
        p.edge = Some(Edge::Top);
        assert!(p.near_edge(egui::pos2(-600.0, 3.0), bounds));
        assert!(!p.near_edge(egui::pos2(600.0, 3.0), bounds));
    }
}
