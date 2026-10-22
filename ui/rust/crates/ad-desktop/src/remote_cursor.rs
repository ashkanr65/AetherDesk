//! Cursor shape is independent of desktop pixels and follows the local pointer.
use eframe::egui;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct Cursor {
    image: Option<Arc<ad_display_decoder::CursorState>>,
    texture: Option<egui::TextureHandle>,
}
impl Cursor {
    pub(crate) fn paint(
        &mut self,
        ui: &mut egui::Ui,
        desktop: egui::Rect,
        scale: f32,
        image: Option<Arc<ad_display_decoder::CursorState>>,
        allowed: bool,
    ) {
        let Some(image) = image else { return };
        if !image.visible || image.width == 0 || image.height == 0 {
            return;
        }
        let Some(position) = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| allowed && desktop.contains(*p))
        else {
            return;
        };
        if self
            .image
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &image))
        {
            let pixels = image
                .rgba
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
                .collect();
            self.texture = Some(ui.ctx().load_texture(
                "host-cursor",
                egui::ColorImage::new([image.width as usize, image.height as usize], pixels),
                egui::TextureOptions::LINEAR,
            ));
            self.image = Some(image.clone());
        }
        if let Some(texture) = &self.texture {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            ui.painter().with_clip_rect(desktop).image(
                texture.id(),
                cursor_rect(position, &image, scale),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}
fn cursor_rect(
    position: egui::Pos2,
    image: &ad_display_decoder::CursorState,
    scale: f32,
) -> egui::Rect {
    egui::Rect::from_min_size(
        position - egui::vec2(image.hotspot_x as f32, image.hotspot_y as f32) * scale,
        egui::vec2(image.width as f32, image.height as f32) * scale,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_cursor_uploads_only_its_small_shape_and_restores_local_pointer_outside_desktop() {
        let ctx = egui::Context::default();
        let shape = Arc::new(ad_display_decoder::CursorState {
            width: 2,
            height: 1,
            visible: true,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            ..Default::default()
        });
        let mut cursor = Cursor::default();
        let desktop = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 150.0));
        let mut frame = |position| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(desktop),
                    events: vec![egui::Event::PointerMoved(position)],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        cursor.paint(ui, desktop, 1.0, Some(shape.clone()), true)
                    });
                },
            )
        };
        let first = frame(egui::pos2(40.0, 40.0));
        assert_eq!(first.platform_output.cursor_icon, egui::CursorIcon::None);
        let second = frame(egui::pos2(55.0, 55.0));
        assert_eq!(second.platform_output.cursor_icon, egui::CursorIcon::None);
        let outside = frame(egui::pos2(400.0, 400.0));
        assert_ne!(outside.platform_output.cursor_icon, egui::CursorIcon::None);
        drop(frame);
        let texture = cursor.texture.as_ref().unwrap();
        assert_eq!(texture.size(), [2, 1]);
        assert!(first
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| *id == texture.id()));
        assert!(!second
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| *id == texture.id()));
    }
    #[test]
    fn host_cursor_hotspot_tracks_local_pointer_at_every_presentation_scale() {
        let image = ad_display_decoder::CursorState {
            width: 24,
            height: 32,
            hotspot_x: 5,
            hotspot_y: 7,
            visible: true,
            ..Default::default()
        };
        for scale in [0.5, 1.0, 1.5, 2.0] {
            let pointer = egui::pos2(420.0, 195.0);
            let rect = cursor_rect(pointer, &image, scale);
            assert_eq!(rect.min + egui::vec2(5.0, 7.0) * scale, pointer);
            assert_eq!(rect.size(), egui::vec2(24.0, 32.0) * scale);
        }
    }
}
