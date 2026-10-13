//! Event types for AetherDesk

use ad_core::SessionId;
use serde::{Deserialize, Serialize};

/// Events that can be emitted by the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// Session lifecycle events
    SessionCreated { session_id: SessionId, session_type: String },
    SessionConnected { session_id: SessionId },
    SessionDisconnected { session_id: SessionId },
    SessionClosed { session_id: SessionId },
    SessionError { session_id: SessionId, error: String },
    
    /// Terminal events
    TerminalData { session_id: SessionId, data: Vec<u8> },
    TerminalResize { session_id: SessionId, cols: u16, rows: u16 },
    
    /// SFTP events
    FileTransferStarted { transfer_id: String, filename: String },
    FileTransferProgress { transfer_id: String, bytes_transferred: u64, total_bytes: u64 },
    FileTransferCompleted { transfer_id: String, success: bool },
    
    /// Tunnel events
    TunnelOpened { local_port: u16, remote_host: String, remote_port: u16 },
    TunnelClosed { local_port: u16 },
    
    /// Authentication events
    AuthRequired { session_id: SessionId, auth_methods: Vec<String> },
    AuthSuccess { session_id: SessionId },
    AuthFailed { session_id: SessionId, reason: String },
    
    /// Application events
    SettingsChanged,
    ThemeChanged { theme: String },
    PluginLoaded { plugin_name: String },
    PluginError { plugin_name: String, error: String },
}
