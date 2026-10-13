//! Low-latency presentation timing for Aether Stream.

mod jitter;

pub use jitter::{FramePacer, JitterBuffer, JitterConfig, JitterError, TimedFrame};
