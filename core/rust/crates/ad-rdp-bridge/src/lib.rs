//! In-process RDP engine boundary.
//!
//! The Windows adapter will implement this interface with FreeRDP callbacks;
//! no external desktop process is part of this contract.

use ad_display::{ClipboardPayload, DisplayInput, SurfaceFrame};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Remains false until the linked build exports the FreeRDP adapter and Flutter
/// texture surface. UI capability reporting must use this value.
pub const EMBEDDED_ENGINE_AVAILABLE: bool = false;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RdpClientConfig {
    pub host: String,
    pub port: u16,
    pub width: u32,
    pub height: u32,
    pub scale_percent: u32,
    pub dynamic_resolution: bool,
    pub multi_monitor: bool,
    pub clipboard: bool,
    pub audio: bool,
    pub security: RdpSecurityPolicy,
}

impl RdpClientConfig {
    pub fn validate(&self) -> Result<(), RdpError> {
        if self.host.trim().is_empty() {
            return Err(RdpError::InvalidConfiguration("host is empty"));
        }
        if self.port == 0 {
            return Err(RdpError::InvalidConfiguration("port is zero"));
        }
        if self.width < 320 || self.height < 200 {
            return Err(RdpError::InvalidConfiguration(
                "desktop dimensions are below the supported minimum",
            ));
        }
        if !(100..=500).contains(&self.scale_percent) {
            return Err(RdpError::InvalidConfiguration(
                "display scale must be between 100 and 500 percent",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RdpSecurityPolicy {
    pub require_tls: bool,
    pub require_nla: bool,
    pub verify_certificate: bool,
    pub allow_legacy_rdp_security: bool,
}

impl Default for RdpSecurityPolicy {
    fn default() -> Self {
        Self {
            require_tls: true,
            require_nla: true,
            verify_certificate: true,
            allow_legacy_rdp_security: false,
        }
    }
}

pub struct RdpCredentials {
    pub username: String,
    secret: Vec<u8>,
}

impl RdpCredentials {
    pub fn new(username: impl Into<String>, secret: impl Into<Vec<u8>>) -> Self {
        Self {
            username: username.into(),
            secret: secret.into(),
        }
    }

    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
}

impl fmt::Debug for RdpCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RdpCredentials")
            .field("username", &self.username)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl Drop for RdpCredentials {
    fn drop(&mut self) {
        self.secret.fill(0);
    }
}

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn credentials(&self) -> Result<RdpCredentials, RdpError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RdpSessionState {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug)]
pub enum RdpEvent {
    StateChanged(RdpSessionState),
    Frame(SurfaceFrame),
    Clipboard(ClipboardPayload),
    Audio(Vec<u8>),
    Diagnostic { code: u32, message: String },
}

#[async_trait]
pub trait RdpEngine: Send {
    async fn connect(
        &mut self,
        config: RdpClientConfig,
        credentials: &dyn CredentialProvider,
    ) -> Result<(), RdpError>;
    async fn next_event(&mut self) -> Result<RdpEvent, RdpError>;
    async fn send_input(&mut self, input: DisplayInput) -> Result<(), RdpError>;
    async fn send_clipboard(&mut self, payload: ClipboardPayload) -> Result<(), RdpError>;
    async fn disconnect(&mut self) -> Result<(), RdpError>;
}

#[derive(Debug, Error)]
pub enum RdpError {
    #[error("invalid RDP configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("the embedded FreeRDP engine is not linked into this build")]
    EngineUnavailable,
    #[error("RDP authentication failed")]
    AuthenticationFailed,
    #[error("RDP certificate verification failed: {0}")]
    CertificateRejected(String),
    #[error("RDP transport failed: {0}")]
    Transport(String),
    #[error("RDP session was closed")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RdpClientConfig {
        RdpClientConfig {
            host: "127.0.0.1".into(),
            port: 3389,
            width: 1920,
            height: 1080,
            scale_percent: 100,
            dynamic_resolution: true,
            multi_monitor: false,
            clipboard: true,
            audio: true,
            security: RdpSecurityPolicy::default(),
        }
    }

    #[test]
    fn secure_defaults_disable_legacy_security() {
        let policy = RdpSecurityPolicy::default();
        assert!(policy.require_tls);
        assert!(policy.require_nla);
        assert!(policy.verify_certificate);
        assert!(!policy.allow_legacy_rdp_security);
    }

    #[test]
    fn validates_surface_size_and_scale() {
        assert!(config().validate().is_ok());
        let mut invalid = config();
        invalid.width = 0;
        assert!(matches!(
            invalid.validate(),
            Err(RdpError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn credentials_do_not_leak_through_debug() {
        const SECRET_VALUE: &str = "aetherdesk-test-credential-7f3c91";
        let credentials =
            RdpCredentials::new("operator", SECRET_VALUE.as_bytes().to_vec());
        let output = format!("{credentials:?}");
        assert!(!output.contains(SECRET_VALUE));
        assert!(output.contains("REDACTED"));
    }
}
