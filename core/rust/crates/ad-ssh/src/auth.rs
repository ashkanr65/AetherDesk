//! SSH authentication methods

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Authentication method for SSH connections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    /// Password authentication
    Password(String),

    /// Public key authentication with key file path
    PublicKey {
        key_path: PathBuf,
        passphrase: Option<String>,
    },

    /// Use SSH agent
    Agent,

    /// Keyboard interactive
    KeyboardInteractive,

    /// Certificate-based authentication
    Certificate {
        cert_path: PathBuf,
        key_path: PathBuf,
    },
}

impl AuthMethod {
    /// Create a password auth method
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password(password.into())
    }

    /// Create a public key auth method
    pub fn public_key(key_path: PathBuf, passphrase: Option<String>) -> Self {
        Self::PublicKey {
            key_path,
            passphrase,
        }
    }

    /// Create an SSH agent auth method
    pub fn agent() -> Self {
        Self::Agent
    }
}
