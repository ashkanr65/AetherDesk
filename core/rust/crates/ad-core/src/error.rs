//! Core error types for AetherDesk

use thiserror::Error;

/// Core error type for AetherDesk operations
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("SSH error: {0}")]
    SshError(#[from] russh::Error),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

/// Result type alias using CoreError
pub type Result<T> = std::result::Result<T, CoreError>;
