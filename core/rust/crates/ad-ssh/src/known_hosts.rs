//! Known hosts management

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::debug;

/// Known hosts database
#[derive(Debug, Clone, Default)]
pub struct KnownHosts {
    hosts: HashMap<String, HostKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostKey {
    pub hostname: String,
    pub key_type: String,
    pub key_data: String,
}

impl KnownHosts {
    /// Load known hosts from the default location
    pub fn load_default() -> Self {
        let home_dir = dirs_next::home_dir().unwrap_or_default();
        let known_hosts_path = home_dir.join(".ssh").join("known_hosts");
        Self::load_from_file(&known_hosts_path)
    }

    /// Load known hosts from a file
    pub fn load_from_file(path: &Path) -> Self {
        let mut hosts = HashMap::new();

        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                if line.trim().is_empty() || line.starts_with('#') {
                    continue;
                }

                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let hostname = parts[0].to_string();
                    let key_type = parts[1].to_string();
                    let key_data = parts[2].to_string();

                    hosts.insert(
                        hostname.clone(),
                        HostKey {
                            hostname,
                            key_type,
                            key_data,
                        },
                    );
                }
            }
        }

        Self { hosts }
    }

    /// Save known hosts to a file
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        let mut content = String::new();

        for host in self.hosts.values() {
            content.push_str(&format!(
                "{} {} {}\n",
                host.hostname, host.key_type, host.key_data
            ));
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, content)?;
        Ok(())
    }

    /// Verify a host key
    pub fn verify(&self, hostname: &str, _key: Option<&str>) -> bool {
        // For now, simple hostname lookup
        // In production, would compare actual key data
        self.hosts.contains_key(hostname)
    }

    /// Add a new host key
    pub fn add_host(&mut self, host: HostKey) {
        debug!("Adding known host: {}", host.hostname);
        self.hosts.insert(host.hostname.clone(), host);
    }

    /// Remove a host
    pub fn remove_host(&mut self, hostname: &str) {
        debug!("Removing known host: {}", hostname);
        self.hosts.remove(hostname);
    }

    /// Get all known hosts
    pub fn get_hosts(&self) -> Vec<&HostKey> {
        self.hosts.values().collect()
    }
}

// Re-export dirs_next
use dirs_next;
