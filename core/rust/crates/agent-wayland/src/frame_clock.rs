//! Pace changed frames, not repeated software renders of an idle desktop.
use std::time::{Duration, Instant};

pub(crate) struct FrameClock {
    fps: u32,
    viewed: bool,
    mirror_viewed: bool,
    dirty: bool,
    rendered: Instant,
    callbacks: Instant,
}

impl FrameClock {
    pub fn new(fps: u32, now: Instant) -> Self {
        Self {
            fps: fps.clamp(1, 240),
            viewed: true,
            mirror_viewed: false,
            dirty: true,
            rendered: now - Duration::from_secs(1),
            callbacks: now - Duration::from_secs(1),
        }
    }
    fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.fps))
    }
    fn callback_interval(&self) -> Duration {
        if self.mirror_viewed {
            Duration::from_secs_f64(1.0 / f64::from(self.fps.max(30)))
        } else if self.viewed {
            self.interval()
        } else {
            Duration::from_secs(1)
        }
    }
    pub fn set_rate(&mut self, fps: u32) {
        self.fps = fps.clamp(1, 240);
    }
    pub fn viewed(&self) -> bool {
        self.viewed
    }
    pub fn set_mirror_viewed(&mut self, viewed: bool, now: Instant) {
        if viewed && !self.mirror_viewed {
            self.callbacks = now - Duration::from_secs(1);
        }
        self.mirror_viewed = viewed;
    }
    pub fn set_viewed(&mut self, viewed: bool, now: Instant) {
        if viewed && !self.viewed {
            self.rendered = now - Duration::from_secs(1);
            self.callbacks = self.rendered;
            self.dirty = true;
        }
        self.viewed = viewed;
    }
    pub fn damage(&mut self) {
        self.dirty = true;
    }
    pub fn render_due(&self, now: Instant) -> bool {
        self.viewed && self.dirty && now >= self.rendered + self.interval()
    }
    pub fn rendered(&mut self, started: Instant) {
        self.rendered = started;
        self.dirty = false;
    }
    pub fn callbacks_due(&self, now: Instant) -> bool {
        now >= self.callbacks + self.callback_interval()
    }
    pub fn sent_callbacks(&mut self, now: Instant) {
        self.callbacks = now;
    }
    pub fn timeout(&self, now: Instant) -> Duration {
        let mut deadline = self.callbacks + self.callback_interval();
        if self.viewed && self.dirty {
            deadline = deadline.min(self.rendered + self.interval());
        }
        deadline.saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_damage_wakes_immediately_without_rendering_unchanged_frames() {
        let now = Instant::now();
        let mut clock = FrameClock::new(30, now);
        clock.rendered(now);
        clock.sent_callbacks(now);
        assert!(!clock.render_due(now + Duration::from_secs(1)));
        clock.damage();
        assert!(clock.render_due(now + Duration::from_secs(1)));
        assert_eq!(clock.timeout(now + Duration::from_secs(1)), Duration::ZERO);
    }
    #[test]
    fn detached_damage_is_retained_and_resume_never_waits_for_an_idle_tick() {
        let now = Instant::now();
        let mut clock = FrameClock::new(30, now);
        clock.rendered(now);
        clock.sent_callbacks(now);
        clock.set_viewed(false, now);
        clock.damage();
        assert!(!clock.render_due(now + Duration::from_secs(5)));
        assert_eq!(clock.timeout(now), Duration::from_secs(1));
        assert!(clock.callbacks_due(now + Duration::from_secs(1)));
        clock.set_rate(5);
        clock.set_viewed(true, now + Duration::from_millis(150));
        assert!(clock.render_due(now + Duration::from_millis(150)));
    }
    #[test]
    fn busy_damage_obeys_the_rate_without_adding_render_time_to_each_period() {
        let now = Instant::now();
        let mut clock = FrameClock::new(30, now);
        clock.rendered(now);
        clock.damage();
        assert!(!clock.render_due(now + Duration::from_millis(32)));
        assert!(clock.render_due(now + Duration::from_millis(34)));
        clock.rendered(now + Duration::from_millis(34));
        clock.damage();
        assert!(!clock.render_due(now + Duration::from_millis(66)));
        assert!(clock.render_due(now + Duration::from_millis(68)));
        clock.set_rate(5);
        assert!(!clock.render_due(now + Duration::from_millis(100)));
    }
    #[test]
    fn independent_viewers_keep_callbacks_live_without_root_capture() {
        let now = Instant::now();
        let mut clock = FrameClock::new(30, now);
        clock.rendered(now);
        clock.sent_callbacks(now);
        clock.set_viewed(false, now);
        clock.damage();
        clock.set_rate(5); // A slow desktop link must not throttle another view.
        clock.set_mirror_viewed(true, now + Duration::from_millis(40));
        assert!(clock.callbacks_due(now + Duration::from_millis(40)));
        assert!(!clock.render_due(now + Duration::from_secs(1)));
        clock.sent_callbacks(now + Duration::from_millis(40));
        assert_eq!(
            clock.timeout(now + Duration::from_millis(40)),
            Duration::from_secs_f64(1.0 / 30.0)
        );
        clock.set_mirror_viewed(false, now + Duration::from_millis(40));
        assert_eq!(
            clock.timeout(now + Duration::from_millis(40)),
            Duration::from_secs(1)
        );
    }
}
