mod quic;
mod ssh;

use std::io;

use ad_stream_protocol::{ArppFrame, MessagePriority, ProtocolError};
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

pub use quic::QuicTransportAdapter;
pub use ssh::{SshChannelIo, SshTransportAdapter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportKind {
    SshChannels,
    Quic,
}

#[async_trait]
pub trait TransportAdapter: Send + Sync {
    fn kind(&self) -> TransportKind;
    async fn send(&self, frame: ArppFrame) -> Result<(), TransportError>;
    async fn receive(&self) -> Result<ArppFrame, TransportError>;
    async fn close(&self, reason: &str) -> Result<(), TransportError>;
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("ARPP protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("ARPP transport I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("ARPP SSH channel {0} carried a message assigned to another channel")]
    WrongSshChannel(u8),
    #[error("ARPP transport connection failed: {0}")]
    Connection(String),
    #[error("ARPP transport closed")]
    Closed,
}

type InboundItem = Result<ArppFrame, TransportError>;

#[derive(Clone)]
struct InboxSenders {
    highest: mpsc::Sender<InboundItem>,
    high: mpsc::Sender<InboundItem>,
    medium: mpsc::Sender<InboundItem>,
    low: mpsc::Sender<InboundItem>,
}

impl InboxSenders {
    async fn send(&self, item: InboundItem) {
        let priority = item
            .as_ref()
            .map(|frame| frame.priority())
            // Keep EOF behind control messages already read on the same stream,
            // particularly the acknowledgement that the remote session ended.
            .unwrap_or(MessagePriority::High);
        let sender = match priority {
            MessagePriority::Highest => &self.highest,
            MessagePriority::High => &self.high,
            MessagePriority::Medium => &self.medium,
            MessagePriority::Low => &self.low,
        };
        let _ = sender.send(item).await;
    }
}

struct InboxReceivers {
    highest: mpsc::Receiver<InboundItem>,
    high: mpsc::Receiver<InboundItem>,
    medium: mpsc::Receiver<InboundItem>,
    low: mpsc::Receiver<InboundItem>,
}

struct PriorityInbox {
    senders: InboxSenders,
    receivers: Mutex<InboxReceivers>,
}

impl PriorityInbox {
    fn new() -> Self {
        let (highest_tx, highest_rx) = mpsc::channel(1024);
        let (high_tx, high_rx) = mpsc::channel(512);
        // Media is interactive, not a recording. Bound buffering before the
        // decoder so congestion propagates to the sender's rate controller.
        let (medium_tx, medium_rx) = mpsc::channel(2);
        let (low_tx, low_rx) = mpsc::channel(32);
        Self {
            senders: InboxSenders {
                highest: highest_tx,
                high: high_tx,
                medium: medium_tx,
                low: low_tx,
            },
            receivers: Mutex::new(InboxReceivers {
                highest: highest_rx,
                high: high_rx,
                medium: medium_rx,
                low: low_rx,
            }),
        }
    }

    async fn receive(&self) -> Result<ArppFrame, TransportError> {
        let mut receivers = self.receivers.lock().await;
        let InboxReceivers {
            highest,
            high,
            medium,
            low,
        } = &mut *receivers;
        tokio::select! {
            biased;
            item = highest.recv() => item.ok_or(TransportError::Closed)?,
            item = high.recv() => item.ok_or(TransportError::Closed)?,
            item = medium.recv() => item.ok_or(TransportError::Closed)?,
            item = low.recv() => item.ok_or(TransportError::Closed)?,
        }
    }
}
