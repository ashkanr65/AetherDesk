//! Configuration manager for AetherDesk

use ad_core::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Main configuration manager
pub struct ConfigManager {
    config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub app: crate::AppConfig,
    pub sessions: Vec<crate::SessionConfig>,
}

impl ConfigManager {
    /// Create a new ConfigManager with the default config path
    pub fn new() -> Result<Self> {
        let config_path = Self::get_default_config_path()?;
        Ok(Self { config_path })
    }

    /// Create a new ConfigManager with a custom config path
    pub fn with_path(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    /// Get the default configuration path
    fn get_default_config_path() -> Result<PathBuf> {
        // On Windows: %APPDATA%\AetherDesk\config.json
        // On Linux: ~/.config/aetherdesk/config.json
        // On macOS: ~/Library/Application Support/AetherDesk/config.json
        let config_dir = dirs_next::config_dir()
            .ok_or_else(|| ad_core::CoreError::Unknown("Could not determine config directory".to_string()))?;
        
        let config_path = config_dir.join("aetherdesk").join("config.json");
        Ok(config_path)
    }

    /// Load configuration from file
    pub fn load(&self) -> Result<Config> {
        info!("Loading configuration from {:?}", self.config_path);
        
        if !self.config_path.exists() {
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&self.config_path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to file
    pub fn save(&self, config: &Config) -> Result<()> {
        info!("Saving configuration to {:?}", self.config_path);
        
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(config)?;
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app: crate::AppConfig::default(),
            sessions: Vec::new(),
        }
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new().expect("Failed to create default ConfigManager")
    }
}

// Re-export dirs_next for path resolution
use dirs_next;
