use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ad_stream_protocol::{ArppFrame, ArppMessage, ControlMessage};
use thiserror::Error;
use uuid::Uuid;

use crate::transport::{TransportAdapter, TransportError};

pub struct ArppSession {
    transport: Arc<dyn TransportAdapter>,
    next_sequence: AtomicU64,
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
        self.transport.receive().await
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
