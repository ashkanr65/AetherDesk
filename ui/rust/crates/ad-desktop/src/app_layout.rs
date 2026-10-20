//! Application geometry uses egui points. Remote framebuffer pixels are separate.
use eframe::egui::{self, Vec2};

pub const MINIMUM: Vec2 = egui::vec2(480.0, 320.0);

pub fn dialog_size(ctx: &egui::Context, desired: Vec2) -> Vec2 {
    desired.min((ctx.content_rect().size() - egui::vec2(32.0, 70.0)).max(egui::vec2(180.0, 140.0)))
}

#[derive(Default)]
pub struct WindowSizing {
    last: Option<(Vec2, f32, f32)>,
}
impl WindowSizing {
    pub fn update(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return;
        }
        let viewport = ctx.input(|i| i.viewport().clone());
        let Some(monitor) = viewport
            .monitor_size
            .filter(|s| s.is_finite() && s.min_elem() > 0.0)
        else {
            return;
        };
        let signature = (
            monitor,
            viewport.native_pixels_per_point.unwrap_or(1.0),
            ctx.zoom_factor(),
        );
        if self.last == Some(signature) || viewport.minimized == Some(true) {
            return;
        }
        self.last = Some(signature);
        // Leave room for native decorations/taskbar. Never change the user's
        // font/zoom preference, or reinterpret a monitor origin using another DPI.
        let available = (monitor - egui::vec2(24.0, 80.0)).max(egui::vec2(120.0, 100.0));
        // Permit Windows half-screen snapping even on a small/high-DPI monitor.
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
            MINIMUM.min(egui::vec2(available.x * 0.5, available.y)),
        ));
        if viewport.maximized != Some(true) && viewport.fullscreen != Some(true) {
            if let Some(inner) = viewport.inner_rect {
                let fitted = inner.size().min(available);
                if inner.width() > available.x + 1.0 || inner.height() > available.y + 1.0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fitted));
                }
            }
        }
        ctx.request_repaint();
    }
}

pub fn sidebar_maximum(available: f32) -> f32 {
    // Keep usable content beside the sidebar, including a half-width window.
    (available - 260.0).min(available * 0.55).clamp(0.0, 900.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_dpi_changes_refit_oversized_windows_without_changing_user_zoom() {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(1.25);
        let mut sizing = WindowSizing::default();
        for (monitor, dpi, expect_fit) in [
            (egui::vec2(2560.0, 1440.0), 1.0, false),
            (egui::vec2(1024.0, 576.0), 1.5, true),
            (egui::vec2(800.0, 450.0), 2.0, true),
            (egui::vec2(2560.0, 1440.0), 1.0, false),
        ] {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.monitor_size = Some(monitor);
            viewport.native_pixels_per_point = Some(dpi);
            viewport.inner_rect = Some(egui::Rect::from_min_size(
                egui::pos2(-1200.0, 20.0),
                egui::vec2(1280.0, 840.0),
            ));
            let output = ctx.run(input, |ctx| sizing.update(ctx));
            let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
            assert_eq!(
                commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::InnerSize(_))),
                expect_fit
            );
            assert!(commands.iter().any(|c| matches!(c, egui::ViewportCommand::MinInnerSize(size) if size.x <= monitor.x * 0.5)));
            assert_eq!(ctx.zoom_factor(), 1.25);
            assert!(!commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::OuterPosition(_))));
        }
    }
    #[test]
    fn monitor_layout_checks_do_not_fight_resizing_maximized_or_fullscreen_windows() {
        let ctx = egui::Context::default();
        let mut sizing = WindowSizing::default();
        for (index, fullscreen) in [false, true].into_iter().enumerate() {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.monitor_size = Some(egui::vec2(1000.0 + index as f32, 600.0));
            viewport.inner_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            ));
            viewport.maximized = Some(!fullscreen);
            viewport.fullscreen = Some(fullscreen);
            let output = ctx.run(input.clone(), |ctx| sizing.update(ctx));
            assert!(!output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::InnerSize(_))));
            let output = ctx.run(input, |ctx| sizing.update(ctx));
            assert!(output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .is_empty());
        }
    }
}
