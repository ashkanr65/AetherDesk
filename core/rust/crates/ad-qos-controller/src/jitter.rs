use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use thiserror::Error;

pub trait TimedFrame {
    fn sequence(&self) -> u64;
    fn presentation_time_us(&self) -> u64;
}

#[derive(Clone, Copy, Debug)]
pub struct JitterConfig {
    pub minimum_delay: Duration,
    pub maximum_delay: Duration,
    pub maximum_frames: usize,
    pub late_tolerance: Duration,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            minimum_delay: Duration::from_millis(2),
            maximum_delay: Duration::from_millis(45),
            maximum_frames: 6,
            late_tolerance: Duration::from_millis(12),
        }
    }
}

impl JitterConfig {
    fn validate(self) -> Result<Self, JitterError> {
        if self.minimum_delay > self.maximum_delay || self.maximum_frames == 0 {
            return Err(JitterError::InvalidConfiguration);
        }
        Ok(self)
    }
}

#[derive(Debug)]
pub struct FramePacer {
    remote_anchor_us: u64,
    local_anchor: Instant,
    delay: Duration,
}

impl FramePacer {
    pub fn new(remote_anchor_us: u64, local_anchor: Instant, delay: Duration) -> Self {
        Self {
            remote_anchor_us,
            local_anchor,
            delay,
        }
    }

    pub fn deadline(&self, presentation_time_us: u64) -> Instant {
        let remote_delta = presentation_time_us.saturating_sub(self.remote_anchor_us);
        self.local_anchor + Duration::from_micros(remote_delta) + self.delay
    }

    pub fn set_delay(&mut self, delay: Duration) {
        self.delay = delay;
    }
}

pub struct JitterBuffer<T: TimedFrame> {
    config: JitterConfig,
    queued: BTreeMap<(u64, u64), T>,
    pacer: Option<FramePacer>,
    previous_transit_us: Option<i128>,
    jitter_us: f64,
    dropped: u64,
}

impl<T: TimedFrame> JitterBuffer<T> {
    pub fn new(config: JitterConfig) -> Result<Self, JitterError> {
        Ok(Self {
            config: config.validate()?,
            queued: BTreeMap::new(),
            pacer: None,
            previous_transit_us: None,
            jitter_us: 0.0,
            dropped: 0,
        })
    }

    pub fn push(&mut self, frame: T, arrival: Instant) {
        let presentation = frame.presentation_time_us();
        if self.pacer.is_none() {
            self.pacer = Some(FramePacer::new(
                presentation,
                arrival,
                self.config.minimum_delay,
            ));
        }
        let local_us = self
            .pacer
            .as_ref()
            .map(|pacer| arrival.duration_since(pacer.local_anchor).as_micros() as i128)
            .unwrap_or(0);
        let remote_us = presentation.saturating_sub(
            self.pacer
                .as_ref()
                .map(|pacer| pacer.remote_anchor_us)
                .unwrap_or(presentation),
        ) as i128;
        let transit = local_us - remote_us;
        if let Some(previous) = self.previous_transit_us {
            let variation = (transit - previous).unsigned_abs() as f64;
            self.jitter_us += (variation - self.jitter_us) / 16.0;
        }
        self.previous_transit_us = Some(transit);
        let desired = Duration::from_micros((self.jitter_us * 2.5) as u64)
            .max(self.config.minimum_delay)
            .min(self.config.maximum_delay);
        if let Some(pacer) = &mut self.pacer {
            pacer.set_delay(desired);
        }
        self.queued.insert((presentation, frame.sequence()), frame);
        while self.queued.len() > self.config.maximum_frames {
            self.queued.pop_first();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub fn pop_ready(&mut self, now: Instant) -> Option<T> {
        let key = *self.queued.first_key_value()?.0;
        let deadline = self.pacer.as_ref()?.deadline(key.0);
        if now < deadline {
            return None;
        }
        let newest_ready = self
            .queued
            .keys()
            .copied()
            .take_while(|(presentation, _)| {
                self.pacer
                    .as_ref()
                    .is_some_and(|pacer| pacer.deadline(*presentation) <= now)
            })
            .last()?;
        let stale: Vec<_> = self
            .queued
            .keys()
            .copied()
            .take_while(|key| *key < newest_ready)
            .collect();
        for key in stale {
            self.queued.remove(&key);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.queued.remove(&newest_ready)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let presentation = self.queued.first_key_value()?.0 .0;
        Some(self.pacer.as_ref()?.deadline(presentation))
    }

    pub fn target_delay(&self) -> Duration {
        self.pacer
            .as_ref()
            .map(|pacer| pacer.delay)
            .unwrap_or(self.config.minimum_delay)
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.pacer = None;
        self.previous_transit_us = None;
        self.jitter_us = 0.0;
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum JitterError {
    #[error("invalid jitter-buffer configuration")]
    InvalidConfiguration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct Frame(u64, u64);

    impl TimedFrame for Frame {
        fn sequence(&self) -> u64 {
            self.0
        }
        fn presentation_time_us(&self) -> u64 {
            self.1
        }
    }

    #[test]
    fn presents_the_freshest_ready_frame() {
        let start = Instant::now();
        let mut buffer = JitterBuffer::new(JitterConfig::default()).unwrap();
        buffer.push(Frame(1, 0), start);
        buffer.push(Frame(2, 16_667), start + Duration::from_millis(17));
        assert_eq!(
            buffer.pop_ready(start + Duration::from_millis(40)),
            Some(Frame(2, 16_667))
        );
        assert_eq!(buffer.dropped_frames(), 1);
    }

    #[test]
    fn caps_latency_by_dropping_old_frames() {
        let start = Instant::now();
        let mut buffer = JitterBuffer::new(JitterConfig {
            maximum_frames: 2,
            ..JitterConfig::default()
        })
        .unwrap();
        for sequence in 0..5 {
            buffer.push(Frame(sequence, sequence * 1_000), start);
        }
        assert_eq!(buffer.queued.len(), 2);
        assert_eq!(buffer.dropped_frames(), 3);
    }
}
