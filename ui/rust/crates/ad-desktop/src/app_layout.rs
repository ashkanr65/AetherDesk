//! Application geometry uses egui points. Remote framebuffer pixels are separate.
use eframe::egui::{self, Vec2};

pub const MINIMUM: Vec2 = egui::vec2(480.0, 320.0);

pub fn dialog_size(ctx: &egui::Context, desired: Vec2) -> Vec2 {
    desired.min((ctx.content_rect().size() - egui::vec2(32.0, 70.0)).max(egui::vec2(180.0, 140.0)))
}

const SETTLE_SECONDS: f64 = 0.18;
#[derive(Clone, Copy, Debug, PartialEq)]
struct Signature {
    monitor: egui::Rect,
    native_scale: f32,
    zoom: f32,
    available: Vec2,
}
#[derive(Clone, Copy)]
struct Sample {
    signature: Signature,
    inner: Vec2,
    outer: egui::Rect,
    work: egui::Rect,
    moving: bool,
    coherent: bool,
    suspended: bool,
}
impl Sample {
    fn native_size(self) -> Vec2 {
        self.inner * self.signature.zoom
    }
    fn snapped(self) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() < 8.0;
        let side = near(self.outer.left(), self.work.left())
            || near(self.outer.right(), self.work.right());
        side && near(self.outer.width(), self.work.width() * 0.5)
            && (near(self.outer.height(), self.work.height())
                || near(self.outer.height(), self.work.height() * 0.5))
    }
}
#[derive(Clone, Default)]
pub struct WindowSizing {
    minimum: Option<Vec2>,
    initial: Option<Vec2>,
    preferred: Option<Vec2>, // Windows logical size, independent of application zoom.
    applied: Option<Signature>,
    last_size: Option<Vec2>,
    pending: Option<(Signature, egui::Rect, Vec2, f64)>,
    drag_start: Option<(Signature, Vec2)>,
    awaiting: Option<(Vec2, f64)>,
    observed: Option<(Signature, bool, bool)>,
    current_size: Option<Vec2>,
}
fn state_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::new(("window-sizing-v2", viewport))
}
fn sample(ctx: &egui::Context) -> Option<Sample> {
    let viewport = ctx.input(|i| i.viewport().clone());
    let inner = viewport.inner_rect?;
    let outer = viewport.outer_rect.unwrap_or(inner);
    let zoom = ctx.zoom_factor();
    let native = viewport.native_pixels_per_point.unwrap_or(1.0);
    if !inner.is_finite()
        || !outer.is_finite()
        || inner.size().min_elem() <= 0.0
        || !zoom.is_finite()
        || zoom <= 0.0
        || !native.is_finite()
        || native <= 0.0
    {
        return None;
    }
    let area = crate::window_geometry::area(ctx).or_else(|| {
        let size = viewport.monitor_size?;
        (size.is_finite() && size.min_elem() > 0.0).then_some(crate::window_geometry::Area {
            work: egui::Rect::from_min_size(egui::Pos2::ZERO, size),
            identity: egui::Rect::from_min_size(egui::Pos2::ZERO, size * native * zoom),
            coherent: true,
        })
    })?;
    // Use the measured native decorations, not a guessed taskbar/title-bar margin.
    let decorations = (outer.size() - inner.size()).max(Vec2::ZERO);
    let available = (area.work.size() - decorations).max(egui::vec2(80.0, 60.0));
    Some(Sample {
        signature: Signature {
            monitor: area.identity,
            native_scale: native,
            zoom,
            available,
        },
        inner: inner.size(),
        outer,
        work: area.work,
        moving: crate::window_geometry::moving(),
        coherent: area.coherent,
        suspended: viewport.minimized == Some(true)
            || viewport.maximized == Some(true)
            || viewport.fullscreen == Some(true),
    })
}
/// One initial size per viewport. Repainting a popup never resets its user's size.
pub fn builder(
    ctx: &egui::Context,
    id: egui::ViewportId,
    title: impl Into<String>,
    desired: Vec2,
) -> egui::ViewportBuilder {
    let key = state_id(id);
    let mut sizing = ctx
        .data(|d| d.get_temp::<WindowSizing>(key))
        .unwrap_or_default();
    let initial = *sizing.initial.get_or_insert_with(|| {
        sample(ctx)
            .map(|s| desired.min(s.signature.available))
            .unwrap_or(desired)
    });
    sizing.preferred.get_or_insert(desired * ctx.zoom_factor());
    ctx.data_mut(|d| d.insert_temp(key, sizing));
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_icon(crate::theme::native_icon())
        .with_inner_size(initial)
        .with_min_inner_size(MINIMUM.min(initial))
        .with_clamp_size_to_monitor_size(true)
}
/// Shared by root, file, editor, review and comparison native windows.
pub fn update(ctx: &egui::Context, role: &'static str) {
    let key = state_id(ctx.viewport_id());
    let mut sizing = ctx
        .data(|d| d.get_temp::<WindowSizing>(key))
        .unwrap_or_default();
    sizing.update_named(ctx, role);
    ctx.data_mut(|d| d.insert_temp(key, sizing));
}
pub(crate) fn diagnostic_size(ctx: &egui::Context, viewport: egui::ViewportId) -> Option<Vec2> {
    ctx.data(|d| d.get_temp::<WindowSizing>(state_id(viewport)))
        .and_then(|s| s.applied.and(s.current_size))
}
/// Seed from the child viewport's own DPI and measured decorations, never its
/// parent's DPI. Keep the same policy for later monitor transitions.
pub(crate) fn application_initial_size(ctx: &egui::Context, desired: Vec2) -> Vec2 {
    let desired = sample(ctx)
        .map(|s| desired.min(s.signature.available))
        .unwrap_or(desired)
        .max(egui::vec2(100.0, 80.0));
    let key = state_id(ctx.viewport_id());
    let zoom = ctx.zoom_factor();
    ctx.data_mut(|d| {
        let mut sizing = d.get_temp::<WindowSizing>(key).unwrap_or_default();
        sizing.minimum = Some(egui::vec2(100.0, 80.0));
        sizing.preferred = Some(desired * zoom);
        d.insert_temp(key, sizing);
    });
    desired
}
/// Remote application resizing must wait for Windows' move/DPI transaction and
/// our native size command to finish. A monitor transition is a new baseline,
/// not a request to resize an application on the server.
pub(crate) fn application_resize_environment(ctx: &egui::Context) -> Option<(egui::Rect, u32)> {
    let s = sample(ctx)?;
    let key = state_id(ctx.viewport_id());
    let sizing = ctx.data(|d| d.get_temp::<WindowSizing>(key))?;
    sizing
        .settled(s)
        .then_some((s.signature.monitor, ctx.pixels_per_point().to_bits()))
}
impl WindowSizing {
    fn settled(&self, s: Sample) -> bool {
        !s.moving
            && s.coherent
            && !s.suspended
            && self.applied == Some(s.signature)
            && self.pending.is_none()
            && self.awaiting.is_none()
            && self
                .last_size
                .is_some_and(|size| (size - s.native_size()).length() < 1.0)
    }
    fn update_named(&mut self, ctx: &egui::Context, role: &'static str) {
        let Some(sample) = sample(ctx) else { return };
        self.current_size = Some(sample.inner);
        let observation = (sample.signature, sample.moving, sample.coherent);
        if self.observed != Some(observation) {
            tracing::info!(target:"window.geometry",role,viewport=?ctx.viewport_id(),work=?sample.work,
                inner=?sample.inner,dpi=sample.signature.native_scale*96.0,zoom=sample.signature.zoom,
                moving=sample.moving,coherent=sample.coherent,"Window geometry transition");
            self.observed = Some(observation);
        }
        let now = ctx.input(|i| i.time);
        for command in self.step(sample, now) {
            ctx.send_viewport_cmd(command);
        }
        if self.pending.is_some() || self.awaiting.is_some() || sample.moving || !sample.coherent {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
    fn step(&mut self, s: Sample, now: f64) -> Vec<egui::ViewportCommand> {
        let mut commands = Vec::new();
        if s.suspended {
            self.pending = None;
            self.drag_start = None;
            self.awaiting = None;
            return commands;
        }
        if s.moving {
            self.drag_start.get_or_insert((
                self.applied.unwrap_or(s.signature),
                self.last_size.unwrap_or(s.native_size()),
            ));
            self.pending = None;
            self.awaiting = None;
            return commands;
        }
        if !s.coherent {
            self.pending = None;
            return commands;
        }
        if let Some((start, size)) = self.drag_start.take() {
            if start.monitor == s.signature.monitor
                && (size - s.native_size()).length() > 1.0
                && !s.snapped()
            {
                self.preferred = Some(s.native_size());
            }
            self.pending = None;
        }
        if let Some((target, since)) = self.awaiting {
            if (target - s.inner).length() < 2.0 {
                self.awaiting = None;
                self.last_size = Some(s.native_size());
            } else if now - since < 0.75 {
                return commands;
            } else {
                self.awaiting = None;
                self.last_size = Some(s.native_size());
            }
        }
        if self.applied == Some(s.signature)
            && self
                .last_size
                .is_some_and(|size| (size - s.native_size()).length() < 1.0)
        {
            return commands;
        }
        let stable = self
            .pending
            .is_some_and(|(signature, outer, inner, since)| {
                signature == s.signature
                    && outer == s.outer
                    && inner == s.inner
                    && now - since >= SETTLE_SECONDS
            });
        if !stable {
            let unchanged = self.pending.is_some_and(|(signature, outer, inner, _)| {
                signature == s.signature && outer == s.outer && inner == s.inner
            });
            if !unchanged {
                self.pending = Some((s.signature, s.outer, s.inner, now));
            }
            return commands;
        }
        self.pending = None;
        let changed = self.applied != Some(s.signature);
        if self.preferred.is_none() {
            self.preferred = Some(s.native_size());
        }
        // A settled manual resize on the same monitor becomes the new preference.
        if !changed && !s.snapped() {
            self.preferred = Some(s.native_size());
        }
        if changed {
            let available = s.signature.available;
            commands.push(egui::ViewportCommand::MinInnerSize(
                self.minimum
                    .unwrap_or(MINIMUM)
                    .min(egui::vec2(available.x * 0.5, available.y)),
            ));
            if !s.snapped() {
                let wanted = (self.preferred.unwrap() / s.signature.zoom).min(available);
                if (wanted - s.inner).length() > 1.0 {
                    commands.push(egui::ViewportCommand::InnerSize(wanted));
                    self.awaiting = Some((wanted, now));
                }
            }
        }
        self.applied = Some(s.signature);
        self.last_size = Some(s.native_size());
        commands
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
    fn remote_application_resize_waits_for_native_drag_dpi_and_size_acknowledgement() {
        let monitor =
            egui::Rect::from_min_size(egui::pos2(-1920.0, 0.0), egui::vec2(1920.0, 1080.0));
        let first = fixture(monitor, 1.0, 1.0, egui::vec2(800.0, 600.0));
        let mut sizing = WindowSizing::default();
        settle(&mut sizing, first, 0.0);
        assert!(sizing.settled(first));
        let mut next = fixture(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, 1440.0)),
            1.5,
            1.0,
            first.inner,
        );
        next.moving = true;
        for at in [1.0, 2.0, 3.0] {
            assert!(sizing.step(next, at).is_empty());
            assert!(!sizing.settled(next));
        }
        next.moving = false;
        next.coherent = false;
        assert!(sizing.step(next, 4.0).is_empty());
        assert!(!sizing.settled(next));
        next.coherent = true;
        settle(&mut sizing, next, 5.0);
        assert!(sizing.settled(next));
        // A manual resize is admitted only after its stable native size is known.
        next.inner = egui::vec2(900.0, 650.0);
        next.outer = egui::Rect::from_min_size(next.outer.min, next.inner + egui::vec2(12.0, 32.0));
        assert!(!sizing.settled(next));
        settle(&mut sizing, next, 6.0);
        assert!(sizing.settled(next));
        sizing.awaiting = Some((next.inner, 7.0));
        assert!(!sizing.settled(next));
    }
    fn fixture(monitor: egui::Rect, dpi: f32, zoom: f32, inner: Vec2) -> Sample {
        let work = egui::Rect::from_min_max(monitor.min / (dpi * zoom), monitor.max / (dpi * zoom));
        Sample {
            signature: Signature {
                monitor,
                native_scale: dpi,
                zoom,
                available: work.size() - egui::vec2(12.0, 32.0),
            },
            inner,
            outer: egui::Rect::from_min_size(
                work.min + egui::vec2(20.0, 20.0),
                inner + egui::vec2(12.0, 32.0),
            ),
            work,
            moving: false,
            coherent: true,
            suspended: false,
        }
    }
    fn settle(sizing: &mut WindowSizing, s: Sample, at: f64) -> Vec<egui::ViewportCommand> {
        assert!(
            sizing.step(s, at).is_empty(),
            "Must wait for stable geometry"
        );
        sizing.step(s, at + 0.25)
    }
    #[test]
    fn monitor_dpi_changes_refit_oversized_windows_without_changing_user_zoom() {
        let mut sizing = WindowSizing::default();
        let zoom = 1.25;
        let left =
            egui::Rect::from_min_size(egui::pos2(-1920.0, 337.0), egui::vec2(1920.0, 1032.0));
        let primary = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2880.0, 1704.0));
        let preferred = egui::vec2(1280.0, 760.0);
        let original = fixture(left, 1.0, zoom, preferred);
        let initial = settle(&mut sizing, original, 0.0);
        assert!(!initial
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::InnerSize(_))));
        let mut destination = fixture(primary, 2.0, zoom, preferred);
        let commands = settle(&mut sizing, destination, 1.0);
        let fitted = commands
            .iter()
            .find_map(|c| {
                if let egui::ViewportCommand::InnerSize(v) = c {
                    Some(*v)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(fitted, preferred.min(destination.signature.available));
        assert!(!commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::OuterPosition(_))));
        destination.inner = fitted;
        destination.outer =
            egui::Rect::from_min_size(destination.outer.min, fitted + egui::vec2(12.0, 32.0));
        assert!(sizing.step(destination, 1.4).is_empty());
        let returned = fixture(left, 1.0, zoom, fitted);
        let commands = settle(&mut sizing, returned, 2.0);
        assert!(commands
            .iter()
            .any(|c| matches!(c,egui::ViewportCommand::InnerSize(v) if *v==preferred)));
        assert_eq!(sizing.preferred, Some(preferred * zoom));
    }
    #[test]
    fn native_drag_and_incoherent_dpi_never_trigger_corrective_resizes() {
        let monitor = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1032.0));
        let mut sizing = WindowSizing::default();
        let normal = fixture(monitor, 1.0, 1.0, egui::vec2(1200.0, 800.0));
        settle(&mut sizing, normal, 0.0);
        let preference = sizing.preferred;
        let mut crossing = fixture(monitor, 2.0, 1.0, normal.inner);
        crossing.moving = true;
        for tick in 0..30 {
            assert!(sizing.step(crossing, 1.0 + tick as f64 / 10.0).is_empty());
        }
        crossing.moving = false;
        crossing.coherent = false;
        assert!(sizing.step(crossing, 5.0).is_empty());
        crossing.coherent = true;
        assert!(sizing.step(crossing, 5.1).is_empty());
        // Any position change restarts the settling period, even after drag ends.
        crossing.outer = crossing.outer.translate(egui::vec2(1.0, 0.0));
        assert!(sizing.step(crossing, 5.3).is_empty());
        assert!(sizing.step(crossing, 5.4).is_empty());
        let commands = sizing.step(crossing, 5.6);
        assert!(commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::InnerSize(_))));
        assert_eq!(sizing.preferred, preference);
    }
    #[test]
    fn monitor_layout_checks_do_not_fight_resizing_maximized_or_fullscreen_windows() {
        let monitor = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1032.0));
        let normal = fixture(monitor, 1.0, 1.0, egui::vec2(1200.0, 800.0));
        let mut sizing = WindowSizing::default();
        settle(&mut sizing, normal, 0.0);
        let preference = sizing.preferred;
        let mut suspended = fixture(monitor, 2.0, 1.0, egui::vec2(960.0, 500.0));
        suspended.suspended = true;
        for at in [1.0, 2.0, 3.0] {
            assert!(sizing.step(suspended, at).is_empty());
        }
        assert_eq!(sizing.preferred, preference);
        let mut snapped = fixture(monitor, 1.0, 1.0, egui::vec2(948.0, 1000.0));
        snapped.outer = egui::Rect::from_min_size(monitor.min, egui::vec2(960.0, 1032.0));
        assert!(settle(&mut sizing, snapped, 4.0).is_empty());
        assert_eq!(sizing.preferred, preference);
        let mut resized = normal;
        resized.inner = egui::vec2(900.0, 600.0);
        resized.outer =
            egui::Rect::from_min_size(normal.outer.min, resized.inner + egui::vec2(12.0, 32.0));
        assert!(settle(&mut sizing, resized, 5.0).is_empty());
        assert_eq!(sizing.preferred, Some(resized.inner));
    }
    #[test]
    fn popup_sizing_state_is_independent_and_initial_builder_size_stays_constant() {
        let ctx = egui::Context::default();
        let files = egui::ViewportId::from_hash_of("files-fixture");
        let editor = egui::ViewportId::from_hash_of("editor-fixture");
        let initial = builder(&ctx, files, "Files", egui::vec2(1100.0, 650.0));
        builder(&ctx, editor, "Editor", egui::vec2(1000.0, 720.0));
        let key = state_id(files);
        ctx.data_mut(|d| {
            let mut state = d.get_temp::<WindowSizing>(key).unwrap();
            state.preferred = Some(egui::vec2(740.0, 500.0));
            d.insert_temp(key, state);
        });
        let repaint = builder(&ctx, files, "Files", egui::vec2(1100.0, 650.0));
        assert_eq!(initial.inner_size, repaint.inner_size);
        assert_eq!(
            ctx.data(|d| d.get_temp::<WindowSizing>(key))
                .unwrap()
                .preferred,
            Some(egui::vec2(740.0, 500.0))
        );
        assert_eq!(
            ctx.data(|d| d.get_temp::<WindowSizing>(state_id(editor)))
                .unwrap()
                .preferred,
            Some(egui::vec2(1000.0, 720.0))
        );
    }
}
