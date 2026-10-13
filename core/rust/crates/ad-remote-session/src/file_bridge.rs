use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ad_stream_client::ArppSession;
use ad_stream_protocol::{ArppMessage, BulkMessage, ControlMessage};
use ring::digest::{Context, SHA256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{oneshot, watch, Mutex};
use tokio::time::timeout;
use uuid::Uuid;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const CHUNK_BYTES: usize = 256 * 1024;
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOffer {
    pub transfer_id: Uuid,
    pub local_path: PathBuf,
    pub file_name: String,
    pub size: u64,
    pub mime_types: Vec<String>,
    pub sha256: [u8; 32],
}

impl FileOffer {
    pub async fn inspect(
        local_path: impl Into<PathBuf>,
        mime_types: Vec<String>,
    ) -> Result<Self, FileBridgeError> {
        let local_path = local_path.into();
        let metadata = tokio::fs::metadata(&local_path).await?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(FileBridgeError::InvalidFile);
        }
        let file_name = local_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| safe_file_name(name))
            .ok_or(FileBridgeError::InvalidFileName)?
            .to_owned();
        if mime_types.is_empty()
            || mime_types.len() > 8
            || mime_types.iter().any(|value| !safe_mime(value))
        {
            return Err(FileBridgeError::InvalidMime);
        }
        let sha256 = digest_file(&local_path).await?;
        Ok(Self {
            transfer_id: Uuid::new_v4(),
            local_path,
            file_name,
            size: metadata.len(),
            mime_types,
            sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTransferProgress {
    pub transfer_id: Uuid,
    pub sent: u64,
    pub total: u64,
    pub completed: bool,
    pub remote_uri: Option<String>,
}

struct AcceptedTransfer {
    mime_type: String,
    resume_offset: u64,
    staging_path: String,
}

pub struct FileBridge {
    session: Arc<ArppSession>,
    pending: Mutex<HashMap<Uuid, oneshot::Sender<Result<AcceptedTransfer, FileBridgeError>>>>,
    ready: Mutex<HashMap<Uuid, String>>,
    progress: watch::Sender<Option<FileTransferProgress>>,
}

impl FileBridge {
    pub fn new(session: Arc<ArppSession>) -> Self {
        let (progress, _) = watch::channel(None);
        Self {
            session,
            pending: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashMap::new()),
            progress,
        }
    }

    pub fn subscribe_progress(&self) -> watch::Receiver<Option<FileTransferProgress>> {
        self.progress.subscribe()
    }

    /// Process file-negotiation control messages received by the session demuxer.
    pub async fn handle_control(&self, message: ControlMessage) -> Result<bool, FileBridgeError> {
        match message {
            ControlMessage::FileAccept {
                transfer_id,
                mime_type,
                resume_offset,
                staging_path,
            } => {
                let sender = self
                    .pending
                    .lock()
                    .await
                    .remove(&transfer_id)
                    .ok_or(FileBridgeError::UnknownTransfer)?;
                let _ = sender.send(Ok(AcceptedTransfer {
                    mime_type,
                    resume_offset,
                    staging_path,
                }));
                Ok(true)
            }
            ControlMessage::FileReject {
                transfer_id,
                reason,
            } => {
                let sender = self
                    .pending
                    .lock()
                    .await
                    .remove(&transfer_id)
                    .ok_or(FileBridgeError::UnknownTransfer)?;
                let _ = sender.send(Err(FileBridgeError::Rejected(bound_text(&reason, 1024))));
                Ok(true)
            }
            ControlMessage::FileReady {
                transfer_id,
                remote_uri,
            } => {
                if remote_uri.len() > 4096 || !remote_uri.starts_with("file://") {
                    return Err(FileBridgeError::UnsafeRemotePath);
                }
                self.ready.lock().await.insert(transfer_id, remote_uri);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub async fn send_file(&self, offer: FileOffer) -> Result<Uuid, FileBridgeError> {
        let (sender, receiver) = oneshot::channel();
        if self
            .pending
            .lock()
            .await
            .insert(offer.transfer_id, sender)
            .is_some()
        {
            return Err(FileBridgeError::DuplicateTransfer);
        }
        self.session
            .send(ArppMessage::Control(ControlMessage::FileOffer {
                transfer_id: offer.transfer_id,
                file_name: offer.file_name.clone(),
                size: offer.size,
                mime_types: offer.mime_types.clone(),
                sha256: offer.sha256,
            }))
            .await?;

        let accepted = match timeout(NEGOTIATION_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err(FileBridgeError::NegotiationClosed),
            Err(_) => {
                self.pending.lock().await.remove(&offer.transfer_id);
                return Err(FileBridgeError::NegotiationTimeout);
            }
        };
        if !offer.mime_types.contains(&accepted.mime_type)
            || accepted.resume_offset > offer.size
            || !safe_remote_relative_path(&accepted.staging_path)
        {
            return Err(FileBridgeError::InvalidAcceptance);
        }

        self.session
            .send(ArppMessage::Bulk(BulkMessage::FileOpen {
                transfer_id: offer.transfer_id,
                path: accepted.staging_path,
                size: offer.size,
                overwrite: accepted.resume_offset == 0,
            }))
            .await?;

        let mut file = tokio::fs::File::open(&offer.local_path).await?;
        file.seek(std::io::SeekFrom::Start(accepted.resume_offset))
            .await?;
        let mut offset = accepted.resume_offset;
        let mut buffer = vec![0_u8; CHUNK_BYTES];
        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            self.session
                .send(ArppMessage::Bulk(BulkMessage::FileChunk {
                    transfer_id: offer.transfer_id,
                    offset,
                    data: buffer[..count].to_vec(),
                }))
                .await?;
            offset = offset
                .checked_add(count as u64)
                .ok_or(FileBridgeError::InvalidFile)?;
            self.progress.send_replace(Some(FileTransferProgress {
                transfer_id: offer.transfer_id,
                sent: offset,
                total: offer.size,
                completed: false,
                remote_uri: None,
            }));
        }
        if offset != offer.size {
            return Err(FileBridgeError::FileChanged);
        }
        self.session
            .send(ArppMessage::Bulk(BulkMessage::FileClose {
                transfer_id: offer.transfer_id,
                sha256: offer.sha256,
            }))
            .await?;
        self.progress.send_replace(Some(FileTransferProgress {
            transfer_id: offer.transfer_id,
            sent: offset,
            total: offer.size,
            completed: true,
            remote_uri: self.ready.lock().await.remove(&offer.transfer_id),
        }));
        Ok(offer.transfer_id)
    }
}

async fn digest_file(path: &Path) -> Result<[u8; 32], FileBridgeError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut context = Context::new(&SHA256);
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }
    context
        .finish()
        .as_ref()
        .try_into()
        .map_err(|_| FileBridgeError::Digest)
}

fn safe_file_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', '\0', '\n', '\r'])
}

fn safe_mime(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\'' | b'\\'))
}

fn safe_remote_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.starts_with('/')
        && !value.contains(['\0', '\n', '\r', '\\'])
        && value
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

fn bound_text(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

#[derive(Debug, Error)]
pub enum FileBridgeError {
    #[error("file must be regular and no larger than 8 GiB")]
    InvalidFile,
    #[error("file name is unsafe")]
    InvalidFileName,
    #[error("file MIME offer is invalid")]
    InvalidMime,
    #[error("file digest calculation failed")]
    Digest,
    #[error("file changed while it was being transferred")]
    FileChanged,
    #[error("file transfer was rejected: {0}")]
    Rejected(String),
    #[error("file negotiation timed out")]
    NegotiationTimeout,
    #[error("file negotiation channel closed")]
    NegotiationClosed,
    #[error("agent returned an invalid file acceptance")]
    InvalidAcceptance,
    #[error("agent returned an unsafe remote path")]
    UnsafeRemotePath,
    #[error("file transfer identifier is unknown")]
    UnknownTransfer,
    #[error("file transfer identifier is already active")]
    DuplicateTransfer,
    #[error("file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] ad_stream_client::TransportError),
}
