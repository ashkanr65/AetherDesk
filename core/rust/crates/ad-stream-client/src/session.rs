use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ad_stream_protocol::{ArppFrame, ArppMessage, ControlMessage};
use thiserror::Error;
use uuid::Uuid;

use crate::transport::{TransportAdapter, TransportError};

pub struct ArppSession {
    transport: Arc<dyn TransportAdapter>,
    next_sequence: AtomicU64,
    startup: std::sync::Mutex<StartupInbox>,
}

#[derive(Default)]
struct StartupInbox {
    frames: std::collections::VecDeque<(ArppFrame, usize)>,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResumeSnapshot {
    pub last_presented_frame: u64,
    pub cache_epoch: u64,
    pub cache_digest: [u8; 32],
}

pub struct ResumptionManager {
    session_id: Uuid,
    session: Arc<ArppSession>,
}

impl ResumptionManager {
    pub fn new(session_id: Uuid, session: Arc<ArppSession>) -> Self {
        Self {
            session_id,
            session,
        }
    }

    pub async fn request(&self, snapshot: ResumeSnapshot) -> Result<u64, ResumptionError> {
        Ok(self
            .session
            .send(ArppMessage::Control(ControlMessage::ResumeSession {
                session_id: self.session_id,
                last_presented_frame: snapshot.last_presented_frame,
                cache_epoch: snapshot.cache_epoch,
                cache_digest: snapshot.cache_digest,
            }))
            .await?)
    }

    pub async fn handle_server_state(
        &self,
        message: &ControlMessage,
        local: ResumeSnapshot,
    ) -> Result<ResumeAction, ResumptionError> {
        let ControlMessage::ResumeState {
            session_id,
            cache_epoch,
            width,
            height,
            full_refresh_required,
        } = message
        else {
            return Ok(ResumeAction::Ignored);
        };
        if *session_id != self.session_id {
            return Err(ResumptionError::SessionMismatch);
        }
        if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 {
            return Err(ResumptionError::InvalidDimensions);
        }
        let refresh = *full_refresh_required || *cache_epoch != local.cache_epoch;
        if refresh {
            self.session
                .send(ArppMessage::Control(ControlMessage::RequestKeyFrame {
                    last_decoded_sequence: local.last_presented_frame,
                }))
                .await?;
        }
        Ok(ResumeAction::Accepted {
            width: *width,
            height: *height,
            cache_epoch: *cache_epoch,
            full_refresh: refresh,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeAction {
    Ignored,
    Accepted {
        width: u32,
        height: u32,
        cache_epoch: u64,
        full_refresh: bool,
    },
}

#[derive(Debug, Error)]
pub enum ResumptionError {
    #[error("resumption response belongs to another session")]
    SessionMismatch,
    #[error("resumption response contains invalid dimensions")]
    InvalidDimensions,
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] TransportError),
}

impl ArppSession {
    pub fn new(transport: Arc<dyn TransportAdapter>) -> Self {
        Self {
            transport,
            next_sequence: AtomicU64::new(1),
            startup: Default::default(),
        }
    }

    pub async fn send(&self, message: ArppMessage) -> Result<u64, TransportError> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        self.transport
            .send(ArppFrame::new(sequence, message))
            .await?;
        Ok(sequence)
    }

    pub async fn receive(&self) -> Result<ArppFrame, TransportError> {
        {
            let mut startup = self.startup.lock().map_err(|_| TransportError::Closed)?;
            if let Some((frame, bytes)) = startup.frames.pop_front() {
                startup.bytes -= bytes;
                return Ok(frame);
            }
        }
        self.transport.receive().await
    }

    /// SSH channels have independent readers. Cursor, clipboard and display
    /// notifications can reach the inbox before an earlier control response.
    /// Preserve them for the normal consumer, without weakening control/session
    /// validation or extending the caller's overall handshake deadline.
    pub async fn receive_startup_control(&self) -> Result<ArppFrame, TransportError> {
        loop {
            let frame = self.transport.receive().await?;
            if !matches!(
                &frame.message,
                ArppMessage::Display(_)
                    | ArppMessage::Clipboard(_)
                    | ArppMessage::Audio(_)
                    | ArppMessage::Metrics(_)
            ) {
                return Ok(frame);
            }
            let bytes = ad_stream_protocol::encode_frame(&frame)?.len();
            let mut startup = self.startup.lock().map_err(|_| TransportError::Closed)?;
            if startup.frames.len() >= 64 || bytes > 8 * 1024 * 1024 - startup.bytes {
                return Err(TransportError::Connection(
                    "Too much asynchronous data before the desktop handshake completed".into(),
                ));
            }
            startup.bytes += bytes;
            startup.frames.push_back((frame, bytes));
        }
    }

    pub async fn close(&self, reason: &str) -> Result<(), TransportError> {
        self.transport.close(reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    use crate::transport::{TransportAdapter, TransportKind};

    struct EarlyNotifications(std::sync::Mutex<std::collections::VecDeque<ArppFrame>>);
    #[tokio::test]
    async fn independent_ssh_readers_preserve_cursor_and_clipboard_before_resume_control_arrives() {
        use crate::{SshChannelIo, SshTransportAdapter};
        use tokio::io::{duplex, AsyncWriteExt};
        let (control, mut control_peer) = duplex(4096);
        let (input, _input_peer) = duplex(1024);
        let (media, mut media_peer) = duplex(4096);
        let session = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        ))));
        let cursor = ArppFrame::new(
            1,
            ArppMessage::Display(ad_stream_protocol::DisplayMessage::CursorUpdate {
                frame_id: 0,
                x: 0,
                y: 0,
                hotspot_x: 0,
                hotspot_y: 0,
                width: 1,
                height: 1,
                visible: true,
                rgba: vec![255; 4].into(),
            }),
        );
        let clipboard = ArppFrame::new(
            2,
            ArppMessage::Clipboard(ad_stream_protocol::ClipboardMessage::Offer {
                revision: 1,
                mime_types: vec!["text/plain".into()],
            }),
        );
        let receiving = {
            let session = session.clone();
            tokio::spawn(async move { session.receive_startup_control().await })
        };
        for (index, frame) in [cursor.clone(), clipboard.clone()].into_iter().enumerate() {
            media_peer
                .write_all(&ad_stream_protocol::encode_frame(&frame).unwrap())
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while session.startup.lock().unwrap().frames.len() != index + 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        let ready = ArppFrame::new(
            3,
            ArppMessage::Control(ControlMessage::ResumeState {
                session_id: Uuid::new_v4(),
                cache_epoch: 0,
                width: 640,
                height: 480,
                full_refresh_required: true,
            }),
        );
        control_peer
            .write_all(&ad_stream_protocol::encode_frame(&ready).unwrap())
            .await
            .unwrap();
        assert_eq!(receiving.await.unwrap().unwrap(), ready);
        assert_eq!(session.receive().await.unwrap(), cursor);
        assert_eq!(session.receive().await.unwrap(), clipboard);
    }
    #[async_trait]
    impl TransportAdapter for EarlyNotifications {
        fn kind(&self) -> TransportKind {
            TransportKind::SshChannels
        }
        async fn send(&self, _: ArppFrame) -> Result<(), TransportError> {
            Ok(())
        }
        async fn receive(&self) -> Result<ArppFrame, TransportError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(TransportError::Closed)
        }
        async fn close(&self, _: &str) -> Result<(), TransportError> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn startup_notifications_are_bounded_and_replayed_before_later_transport_data() {
        let pong = |n| {
            ArppFrame::new(
                n,
                ArppMessage::Metrics(ad_stream_protocol::MetricsMessage::Pong {
                    nonce: n,
                    sent_at_us: 0,
                    received_at_us: 0,
                }),
            )
        };
        let reply = ArppFrame::new(
            3,
            ArppMessage::Control(ControlMessage::ServerHello {
                protocol_version: ad_stream_protocol::PROTOCOL_VERSION,
                server_nonce: [9; 32],
            }),
        );
        let session = ArppSession::new(Arc::new(EarlyNotifications(std::sync::Mutex::new(
            [pong(1), pong(2), reply.clone(), pong(4)].into(),
        ))));
        assert_eq!(session.receive_startup_control().await.unwrap(), reply);
        for n in [1, 2, 4] {
            assert_eq!(session.receive().await.unwrap(), pong(n));
        }
        let session = ArppSession::new(Arc::new(EarlyNotifications(std::sync::Mutex::new(
            (1..=65).map(pong).collect(),
        ))));
        assert!(matches!(
            session.receive_startup_control().await,
            Err(TransportError::Connection(_))
        ));
        assert_eq!(session.startup.lock().unwrap().frames.len(), 64);
        let large = ArppFrame::new(
            1,
            ArppMessage::Clipboard(ad_stream_protocol::ClipboardMessage::Data {
                revision: 1,
                mime_type: "image/png".into(),
                data: vec![0; 8 * 1024 * 1024].into(),
            }),
        );
        let session = ArppSession::new(Arc::new(EarlyNotifications(std::sync::Mutex::new(
            [large].into(),
        ))));
        assert!(matches!(
            session.receive_startup_control().await,
            Err(TransportError::Connection(_))
        ));
        assert!(session.startup.lock().unwrap().frames.is_empty());
    }

    #[derive(Default)]
    struct MockTransport {
        sent: Mutex<Vec<ArppFrame>>,
    }

    #[async_trait]
    impl TransportAdapter for MockTransport {
        fn kind(&self) -> TransportKind {
            TransportKind::SshChannels
        }

        async fn send(&self, frame: ArppFrame) -> Result<(), TransportError> {
            self.sent.lock().unwrap().push(frame);
            Ok(())
        }

        async fn receive(&self) -> Result<ArppFrame, TransportError> {
            Err(TransportError::Closed)
        }

        async fn close(&self, _reason: &str) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn cache_epoch_mismatch_requests_a_keyframe() {
        let transport = Arc::new(MockTransport::default());
        let session = Arc::new(ArppSession::new(transport.clone()));
        let session_id = Uuid::new_v4();
        let manager = ResumptionManager::new(session_id, session);
        let action = manager
            .handle_server_state(
                &ControlMessage::ResumeState {
                    session_id,
                    cache_epoch: 8,
                    width: 1920,
                    height: 1080,
                    full_refresh_required: false,
                },
                ResumeSnapshot {
                    cache_epoch: 7,
                    last_presented_frame: 99,
                    cache_digest: [1; 32],
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            action,
            ResumeAction::Accepted {
                full_refresh: true,
                ..
            }
        ));
        assert!(matches!(
            transport.sent.lock().unwrap()[0].message,
            ArppMessage::Control(ControlMessage::RequestKeyFrame {
                last_decoded_sequence: 99
            })
        ));
    }

    #[tokio::test]
    async fn matching_cache_resumes_without_keyframe() {
        let transport = Arc::new(MockTransport::default());
        let session = Arc::new(ArppSession::new(transport.clone()));
        let session_id = Uuid::new_v4();
        let manager = ResumptionManager::new(session_id, session);
        let action = manager
            .handle_server_state(
                &ControlMessage::ResumeState {
                    session_id,
                    cache_epoch: 4,
                    width: 1280,
                    height: 720,
                    full_refresh_required: false,
                },
                ResumeSnapshot {
                    cache_epoch: 4,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            action,
            ResumeAction::Accepted {
                full_refresh: false,
                ..
            }
        ));
        assert!(transport.sent.lock().unwrap().is_empty());
    }
}
