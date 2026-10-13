//! Session types and enums

use ad_core::SessionId;
use ad_config::SessionType as ConfigSessionType;
use serde::{Deserialize, Serialize};

/// Session state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// Session is being created
    Creating,
    /// Session is connecting
    Connecting,
    /// Session is connected and active
    Connected,
    /// Session is disconnected but can reconnect
    Disconnected,
    /// Session is closing
    Closing,
    /// Session has an error
    Error(String),
}

/// Session type enumeration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionType {
    Ssh,
    Sftp,
    Rdp,
    Vnc,
    X11,
    Tunnel,
    LocalTerminal,
    RemoteApp,
}

impl From<&ConfigSessionType> for SessionType {
    fn from(config_type: &ConfigSessionType) -> Self {
        match config_type {
            ConfigSessionType::SSH { .. } => SessionType::Ssh,
            ConfigSessionType::SFTP { .. } => SessionType::Sftp,
            ConfigSessionType::RDP { .. } => SessionType::Rdp,
            ConfigSessionType::VNC { .. } => SessionType::Vnc,
            ConfigSessionType::X11 { .. } => SessionType::X11,
            ConfigSessionType::Tunnel { .. } => SessionType::Tunnel,
        }
    }
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: SessionId,
    pub name: String,
    pub session_type: SessionType,
    pub state: SessionState,
    pub tags: Vec<String>,
    pub folder: Option<String>,
}

impl SessionMetadata {
    pub fn new(id: SessionId, name: String, session_type: SessionType) -> Self {
        Self {
            id,
            name,
            session_type,
            state: SessionState::Creating,
            tags: Vec::new(),
            folder: None,
        }
    }
}
