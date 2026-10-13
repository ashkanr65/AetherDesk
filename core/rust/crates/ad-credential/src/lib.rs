//! Credential Management Module
//! 
//! Handles secure storage and retrieval of authentication credentials

use std::collections::HashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn, error};

/// Errors that can occur during credential operations
#[derive(Error, Debug)]
pub enum CredentialError {
    #[error("Credential not found: {0}")]
    NotFound(String),
    #[error("Encryption error: {0}")]
    EncryptionError(String),
    #[error("Decryption error: {0}")]
    DecryptionError(String),
    #[error("Invalid credential format: {0}")]
    InvalidFormat(String),
}

/// Represents different types of credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CredentialType {
    Password(String),
    PrivateKey(String),
    Certificate(String),
    Token(String),
}

/// A stored credential with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub name: String,
    pub credential_type: CredentialType,
    pub username: Option<String>,
    pub host: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub tags: Vec<String>,
}

impl Credential {
    pub fn new(id: String, name: String, credential_type: CredentialType) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id,
            name,
            credential_type,
            username: None,
            host: None,
            created_at: now,
            updated_at: now,
            tags: Vec::new(),
        }
    }

    pub fn with_username(mut self, username: String) -> Self {
        self.username = Some(username);
        self
    }

    pub fn with_host(mut self, host: String) -> Self {
        self.host = Some(host);
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// Credential manager for secure storage and retrieval
pub struct CredentialManager {
    credentials: RwLock<HashMap<String, Credential>>,
}

impl CredentialManager {
    pub fn new() -> Self {
        Self {
            credentials: RwLock::new(HashMap::new()),
        }
    }

    /// Add a new credential
    pub fn add(&self, credential: Credential) -> Result<(), CredentialError> {
        let mut creds = self.credentials.write();
        info!("Adding credential: {}", credential.name);
        creds.insert(credential.id.clone(), credential);
        Ok(())
    }

    /// Get a credential by ID
    pub fn get(&self, id: &str) -> Result<Credential, CredentialError> {
        let creds = self.credentials.read();
        creds
            .get(id)
            .cloned()
            .ok_or_else(|| CredentialError::NotFound(id.to_string()))
    }

    /// Remove a credential by ID
    pub fn remove(&self, id: &str) -> Result<Credential, CredentialError> {
        let mut creds = self.credentials.write();
        creds
            .remove(id)
            .ok_or_else(|| CredentialError::NotFound(id.to_string()))
    }

    /// List all credentials
    pub fn list(&self) -> Vec<Credential> {
        let creds = self.credentials.read();
        creds.values().cloned().collect()
    }

    /// Find credentials by tag
    pub fn find_by_tag(&self, tag: &str) -> Vec<Credential> {
        let creds = self.credentials.read();
        creds
            .values()
            .filter(|c| c.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// Update a credential
    pub fn update(&self, credential: Credential) -> Result<(), CredentialError> {
        let mut creds = self.credentials.write();
        if !creds.contains_key(&credential.id) {
            return Err(CredentialError::NotFound(credential.id));
        }
        info!("Updating credential: {}", credential.name);
        creds.insert(credential.id.clone(), credential);
        Ok(())
    }
}

impl Default for CredentialManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_creation() {
        let cred = Credential::new(
            "test-id".to_string(),
            "Test Credential".to_string(),
            CredentialType::Password("secret".to_string()),
        );
        assert_eq!(cred.id, "test-id");
        assert_eq!(cred.name, "Test Credential");
    }

    #[test]
    fn test_credential_manager() {
        let manager = CredentialManager::new();
        let cred = Credential::new(
            "test-id".to_string(),
            "Test Credential".to_string(),
            CredentialType::Password("secret".to_string()),
        );
        
        manager.add(cred.clone()).unwrap();
        let retrieved = manager.get("test-id").unwrap();
        assert_eq!(retrieved.id, cred.id);
        
        let list = manager.list();
        assert_eq!(list.len(), 1);
        
        manager.remove("test-id").unwrap();
        assert!(manager.get("test-id").is_err());
    }
}
