//! AetherDesk Core Library
//!
//! This crate provides fundamental types, traits, and utilities used across
//! all AetherDesk components.

pub mod error;
pub mod types;
pub mod utils;

pub use error::{CoreError, Result};
pub use types::{SessionId, ConnectionId, Message};
pub use utils::generate_id;
