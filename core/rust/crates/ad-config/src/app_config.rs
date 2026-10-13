//! Application configuration types

use serde::{Deserialize, Serialize};

/// Application-wide configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Theme name (light, dark, system)
    pub theme: String,
    
    /// Font family for terminal
    pub terminal_font: String,
    
    /// Font size for terminal
    pub terminal_font_size: u16,
    
    /// Enable GPU acceleration for terminal rendering
    pub terminal_gpu_acceleration: bool,
    
    /// Default number of scrollback lines
    pub scrollback_lines: usize,
    
    /// Auto-save session layouts on exit
    pub auto_save_layouts: bool,
    
    /// Check for updates on startup
    pub check_updates: bool,
    
    /// Send anonymous telemetry data
    pub telemetry_enabled: bool,
    
    /// SSH connection timeout in seconds
    pub ssh_timeout_secs: u64,
    
    /// Enable SSH compression
    pub ssh_compression: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            terminal_font: "Consolas".to_string(),
            terminal_font_size: 14,
            terminal_gpu_acceleration: true,
            scrollback_lines: 10000,
            auto_save_layouts: true,
            check_updates: true,
            telemetry_enabled: false,
            ssh_timeout_secs: 30,
            ssh_compression: false,
        }
    }
}
