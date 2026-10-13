use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct NetworkObservation {
    pub interval: Duration,
    pub delivered_bytes: u64,
    pub delivered_packets: u32,
    pub lost_packets: u32,
    pub rtt: Duration,
    pub jitter: Duration,
}

#[derive(Clone, Debug)]
pub struct BandwidthEstimator {
    bandwidth_bps: f64,
    loss_ratio: f64,
    rtt: Duration,
    jitter: Duration,
    initialized: bool,
}

impl Default for BandwidthEstimator {
    fn default() -> Self {
        Self {
            bandwidth_bps: 0.0,
            loss_ratio: 0.0,
            rtt: Duration::ZERO,
            jitter: Duration::ZERO,
            initialized: false,
        }
    }
}

impl BandwidthEstimator {
    const FAST_ALPHA: f64 = 0.35;
    const SLOW_ALPHA: f64 = 0.12;

    pub fn observe(&mut self, sample: NetworkObservation) {
        let seconds = sample.interval.as_secs_f64();
        if seconds <= 0.0 {
            return;
        }
        let instantaneous_bps = sample.delivered_bytes as f64 * 8.0 / seconds;
        let total_packets = sample.delivered_packets.saturating_add(sample.lost_packets);
        let instantaneous_loss = if total_packets == 0 {
            0.0
        } else {
            sample.lost_packets as f64 / total_packets as f64
        };

        if !self.initialized {
            self.bandwidth_bps = instantaneous_bps;
            self.loss_ratio = instantaneous_loss;
            self.rtt = sample.rtt;
            self.jitter = sample.jitter;
            self.initialized = true;
            return;
        }

        let bandwidth_alpha = if instantaneous_bps < self.bandwidth_bps {
            Self::FAST_ALPHA
        } else {
            Self::SLOW_ALPHA
        };
        self.bandwidth_bps = ewma(self.bandwidth_bps, instantaneous_bps, bandwidth_alpha);
        self.loss_ratio = ewma(self.loss_ratio, instantaneous_loss, Self::FAST_ALPHA);
        self.rtt = ewma_duration(self.rtt, sample.rtt, Self::FAST_ALPHA);
        self.jitter = ewma_duration(self.jitter, sample.jitter, Self::FAST_ALPHA);
    }

    pub fn available_bitrate_bps(&self) -> u64 {
        (self.bandwidth_bps * 0.85).max(0.0) as u64
    }

    pub fn loss_ratio(&self) -> f64 {
        self.loss_ratio
    }

    pub fn rtt(&self) -> Duration {
        self.rtt
    }

    pub fn jitter(&self) -> Duration {
        self.jitter
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QosLimits {
    pub minimum_bitrate_bps: u64,
    pub maximum_bitrate_bps: u64,
    pub minimum_fps: u16,
    pub maximum_fps: u16,
}

impl QosLimits {
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.minimum_bitrate_bps == 0 || self.maximum_bitrate_bps < self.minimum_bitrate_bps {
            return Err("invalid bitrate limits");
        }
        if self.minimum_fps == 0 || self.maximum_fps < self.minimum_fps {
            return Err("invalid frame-rate limits");
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QosDecision {
    pub target_bitrate_bps: u64,
    pub target_fps: u16,
    pub frame_interval: Duration,
    pub request_key_frame: bool,
}

pub struct QosController {
    limits: QosLimits,
    target_bitrate_bps: u64,
    target_fps: u16,
    consecutive_healthy_samples: u8,
}

impl QosController {
    pub fn new(limits: QosLimits) -> Result<Self, &'static str> {
        let limits = limits.validate()?;
        Ok(Self {
            target_bitrate_bps: limits.maximum_bitrate_bps,
            target_fps: limits.maximum_fps,
            limits,
            consecutive_healthy_samples: 0,
        })
    }

    pub fn update(&mut self, estimator: &BandwidthEstimator) -> QosDecision {
        if !estimator.is_initialized() {
            return self.decision(false);
        }

        let loss = estimator.loss_ratio();
        let rtt = estimator.rtt();
        let severe_congestion = loss >= 0.10 || rtt >= Duration::from_millis(350);
        let moderate_congestion = loss >= 0.03 || rtt >= Duration::from_millis(180);
        let request_key_frame = severe_congestion;

        if severe_congestion {
            self.target_bitrate_bps = self.target_bitrate_bps.saturating_mul(65) / 100;
            self.target_fps = ((self.target_fps as u32 * 70) / 100) as u16;
            self.consecutive_healthy_samples = 0;
        } else if moderate_congestion {
            self.target_bitrate_bps = self.target_bitrate_bps.saturating_mul(82) / 100;
            self.target_fps = ((self.target_fps as u32 * 85) / 100) as u16;
            self.consecutive_healthy_samples = 0;
        } else {
            self.consecutive_healthy_samples = self.consecutive_healthy_samples.saturating_add(1);
            if self.consecutive_healthy_samples >= 3 {
                self.target_bitrate_bps = self.target_bitrate_bps.saturating_mul(108) / 100;
                self.target_fps = self.target_fps.saturating_add(2);
                self.consecutive_healthy_samples = 0;
            }
        }

        let estimated_limit = estimator
            .available_bitrate_bps()
            .max(self.limits.minimum_bitrate_bps);
        self.target_bitrate_bps = self.target_bitrate_bps.min(estimated_limit).clamp(
            self.limits.minimum_bitrate_bps,
            self.limits.maximum_bitrate_bps,
        );
        self.target_fps = self
            .target_fps
            .clamp(self.limits.minimum_fps, self.limits.maximum_fps);
        self.decision(request_key_frame)
    }

    fn decision(&self, request_key_frame: bool) -> QosDecision {
        QosDecision {
            target_bitrate_bps: self.target_bitrate_bps,
            target_fps: self.target_fps,
            frame_interval: Duration::from_secs_f64(1.0 / f64::from(self.target_fps)),
            request_key_frame,
        }
    }
}

fn ewma(previous: f64, current: f64, alpha: f64) -> f64 {
    previous + alpha * (current - previous)
}

fn ewma_duration(previous: Duration, current: Duration, alpha: f64) -> Duration {
    Duration::from_secs_f64(ewma(previous.as_secs_f64(), current.as_secs_f64(), alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn congestion_reduces_bitrate_and_frame_rate() {
        let mut estimator = BandwidthEstimator::default();
        estimator.observe(NetworkObservation {
            interval: Duration::from_secs(1),
            delivered_bytes: 500_000,
            delivered_packets: 90,
            lost_packets: 20,
            rtt: Duration::from_millis(400),
            jitter: Duration::from_millis(40),
        });
        let mut controller = QosController::new(QosLimits {
            minimum_bitrate_bps: 500_000,
            maximum_bitrate_bps: 12_000_000,
            minimum_fps: 10,
            maximum_fps: 60,
        })
        .unwrap();
        let decision = controller.update(&estimator);
        assert!(decision.target_bitrate_bps < 12_000_000);
        assert!(decision.target_fps < 60);
        assert!(decision.request_key_frame);
    }
}
