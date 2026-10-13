//! Session manager implementation

use ad_core::{Result as CoreResult, SessionId};
use ad_events::EventBus;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, error};

use crate::types::{SessionMetadata, SessionState, SessionType};
use crate::session::Session;

/// Manages all active sessions in the application
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, Arc<Session>>>>,
    event_bus: Arc<EventBus>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_bus,
        }
    }

    /// Create a new session
    pub fn create_session(&self, metadata: SessionMetadata) -> CoreResult<Arc<Session>> {
        let session_id = metadata.id;
        info!("Creating new session: {} ({:?})", metadata.name, metadata.session_type);

        let session = Arc::new(Session::new(metadata, self.event_bus.clone()));
        
        self.sessions.write().insert(session_id, session.clone());
        self.event_bus.emit_session_created(session_id, format!("{:?}", session.session_type()));
        
        Ok(session)
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &SessionId) -> Option<Arc<Session>> {
        self.sessions.read().get(session_id).cloned()
    }

    /// Get all sessions
    pub fn get_all_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions.read().values().cloned().collect()
    }

    /// Get sessions by type
    pub fn get_sessions_by_type(&self, session_type: SessionType) -> Vec<Arc<Session>> {
        self.sessions
            .read()
            .values()
            .filter(|s| s.session_type() == session_type)
            .cloned()
            .collect()
    }

    /// Connect a session
    pub async fn connect_session(&self, session_id: &SessionId) -> CoreResult<()> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| ad_core::CoreError::NotFound(format!("Session {:?} not found", session_id)))?;

        info!("Connecting session: {:?}", session_id);
        session.connect().await?;
        
        Ok(())
    }

    /// Disconnect a session
    pub async fn disconnect_session(&self, session_id: &SessionId) -> CoreResult<()> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| ad_core::CoreError::NotFound(format!("Session {:?} not found", session_id)))?;

        info!("Disconnecting session: {:?}", session_id);
        session.disconnect().await?;
        
        Ok(())
    }

    /// Close and remove a session
    pub fn close_session(&self, session_id: &SessionId) -> CoreResult<()> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| ad_core::CoreError::NotFound(format!("Session {:?} not found", session_id)))?;

        info!("Closing session: {:?}", session_id);
        
        // Update state
        session.set_state(SessionState::Closing);
        
        // Remove from manager
        self.sessions.write().remove(session_id);
        
        self.event_bus.emit_session_closed(*session_id);
        
        Ok(())
    }

    /// Get count of active sessions
    pub fn session_count(&self) -> usize {
        self.sessions.read().len()
    }

    /// Clear all sessions
    pub fn clear_all(&self) {
        let session_ids: Vec<_> = self.sessions.read().keys().copied().collect();
        for id in session_ids {
            let _ = self.close_session(&id);
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(Arc::new(EventBus::default()))
    }
}
