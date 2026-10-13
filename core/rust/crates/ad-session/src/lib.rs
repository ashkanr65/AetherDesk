//! AetherDesk Session Management Library
//!
//! Provides session lifecycle management, multiplexing, and coordination.

pub mod manager;
pub mod session;
pub mod types;

pub use manager::SessionManager;
pub use session::Session;
pub use types::SessionState;
