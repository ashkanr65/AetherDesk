//! AetherDesk remote-session integration.
//!
//! This crate deliberately separates deployment and lifecycle policy from the
//! concrete SSH implementation. The same state machine is exercised against a
//! deterministic in-memory agent in integration tests.

pub mod bootstrap;
pub mod clipboard;
pub mod engine;
pub mod file_bridge;
pub mod lifecycle;
pub mod ssh_host;
#[cfg(target_os = "windows")]
pub mod windows_clipboard;

pub use bootstrap::{
    AgentArtifact, AgentBootstrap, AgentBootstrapError, AgentDeployment, AgentRuntimeBundle,
    RemoteAgentHost, RemoteCommandResult, RuntimeLibrary, RuntimeManifest,
};
pub use clipboard::{
    ClipboardBackend, ClipboardBridge, ClipboardContent, ClipboardError, ClipboardLimits,
};
#[cfg(feature = "rendering")]
pub use engine::AetherDisplayEngine;
pub use engine::{DisplayEngine, DisplayEngineError, EngineConnectionState, RemoteDesktopConfig};
pub use file_bridge::{FileBridge, FileBridgeError, FileOffer, FileTransferProgress};
pub use lifecycle::{
    AgentConnection, AgentEndpoint, DisconnectMode, LifecycleError, RemoteSessionLifecycle,
};
pub use ssh_host::SshAgentHost;
#[cfg(target_os = "windows")]
pub use windows_clipboard::WindowsClipboard;
