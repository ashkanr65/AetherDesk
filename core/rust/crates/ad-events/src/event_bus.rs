//! Event bus implementation

use ad_core::SessionId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error};

use crate::events::Event;

/// Type alias for the broadcast sender
type Sender = broadcast::Sender<Event>;
/// Type alias for the broadcast receiver
type Receiver = broadcast::Receiver<Event>;

/// Callback type for event handlers
pub type EventHandler = Box<dyn Fn(&Event) + Send + Sync + 'static>;

/// Event subscription ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Central event bus for AetherDesk
pub struct EventBus {
    sender: Sender,
    subscriptions: Arc<RwLock<HashMap<SubscriptionId, EventHandler>>>,
    next_subscription_id: RwLock<u64>,
}

impl EventBus {
    /// Create a new event bus with the specified capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_subscription_id: RwLock::new(0),
        }
    }

    /// Subscribe to all events
    pub fn subscribe(&self) -> Receiver {
        self.sender.subscribe()
    }

    /// Register an event handler
    pub fn add_handler(&self, handler: EventHandler) -> SubscriptionId {
        let mut next_id = self.next_subscription_id.write();
        let id = SubscriptionId::new(*next_id);
        *next_id += 1;
        drop(next_id);

        self.subscriptions.write().insert(id, handler);
        debug!("Registered event handler with id {:?}", id);
        id
    }

    /// Remove an event handler
    pub fn remove_handler(&self, id: SubscriptionId) {
        self.subscriptions.write().remove(&id);
        debug!("Removed event handler with id {:?}", id);
    }

    /// Emit an event to all subscribers
    pub fn emit(&self, event: Event) {
        debug!("Emitting event: {:?}", event);
        
        // Send to broadcast channel
        if let Err(e) = self.sender.send(event.clone()) {
            error!("Failed to send event to broadcast channel: {}", e);
        }

        // Call registered handlers
        let handlers = self.subscriptions.read();
        for (_id, handler) in handlers.iter() {
            handler(&event);
        }
    }

    /// Emit a session created event
    pub fn emit_session_created(&self, session_id: SessionId, session_type: String) {
        self.emit(Event::SessionCreated { session_id, session_type });
    }

    /// Emit a session connected event
    pub fn emit_session_connected(&self, session_id: SessionId) {
        self.emit(Event::SessionConnected { session_id });
    }

    /// Emit a session disconnected event
    pub fn emit_session_disconnected(&self, session_id: SessionId) {
        self.emit(Event::SessionDisconnected { session_id });
    }

    /// Emit a session closed event
    pub fn emit_session_closed(&self, session_id: SessionId) {
        self.emit(Event::SessionClosed { session_id });
    }

    /// Emit a session error event
    pub fn emit_session_error(&self, session_id: SessionId, error: String) {
        self.emit(Event::SessionError { session_id, error });
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1000)
    }
}
