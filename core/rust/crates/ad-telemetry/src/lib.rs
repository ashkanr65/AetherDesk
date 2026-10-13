//! Telemetry Module
//! 
//! Collects and reports usage analytics and performance metrics

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum TelemetryError {
    #[error("Collection error: {0}")]
    CollectionError(String),
    #[error("Transmission error: {0}")]
    TransmissionError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub event_type: String,
    pub timestamp: i64,
    pub data: serde_json::Value,
}

pub struct TelemetryCollector {
    enabled: bool,
    events: Vec<TelemetryEvent>,
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self {
            enabled: true,
            events: Vec::new(),
        }
    }

    pub fn collect(&mut self, event: TelemetryEvent) {
        if self.enabled {
            info!("Collecting event: {}", event.event_type);
            self.events.push(event);
        }
    }

    pub fn flush(&mut self) -> Vec<TelemetryEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

impl Default for TelemetryCollector {
    fn default() -> Self {
        Self::new()
    }
}
