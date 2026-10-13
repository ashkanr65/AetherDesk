//! SSH-specific configuration

use serde::{Deserialize, Serialize};

/// Runtime-only X11 forwarding parameters. The cookie is generated for an
/// ephemeral local X server and must never be written to a profile.
#[derive(Debug, Clone)]
pub struct X11ForwardingConfig {
    pub local_port: u16,
    pub authentication_protocol: String,
    /// Random cookie advertised to the remote SSH server.
    pub authentication_cookie: String,
    /// VcXsrv's real cookie, used only after validating and rewriting the
    /// forwarded X11 setup packet on the local machine.
    pub local_authentication_cookie: String,
    pub screen_number: u32,
}

/// SSH connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
    /// Path to a WireGuard client config; secrets are never copied into profiles.
    #[serde(default)]
    pub wireguard_config_path: Option<String>,
    /// Passphrase used to decrypt the private key. This field is runtime-only
    /// for desktop clients and must never be serialized into a saved profile.
    #[serde(default, skip_serializing)]
    pub key_passphrase: Option<String>,
    /// Unknown host keys are refused unless the user explicitly confirmed the
    /// fingerprint for this connection attempt.
    #[serde(default)]
    pub trust_new_host_key: bool,
    /// Bind a native UI trust decision to the exact fingerprint shown to the
    /// user. A different key on the retry must never inherit that decision.
    #[serde(skip)]
    pub confirmed_host_fingerprint: Option<String>,
    /// Optional X11 forwarding target on the local machine.
    #[serde(skip)]
    pub x11: Option<X11ForwardingConfig>,
    pub timeout_secs: u64,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: String::new(),
            password: None,
            key_path: None,
            wireguard_config_path: None,
            key_passphrase: None,
            trust_new_host_key: false,
            confirmed_host_fingerprint: None,
            x11: None,
            timeout_secs: 30,
        }
    }
}
