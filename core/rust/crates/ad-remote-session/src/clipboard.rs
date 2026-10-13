use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ad_stream_client::ArppSession;
use ad_stream_protocol::{ArppMessage, ClipboardMessage};
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;

type ClipboardFormats = HashMap<String, Vec<u8>>;
type VersionedClipboard = Option<(u64, ClipboardFormats)>;
type PendingClipboard = HashMap<(u64, String), Vec<u8>>;

const CLIPBOARD_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ClipboardLimits {
    pub maximum_text_bytes: usize,
    pub maximum_image_bytes: usize,
}

impl Default for ClipboardLimits {
    fn default() -> Self {
        Self {
            maximum_text_bytes: 10 * 1024 * 1024,
            maximum_image_bytes: 10 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardContent {
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[async_trait]
pub trait ClipboardBackend: Send + Sync {
    async fn set_content(&self, content: ClipboardContent) -> Result<(), ClipboardError>;
}

pub struct ClipboardBridge<B: ClipboardBackend> {
    backend: Arc<B>,
    session: Arc<ArppSession>,
    limits: ClipboardLimits,
    next_revision: AtomicU64,
    local: Mutex<VersionedClipboard>,
    remote_offers: Mutex<HashMap<u64, Vec<String>>>,
    pending_remote: Mutex<PendingClipboard>,
    last_remote_content: Mutex<Option<ClipboardContent>>,
    last_applied_remote_revision: AtomicU64,
}

impl<B: ClipboardBackend> ClipboardBridge<B> {
    pub fn new(backend: Arc<B>, session: Arc<ArppSession>, limits: ClipboardLimits) -> Self {
        Self {
            backend,
            session,
            limits,
            next_revision: AtomicU64::new(1),
            local: Mutex::new(None),
            remote_offers: Mutex::new(HashMap::new()),
            pending_remote: Mutex::new(HashMap::new()),
            last_remote_content: Mutex::new(None),
            last_applied_remote_revision: AtomicU64::new(0),
        }
    }

    /// Advertise a clipboard snapshot. Data is retained only for the current
    /// revision and transmitted lazily when the remote endpoint requests a MIME type.
    pub async fn publish_local(
        &self,
        contents: Vec<ClipboardContent>,
    ) -> Result<u64, ClipboardError> {
        if contents.is_empty() || contents.len() > 8 {
            return Err(ClipboardError::InvalidOffer);
        }
        let mut data = HashMap::new();
        for content in contents {
            validate_content(&content, self.limits)?;
            if data.insert(content.mime_type, content.data).is_some() {
                return Err(ClipboardError::InvalidOffer);
            }
        }
        if data.len() == 1 {
            if let Some(last) = self.last_remote_content.lock().await.as_ref() {
                if data
                    .get(&last.mime_type)
                    .is_some_and(|candidate| candidate == &last.data)
                {
                    return Ok(self.last_applied_remote_revision.load(Ordering::Acquire));
                }
            }
        }
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let mime_types = data.keys().cloned().collect::<Vec<_>>();
        *self.local.lock().await = Some((revision, data));
        self.session
            .send(ArppMessage::Clipboard(ClipboardMessage::Offer {
                revision,
                mime_types,
            }))
            .await?;
        Ok(revision)
    }

    pub async fn handle_remote(&self, message: ClipboardMessage) -> Result<(), ClipboardError> {
        match message {
            ClipboardMessage::Offer {
                revision,
                mime_types,
            } => {
                if revision <= self.last_applied_remote_revision.load(Ordering::Acquire)
                    || mime_types.is_empty()
                    || mime_types.len() > 8
                    || mime_types.iter().any(|value| !supported_mime(value))
                {
                    return Err(ClipboardError::InvalidOffer);
                }
                let requested = preferred_mime(&mime_types)
                    .ok_or(ClipboardError::UnsupportedMime)?
                    .to_owned();
                let mut offers = self.remote_offers.lock().await;
                offers.clear();
                offers.insert(revision, mime_types);
                drop(offers);
                self.session
                    .send(ArppMessage::Clipboard(ClipboardMessage::Request {
                        revision,
                        mime_type: requested,
                    }))
                    .await?;
            }
            ClipboardMessage::Request {
                revision,
                mime_type,
            } => {
                let local = self.local.lock().await;
                let (_, formats) = local
                    .as_ref()
                    .filter(|(current, _)| *current == revision)
                    .ok_or(ClipboardError::StaleRevision)?;
                let data = formats
                    .get(&mime_type)
                    .ok_or(ClipboardError::UnsupportedMime)?
                    .clone();
                drop(local);
                if data.len() <= CLIPBOARD_CHUNK_BYTES {
                    self.session
                        .send(ArppMessage::Clipboard(ClipboardMessage::Data {
                            revision,
                            mime_type,
                            data,
                        }))
                        .await?;
                } else {
                    let total_size = data.len() as u64;
                    for (index, chunk) in data.chunks(CLIPBOARD_CHUNK_BYTES).enumerate() {
                        self.session
                            .send(ArppMessage::Clipboard(ClipboardMessage::DataChunk {
                                revision,
                                mime_type: mime_type.clone(),
                                offset: (index * CLIPBOARD_CHUNK_BYTES) as u64,
                                total_size,
                                data: chunk.to_vec(),
                            }))
                            .await?;
                    }
                }
            }
            ClipboardMessage::Data {
                revision,
                mime_type,
                data,
            } => {
                let mut offers = self.remote_offers.lock().await;
                let offered = offers
                    .remove(&revision)
                    .ok_or(ClipboardError::StaleRevision)?;
                if !offered.contains(&mime_type) {
                    return Err(ClipboardError::UnsupportedMime);
                }
                self.apply_remote(revision, mime_type, data).await?;
            }
            ClipboardMessage::DataChunk {
                revision,
                mime_type,
                offset,
                total_size,
                data,
            } => {
                let maximum = maximum_bytes(&mime_type, self.limits)? as u64;
                if total_size == 0
                    || total_size > maximum
                    || data.is_empty()
                    || data.len() > CLIPBOARD_CHUNK_BYTES
                {
                    return Err(ClipboardError::PayloadTooLarge);
                }
                let offered = self.remote_offers.lock().await;
                if !offered
                    .get(&revision)
                    .is_some_and(|types| types.contains(&mime_type))
                {
                    return Err(ClipboardError::StaleRevision);
                }
                drop(offered);
                let mut pending = self.pending_remote.lock().await;
                let assembled = pending.entry((revision, mime_type.clone())).or_default();
                if offset != assembled.len() as u64
                    || offset.saturating_add(data.len() as u64) > total_size
                {
                    pending.remove(&(revision, mime_type));
                    return Err(ClipboardError::InvalidChunk);
                }
                assembled.extend_from_slice(&data);
                if assembled.len() as u64 == total_size {
                    let complete = pending
                        .remove(&(revision, mime_type.clone()))
                        .ok_or(ClipboardError::InvalidChunk)?;
                    drop(pending);
                    self.remote_offers.lock().await.remove(&revision);
                    self.apply_remote(revision, mime_type, complete).await?;
                }
            }
        }
        Ok(())
    }

    async fn apply_remote(
        &self,
        revision: u64,
        mime_type: String,
        data: Vec<u8>,
    ) -> Result<(), ClipboardError> {
        let content = ClipboardContent { mime_type, data };
        validate_content(&content, self.limits)?;
        self.backend.set_content(content.clone()).await?;
        *self.last_remote_content.lock().await = Some(content);
        self.last_applied_remote_revision
            .store(revision, Ordering::Release);
        Ok(())
    }

    pub fn last_remote_revision(&self) -> u64 {
        self.last_applied_remote_revision.load(Ordering::Acquire)
    }
}

fn supported_mime(value: &str) -> bool {
    matches!(
        value.split(';').next().map(str::trim),
        Some("text/plain" | "text/html" | "image/png" | "image/bmp")
    )
}

fn preferred_mime(values: &[String]) -> Option<&str> {
    [
        "text/plain;charset=utf-8",
        "text/plain",
        "text/html",
        "image/png",
        "image/bmp",
    ]
    .into_iter()
    .find(|candidate| values.iter().any(|value| value == candidate))
}

fn validate_content(
    content: &ClipboardContent,
    limits: ClipboardLimits,
) -> Result<(), ClipboardError> {
    if !supported_mime(&content.mime_type) {
        return Err(ClipboardError::UnsupportedMime);
    }
    let maximum = maximum_bytes(&content.mime_type, limits)?;
    if content.data.len() > maximum {
        return Err(ClipboardError::PayloadTooLarge);
    }
    if (content.mime_type.starts_with("text/plain") || content.mime_type.starts_with("text/html"))
        && std::str::from_utf8(&content.data).is_err()
    {
        return Err(ClipboardError::InvalidText);
    }
    if content.mime_type == "image/png"
        && !content
            .data
            .starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
    {
        return Err(ClipboardError::InvalidImage);
    }
    if content.mime_type == "image/bmp" && !content.data.starts_with(b"BM") {
        return Err(ClipboardError::InvalidImage);
    }
    Ok(())
}

fn maximum_bytes(mime_type: &str, limits: ClipboardLimits) -> Result<usize, ClipboardError> {
    if !supported_mime(mime_type) {
        return Err(ClipboardError::UnsupportedMime);
    }
    Ok(if mime_type.starts_with("text/") {
        limits.maximum_text_bytes
    } else {
        limits.maximum_image_bytes
    })
}

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard MIME type is unsupported")]
    UnsupportedMime,
    #[error("clipboard offer is invalid")]
    InvalidOffer,
    #[error("clipboard revision is stale or was not offered")]
    StaleRevision,
    #[error("clipboard payload exceeds the configured limit")]
    PayloadTooLarge,
    #[error("text clipboard data is not valid UTF-8")]
    InvalidText,
    #[error("clipboard image does not match its declared format")]
    InvalidImage,
    #[error("clipboard chunks are missing, duplicated, or out of order")]
    InvalidChunk,
    #[error("clipboard backend failed: {0}")]
    Backend(String),
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] ad_stream_client::TransportError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ad_stream_client::{TransportAdapter, TransportError, TransportKind};
    use ad_stream_protocol::ArppFrame;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct MemoryClipboard(StdMutex<Vec<ClipboardContent>>);

    #[async_trait]
    impl ClipboardBackend for MemoryClipboard {
        async fn set_content(&self, content: ClipboardContent) -> Result<(), ClipboardError> {
            self.0.lock().unwrap().push(content);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryTransport(StdMutex<Vec<ArppFrame>>);

    #[async_trait]
    impl TransportAdapter for MemoryTransport {
        fn kind(&self) -> TransportKind {
            TransportKind::SshChannels
        }
        async fn send(&self, frame: ArppFrame) -> Result<(), TransportError> {
            self.0.lock().unwrap().push(frame);
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
    async fn negotiates_and_applies_remote_text() {
        let backend = Arc::new(MemoryClipboard::default());
        let transport = Arc::new(MemoryTransport::default());
        let bridge = ClipboardBridge::new(
            backend.clone(),
            Arc::new(ArppSession::new(transport.clone())),
            ClipboardLimits::default(),
        );
        bridge
            .handle_remote(ClipboardMessage::Offer {
                revision: 7,
                mime_types: vec!["text/plain;charset=utf-8".into()],
            })
            .await
            .unwrap();
        bridge
            .handle_remote(ClipboardMessage::Data {
                revision: 7,
                mime_type: "text/plain;charset=utf-8".into(),
                data: b"hello".to_vec(),
            })
            .await
            .unwrap();
        assert_eq!(backend.0.lock().unwrap()[0].data, b"hello");
        assert_eq!(bridge.last_remote_revision(), 7);
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reassembles_large_clipboards_and_rejects_out_of_order_chunks() {
        let backend = Arc::new(MemoryClipboard::default());
        let transport = Arc::new(MemoryTransport::default());
        let bridge = ClipboardBridge::new(
            backend.clone(),
            Arc::new(ArppSession::new(transport)),
            ClipboardLimits::default(),
        );
        bridge
            .handle_remote(ClipboardMessage::Offer {
                revision: 9,
                mime_types: vec!["image/png".into()],
            })
            .await
            .unwrap();
        bridge
            .handle_remote(ClipboardMessage::DataChunk {
                revision: 9,
                mime_type: "image/png".into(),
                offset: 0,
                total_size: 12,
                data: vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            })
            .await
            .unwrap();
        assert!(matches!(
            bridge
                .handle_remote(ClipboardMessage::DataChunk {
                    revision: 9,
                    mime_type: "image/png".into(),
                    offset: 9,
                    total_size: 12,
                    data: vec![1, 2],
                })
                .await,
            Err(ClipboardError::InvalidChunk)
        ));

        bridge
            .handle_remote(ClipboardMessage::DataChunk {
                revision: 9,
                mime_type: "image/png".into(),
                offset: 0,
                total_size: 12,
                data: vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            })
            .await
            .unwrap();
        bridge
            .handle_remote(ClipboardMessage::DataChunk {
                revision: 9,
                mime_type: "image/png".into(),
                offset: 8,
                total_size: 12,
                data: vec![1, 2, 3, 4],
            })
            .await
            .unwrap();
        assert_eq!(backend.0.lock().unwrap()[0].data.len(), 12);
    }

    #[tokio::test]
    async fn suppresses_a_clipboard_echo_after_remote_injection() {
        let backend = Arc::new(MemoryClipboard::default());
        let transport = Arc::new(MemoryTransport::default());
        let bridge = ClipboardBridge::new(
            backend,
            Arc::new(ArppSession::new(transport.clone())),
            ClipboardLimits::default(),
        );
        bridge
            .handle_remote(ClipboardMessage::Offer {
                revision: 11,
                mime_types: vec!["text/plain".into()],
            })
            .await
            .unwrap();
        bridge
            .handle_remote(ClipboardMessage::Data {
                revision: 11,
                mime_type: "text/plain".into(),
                data: b"same".to_vec(),
            })
            .await
            .unwrap();
        let messages_before = transport.0.lock().unwrap().len();
        let revision = bridge
            .publish_local(vec![ClipboardContent {
                mime_type: "text/plain".into(),
                data: b"same".to_vec(),
            }])
            .await
            .unwrap();
        assert_eq!(revision, 11);
        assert_eq!(transport.0.lock().unwrap().len(), messages_before);
    }
}
