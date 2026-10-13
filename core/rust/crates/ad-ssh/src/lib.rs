//! AetherDesk SSH Client Library
//!
//! Provides SSH client functionality including authentication, channels, and session management.

pub mod auth;
pub mod client;
pub mod known_hosts;
mod shell;

pub use auth::AuthMethod;
pub use client::{CommandOutput, SftpEntry, SshClient, SshConfig, SshSession, X11ForwardingConfig};
pub use client::{
    FileProgress, HostIdentity, LanguageServer, LanguageServerConfig, RemoteFileMetadata,
    RemoteFileRequest, RemoteFileResult,
};
pub use known_hosts::KnownHosts;
