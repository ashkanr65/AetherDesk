//! Transport interfaces for the data-only ARPP message model.

use std::io::Result;

use async_trait::async_trait;

use crate::messages::{
    ArppMessage, ClipboardPayload, ControlPayload, DisplayPayload, InputPayload,
};

/// Asynchronous interface for sending and receiving ARPP messages.
///
/// Implementations must be safe to share across threads. Returned futures are
/// `Send`, and the interface can be used through `dyn TransportAdapter`.
/// Operations report failures through [`std::io::Error`], which may wrap an
/// underlying framing error. Connection setup and transport implementations are
/// outside this interface.
#[async_trait]
pub trait TransportAdapter: Send + Sync {
    /// Send a session control payload.
    async fn send_control(&self, msg: ControlPayload) -> Result<()>;

    /// Send a keyboard, mouse, or touch input payload.
    async fn send_input(&self, msg: InputPayload) -> Result<()>;

    /// Send a display update payload.
    async fn send_display(&self, msg: DisplayPayload) -> Result<()>;

    /// Send a clipboard payload.
    async fn send_clipboard(&self, msg: ClipboardPayload) -> Result<()>;

    /// Receive the next complete message from the transport.
    async fn recv(&self) -> Result<ArppMessage>;
}

/// SSH channel identifiers for control, input, and display traffic.
///
/// The conventional mapping is control on channel 0, input on channel 1, and
/// display on channel 2. This structure only records identifiers; it does not
/// open channels or enforce routing.
pub struct SshChannelMap {
    /// Control channel identifier, conventionally channel 0.
    pub control_channel: u32,
    /// Input channel identifier, conventionally channel 1.
    pub input_channel: u32,
    /// Display channel identifier, conventionally channel 2.
    pub display_channel: u32,
}
