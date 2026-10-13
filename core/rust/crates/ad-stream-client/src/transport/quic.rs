use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ad_stream_protocol::{
    encode_frame, ArppFrame, ArppMessage, ControlMessage, MessagePriority, QuicBootstrap,
};
use async_trait::async_trait;
use quinn::{Connection, Endpoint, VarInt};

use super::ssh::read_frame;
use super::{PriorityInbox, TransportAdapter, TransportError, TransportKind};

pub struct QuicTransportAdapter {
    connection: Connection,
    inbox: Arc<PriorityInbox>,
}

impl QuicTransportAdapter {
    pub fn from_connection(connection: Connection) -> Self {
        let inbox = Arc::new(PriorityInbox::new());
        spawn_receiver(connection.clone(), Arc::clone(&inbox));
        Self { connection, inbox }
    }

    pub async fn connect(
        endpoint: &Endpoint,
        remote_address: SocketAddr,
        bootstrap: QuicBootstrap,
    ) -> Result<Self, TransportError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| TransportError::Connection(error.to_string()))?
            .as_millis() as u64;
        bootstrap.validate_at(now)?;
        let connection = endpoint
            .connect(remote_address, &bootstrap.server_name)
            .map_err(|error| TransportError::Connection(error.to_string()))?
            .await
            .map_err(|error| TransportError::Connection(error.to_string()))?;
        let adapter = Self::from_connection(connection);
        adapter
            .send(ArppFrame::new(
                0,
                ArppMessage::Control(ControlMessage::QuicAuthenticate {
                    session_id: bootstrap.session_id,
                    one_time_token: bootstrap.one_time_token,
                }),
            ))
            .await?;
        Ok(adapter)
    }
}

#[async_trait]
impl TransportAdapter for QuicTransportAdapter {
    fn kind(&self) -> TransportKind {
        TransportKind::Quic
    }

    async fn send(&self, frame: ArppFrame) -> Result<(), TransportError> {
        let encoded = encode_frame(&frame)?;
        let mut stream = self
            .connection
            .open_uni()
            .await
            .map_err(|error| TransportError::Connection(error.to_string()))?;
        stream
            .set_priority(quic_priority(frame.priority()))
            .map_err(|error| TransportError::Connection(error.to_string()))?;
        stream
            .write_all(&encoded)
            .await
            .map_err(|error| TransportError::Connection(error.to_string()))?;
        stream
            .finish()
            .map_err(|error| TransportError::Connection(error.to_string()))?;
        Ok(())
    }

    async fn receive(&self) -> Result<ArppFrame, TransportError> {
        self.inbox.receive().await
    }

    async fn close(&self, reason: &str) -> Result<(), TransportError> {
        self.connection
            .close(VarInt::from_u32(0), reason.as_bytes());
        Ok(())
    }
}

const fn quic_priority(priority: MessagePriority) -> i32 {
    match priority {
        MessagePriority::Highest => 30,
        MessagePriority::High => 20,
        MessagePriority::Medium => 10,
        MessagePriority::Low => 0,
    }
}

fn spawn_receiver(connection: Connection, inbox: Arc<PriorityInbox>) {
    tokio::spawn(async move {
        loop {
            let mut stream = match connection.accept_uni().await {
                Ok(stream) => stream,
                Err(error) => {
                    inbox
                        .senders
                        .send(Err(TransportError::Connection(error.to_string())))
                        .await;
                    break;
                }
            };
            let inbox = Arc::clone(&inbox);
            tokio::spawn(async move {
                let result = read_frame(&mut stream).await;
                inbox.senders.send(result).await;
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_streams_have_strictly_higher_quic_priority_than_media() {
        assert!(quic_priority(MessagePriority::Highest) > quic_priority(MessagePriority::Medium));
        assert!(quic_priority(MessagePriority::High) > quic_priority(MessagePriority::Low));
    }
}
