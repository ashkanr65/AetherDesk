//! AetherDesk Event Bus Library
//!
//! Provides a centralized event bus for communication between components.

pub mod event_bus;
pub mod events;

pub use event_bus::EventBus;
pub use events::Event;
