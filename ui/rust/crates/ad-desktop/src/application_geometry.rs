//! Remote pixels, Windows physical pixels and egui points must remain separate.
use eframe::egui::{self, Rect, Vec2};

#[derive(Clone, Copy)]
pub(super) struct Scale {
    physical: f32,
    points: f32,
}
impl Scale {
    pub(super) fn new(native: f32, pixels_per_point: f32, zoom: f32) -> Self {
        Self {
            physical: native.max(0.01) * zoom.clamp(0.5, 3.0),
            points: pixels_per_point.max(0.01),
        }
    }
    pub(super) fn logical(self, pixels: Vec2) -> Vec2 {
        pixels * self.physical / self.points
    }
    pub(super) fn physical(self, pixels: Vec2) -> Vec2 {
        pixels * self.physical
    }
    pub(super) fn extent(self, points: Vec2) -> (u32, u32) {
        let pixels = points * self.points / self.physical;
        (
            pixels.x.round().max(64.0) as u32,
            pixels.y.round().max(64.0) as u32,
        )
    }
    pub(super) fn image(self, area: Rect, pixels: Vec2) -> Rect {
        Rect::from_min_size(area.min, self.logical(pixels))
    }
}

/// Keep the entire menu reachable even when the host opened it above the app,
/// across a monitor gap, or on a display with a different work area.
pub(super) fn menu_bounds(
    origin: (i32, i32),
    anchor: Rect,
    ppp: f32,
    scale: Scale,
    popup: &ad_stream_protocol::window_stream::Popup,
    work: Option<crate::remote_geometry::PhysicalRect>,
) -> crate::remote_geometry::PhysicalRect {
    let offset = scale.physical(egui::vec2(popup.x as f32, popup.y as f32));
    let size = scale.physical(egui::vec2(popup.width as f32, popup.height as f32));
    let mut rect = crate::remote_geometry::PhysicalRect {
        x: origin.0 + (anchor.left() * ppp + offset.x).round() as i32,
        y: origin.1 + (anchor.top() * ppp + offset.y).round() as i32,
        width: size.x.round().max(1.0) as u32,
        height: size.y.round().max(1.0) as u32,
    };
    if let Some(work) = work {
        let fit = (work.width as f32 / rect.width as f32)
            .min(work.height as f32 / rect.height as f32)
            .min(1.0);
        rect.width = (rect.width as f32 * fit).floor().max(1.0) as u32;
        rect.height = (rect.height as f32 * fit).floor().max(1.0) as u32;
        rect.x = i64::from(rect.x).clamp(
            i64::from(work.x),
            i64::from(work.x) + i64::from(work.width - rect.width),
        ) as i32;
        rect.y = i64::from(rect.y).clamp(
            i64::from(work.y),
            i64::from(work.y) + i64::from(work.height - rect.height),
        ) as i32;
    }
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_dpi_keeps_text_logical_size_and_host_extent_unchanged() {
        let pixels = egui::vec2(800.0, 400.0);
        for native in [1.0, 1.25, 1.5, 2.0] {
            for ui_zoom in [1.0, 1.3] {
                let scale = Scale::new(native, native * ui_zoom, 1.0);
                let size = scale.logical(pixels);
                assert!((size * ui_zoom - pixels).length() < 0.01);
                assert_eq!(scale.extent(size), (800, 400));
                assert_eq!(scale.physical(pixels), pixels * native);
            }
        }
        let scale = Scale::new(2.0, 2.0, 1.5);
        assert_eq!(scale.logical(pixels), pixels * 1.5);
        assert_eq!(scale.extent(pixels * 1.5), (800, 400));
        let area = Rect::from_min_size(egui::pos2(3.0, 7.0), pixels * 5.0);
        assert_eq!(scale.image(area, pixels).size(), pixels * 1.5);
    }
    #[test]
    fn negative_menu_offsets_stay_inside_owner_monitor_and_keep_input_mapping() {
        let work = crate::remote_geometry::PhysicalRect {
            x: -1920,
            y: 337,
            width: 1920,
            height: 1032,
        };
        let popup = ad_stream_protocol::window_stream::Popup {
            window: 1,
            x: 24,
            y: -480,
            width: 244,
            height: 483,
            tiles: vec![],
        };
        for native in [1.0, 2.0] {
            let scale = Scale::new(native, native, 1.0);
            let rect = menu_bounds(
                (-1800, 400),
                Rect::from_min_size(egui::pos2(0.0, 32.0), Vec2::ZERO),
                native,
                scale,
                &popup,
                Some(work),
            );
            assert!(rect.x >= work.x && rect.y >= work.y);
            assert!(rect.x + rect.width as i32 <= work.x + work.width as i32);
            assert!(rect.y + rect.height as i32 <= work.y + work.height as i32);
            // Repositioning never changes the host menu's source offsets.
            assert_eq!((popup.x, popup.y), (24, -480));
            assert_eq!(rect.width, (244.0 * native) as u32);
        }
    }
    #[test]
    fn scaled_and_fitted_menus_map_clicks_to_the_original_host_pixels() {
        let popup = ad_stream_protocol::window_stream::Popup {
            window: 1,
            x: 290,
            y: -480,
            width: 244,
            height: 2000,
            tiles: vec![],
        };
        let work = crate::remote_geometry::PhysicalRect {
            x: -1920,
            y: 337,
            width: 1920,
            height: 1032,
        };
        for native in [1.0, 1.5, 2.0] {
            for zoom in [0.5, 1.0, 3.0] {
                let scale = Scale::new(native, native, zoom);
                let bounds =
                    menu_bounds((-1800, 400), Rect::ZERO, native, scale, &popup, Some(work));
                assert!(bounds.width <= work.width && bounds.height <= work.height);
                let rect = Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(bounds.width as f32, bounds.height as f32) / native,
                );
                let info = ad_stream_protocol::window_stream::WindowInfo {
                    id: 1,
                    title: String::new(),
                    width: popup.width,
                    height: popup.height,
                    transient_for: None,
                };
                let (x, y) = super::super::point(rect.min + rect.size() * 0.5, rect, &info);
                assert_eq!((x + popup.x, y + popup.y), (412, 520));
            }
        }
    }
}
