//! Plugin Host Module
//! 
//! Provides plugin system for extending AetherDesk functionality

use std::collections::HashMap;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Error, Debug)]
pub enum PluginError {
    #[error("Plugin not found: {0}")]
    NotFound(String),
    #[error("Plugin load error: {0}")]
    LoadError(String),
    #[error("Plugin execution error: {0}")]
    ExecutionError(String),
}

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

pub struct PluginHost {
    plugins: HashMap<String, PluginInfo>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn register(&mut self, info: PluginInfo) -> Result<(), PluginError> {
        info!("Registering plugin: {}", info.name);
        self.plugins.insert(info.id.clone(), info);
        Ok(())
    }

    pub fn enable(&mut self, id: &str) -> Result<(), PluginError> {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.enabled = true;
            info!("Enabled plugin: {}", plugin.name);
            Ok(())
        } else {
            Err(PluginError::NotFound(id.to_string()))
        }
    }

    pub fn disable(&mut self, id: &str) -> Result<(), PluginError> {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.enabled = false;
            info!("Disabled plugin: {}", plugin.name);
            Ok(())
        } else {
            Err(PluginError::NotFound(id.to_string()))
        }
    }

    pub fn list(&self) -> Vec<&PluginInfo> {
        self.plugins.values().collect()
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}
