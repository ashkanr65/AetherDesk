//! AetherDesk Configuration Library
//!
//! Provides configuration management for sessions, credentials, and application settings.

pub mod config;
pub mod session_config;
pub mod app_config;
pub mod ssh_config;

pub use config::ConfigManager;
pub use session_config::{SessionConfig, SessionType};
pub use app_config::AppConfig;
pub use ssh_config::{SshConfig, X11ForwardingConfig};
