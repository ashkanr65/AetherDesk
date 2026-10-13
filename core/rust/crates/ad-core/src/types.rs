//! Core type definitions for AetherDesk

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique identifier for a connection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionId(Uuid);

impl ConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Message types for internal communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Request to establish a new session
    NewSession { session_type: String },
    /// Session established successfully
    SessionEstablished { session_id: SessionId },
    /// Session closed
    SessionClosed { session_id: SessionId },
    /// Error occurred
    Error { message: String },
    /// Data received
    Data { payload: Vec<u8> },
    /// Control command
    Command { cmd: String, args: Vec<String> },
}
