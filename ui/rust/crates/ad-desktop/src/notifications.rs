//! Bounded, five-second notifications. Entries hold data, never closed-tab callbacks.
use eframe::egui;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
const LIFETIME: Duration = Duration::from_secs(5);
const FADE: Duration = Duration::from_millis(180);
const MAX_PENDING: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Information,
    Status,
    Warning,
    Error,
}
pub struct Notice {
    pub text: String,
    pub kind: Kind,
}
impl Notice {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Self {
            text: crate::logging::redact("message", &text.into()),
            kind,
        }
    }
}
#[derive(Default)]
pub struct Center {
    pending: VecDeque<Notice>,
    current: Option<(Notice, Instant)>,
}
impl Center {
    pub fn push(&mut self, notice: Notice) {
        match notice.kind {
            Kind::Error => tracing::error!(target: "app.notice", message = %notice.text),
            Kind::Warning => tracing::warn!(target: "app.notice", message = %notice.text),
            _ => tracing::info!(target: "app.notice", message = %notice.text),
        }
        if self.current.is_none() {
            self.current = Some((notice, Instant::now()));
            return;
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(notice);
    }
    fn advance(&mut self, now: Instant) {
        if self
            .current
            .as_ref()
            .is_some_and(|(_, start)| now.saturating_duration_since(*start) >= LIFETIME)
        {
            self.current = None;
        }
        if self.current.is_none() {
            self.current = self.pending.pop_front().map(|n| (n, now));
        }
    }
    pub fn ui(&mut self, ctx: &egui::Context, dark: bool) {
        self.advance(Instant::now());
        let Some((notice, start)) = &self.current else {
            return;
        };
        let remaining = LIFETIME.saturating_sub(start.elapsed());
        let opacity = (remaining.as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        let colors = crate::theme::colors(dark);
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("notification"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -32.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                egui::Frame::popup(ui.style())
                    .fill(colors.selection)
                    .show(ui, |ui| {
                        ui.set_max_width((ctx.content_rect().width() - 40.0).clamp(120.0, 800.0));
                        ui.horizontal_wrapped(|ui| {
                            let label = match notice.kind {
                                Kind::Error => "Error",
                                Kind::Warning => "Warning",
                                Kind::Status => "Status",
                                Kind::Information => "Information",
                            };
                            ui.label(
                                egui::RichText::new(format!("{label}: {}", notice.text))
                                    .color(colors.selection_text),
                            );
                            dismiss = ui
                                .small_button("×")
                                .on_hover_text("Dismiss notification")
                                .clicked();
                        });
                    });
            });
        if dismiss {
            self.current = None;
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_is_bounded_and_each_visible_notice_gets_its_own_lifetime() {
        let mut center = Center::default();
        for i in 0..300 {
            center.push(Notice::new(Kind::Information, i.to_string()));
        }
        assert_eq!(center.pending.len(), MAX_PENDING);
        let now = center.current.as_ref().unwrap().1;
        center.advance(now);
        assert_eq!(center.current.as_ref().unwrap().0.text, "0");
        center.advance(now + LIFETIME - Duration::from_millis(1));
        assert_eq!(center.current.as_ref().unwrap().0.text, "0");
        center.advance(now + LIFETIME);
        assert_eq!(center.current.as_ref().unwrap().0.text, "44");
        assert_eq!(center.current.as_ref().unwrap().1, now + LIFETIME);
        assert!(
            !Notice::new(Kind::Error, "Authorization: Bearer test-secret")
                .text
                .contains("test-secret")
        );
    }
}
