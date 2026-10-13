//! Session configuration types

use ad_core::SessionId;
use serde::{Deserialize, Serialize};

/// Type of session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionType {
    SSH { host: String, port: u16, username: String },
    SFTP { host: String, port: u16, username: String },
    RDP { host: String, port: u16 },
    VNC { host: String, port: u16 },
    X11 { host: String, port: u16, display: String },
    Tunnel { local_port: u16, remote_host: String, remote_port: u16 },
}

/// Session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: SessionId,
    pub name: String,
    pub session_type: SessionType,
    pub tags: Vec<String>,
    pub folder: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

impl SessionConfig {
    pub fn new(name: String, session_type: SessionType) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: SessionId::new(),
            name,
            session_type,
            tags: Vec::new(),
            folder: None,
            created_at: now,
            modified_at: now,
        }
    }
}
