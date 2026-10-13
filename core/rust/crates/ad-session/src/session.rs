//! Session implementation

use ad_core::{Result as CoreResult, SessionId};
use ad_events::EventBus;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{info, debug, error};

use crate::types::{SessionMetadata, SessionState, SessionType};

/// Represents an active session
pub struct Session {
    metadata: RwLock<SessionMetadata>,
    event_bus: Arc<EventBus>,
}

impl Session {
    /// Create a new session
    pub fn new(metadata: SessionMetadata, event_bus: Arc<EventBus>) -> Self {
        Self {
            metadata: RwLock::new(metadata),
            event_bus,
        }
    }

    /// Get the session ID
    pub fn id(&self) -> SessionId {
        self.metadata.read().id
    }

    /// Get the session name
    pub fn name(&self) -> String {
        self.metadata.read().name.clone()
    }

    /// Get the session type
    pub fn session_type(&self) -> SessionType {
        self.metadata.read().session_type.clone()
    }

    /// Get the current state
    pub fn state(&self) -> SessionState {
        self.metadata.read().state.clone()
    }

    /// Set the session state
    pub fn set_state(&self, state: SessionState) {
        let mut metadata = self.metadata.write();
        metadata.state = state;
    }

    /// Connect the session
    pub async fn connect(&self) -> CoreResult<()> {
        info!("Connecting session: {}", self.name());
        self.set_state(SessionState::Connecting);
        
        // Actual connection logic would be implemented here
        // based on session type (SSH, RDP, VNC, etc.)
        
        self.set_state(SessionState::Connected);
        self.event_bus.emit_session_connected(self.id());
        
        Ok(())
    }

    /// Disconnect the session
    pub async fn disconnect(&self) -> CoreResult<()> {
        info!("Disconnecting session: {}", self.name());
        
        if self.state() == SessionState::Connected {
            self.set_state(SessionState::Disconnected);
            self.event_bus.emit_session_disconnected(self.id());
        }
        
        Ok(())
    }

    /// Get metadata
    pub fn metadata(&self) -> SessionMetadata {
        self.metadata.read().clone()
    }

    /// Update metadata
    pub fn update_metadata(&self, name: Option<String>, tags: Option<Vec<String>>, folder: Option<String>) {
        let mut metadata = self.metadata.write();
        if let Some(n) = name {
            metadata.name = n;
        }
        if let Some(t) = tags {
            metadata.tags = t;
        }
        if let Some(f) = folder {
            metadata.folder = Some(f);
        }
    }
}
