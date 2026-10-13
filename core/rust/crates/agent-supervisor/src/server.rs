use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ad_stream_client::{
    ArppSession, SshChannelIo, SshTransportAdapter, TransportAdapter, TransportError,
};
use ad_stream_protocol::{
    ArppMessage, AudioCodec, BulkMessage, Capabilities, CaptureBackend, ClipboardMessage,
    ControlMessage, DisplayMessage, InputMessage, MetricsMessage, VideoCodec, PROTOCOL_VERSION,
};
use ring::digest::{Context, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{AgentSupervisor, SupervisorConfig, SupervisorError};

const CHANNEL_MAGIC: [u8; 4] = *b"ADCH";
const CHANNEL_VERSION: u8 = 1;
const CHANNEL_PREFACE_BYTES: usize = 38;
const CHANNEL_SET_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILE_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub struct AgentServerConfig {
    pub supervisor: SupervisorConfig,
    pub session_directory: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentServerEndpoint {
    pub port: u16,
    pub token: [u8; 32],
}

pub struct AgentServer {
    config: AgentServerConfig,
    listener: TcpListener,
    endpoint: AgentServerEndpoint,
    initial_width: u32,
    initial_height: u32,
}

impl AgentServer {
    pub async fn bind(config: AgentServerConfig) -> Result<Self, AgentServerError> {
        validate_session_directory(&config.session_directory)?;
        tokio::fs::create_dir_all(&config.session_directory).await?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let mut token = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| AgentServerError::Random)?;
        let initial_width = config.supervisor.encoder.width;
        let initial_height = config.supervisor.encoder.height;
        Ok(Self {
            config,
            listener,
            endpoint: AgentServerEndpoint { port, token },
            initial_width,
            initial_height,
        })
    }

    pub fn endpoint(&self) -> &AgentServerEndpoint {
        &self.endpoint
    }

    pub async fn run(self) -> Result<(), AgentServerError> {
        let supervisor = AgentSupervisor::start(self.config.supervisor.clone()).await?;
        let mut dimensions = (self.initial_width, self.initial_height);
        info!(port = self.endpoint.port, session_id = %supervisor.session_id(), "agent broker ready");
        let closing_session = loop {
            let channels = match self.accept_channel_set().await {
                Ok(channels) => channels,
                Err(AgentServerError::ChannelTimeout) => {
                    // A detached desktop has no expiration. The timeout only
                    // discards incomplete channel sets so their sockets cannot
                    // consume resources forever.
                    continue;
                }
                Err(error) => return Err(error),
            };
            match run_client(
                &supervisor,
                channels,
                &self.config.session_directory,
                &mut dimensions,
            )
            .await
            {
                Ok(ClientDisposition::Detached) => {}
                Ok(ClientDisposition::Terminate(session)) => break session,
                Err(error) => warn!(%error, "client dropped; retaining remote session"),
            }
        };
        supervisor.shutdown().await?;
        // The desktop is stopped. Log/storage cleanup is best-effort and must
        // never suppress the authoritative acknowledgement of that fact.
        if let Err(error) = remove_tree_if_present(&self.config.session_directory).await {
            warn!(%error, "ended session storage could not be removed");
        }
        let _ = closing_session
            .send(ArppMessage::Control(ControlMessage::Close {
                reason_code: 0,
                detail: "Remote session ended".into(),
            }))
            .await;
        Ok(())
    }

    async fn accept_channel_set(&self) -> Result<[TcpStream; 3], AgentServerError> {
        timeout(CHANNEL_SET_TIMEOUT, async {
            let mut channels: [Option<TcpStream>; 3] = [None, None, None];
            while channels.iter().any(Option::is_none) {
                let (mut stream, peer) = self.listener.accept().await?;
                if !peer.ip().is_loopback() {
                    continue;
                }
                stream.set_nodelay(true)?;
                let mut prelude = [0_u8; CHANNEL_PREFACE_BYTES];
                if !matches!(
                    timeout(Duration::from_secs(3), stream.read_exact(&mut prelude)).await,
                    Ok(Ok(_))
                ) || prelude[..4] != CHANNEL_MAGIC
                    || prelude[4] != CHANNEL_VERSION
                {
                    warn!(%peer, "rejected incomplete or invalid agent channel");
                    continue;
                }
                let channel = usize::from(prelude[5]);
                if channel >= 3 || channels[channel].is_some() {
                    warn!(%peer, channel, "rejected duplicate agent channel");
                    continue;
                }
                if !bool::from(prelude[6..].ct_eq(&self.endpoint.token)) {
                    warn!(%peer, "rejected unauthenticated agent channel");
                    continue;
                }
                channels[channel] = Some(stream);
            }
            Ok::<_, AgentServerError>([
                channels[0].take().expect("control channel"),
                channels[1].take().expect("input channel"),
                channels[2].take().expect("media channel"),
            ])
        })
        .await
        .map_err(|_| AgentServerError::ChannelTimeout)?
    }
}

enum ClientDisposition {
    Detached,
    Terminate(Arc<ArppSession>),
}

async fn run_client(
    supervisor: &AgentSupervisor,
    channels: [TcpStream; 3],
    session_directory: &Path,
    dimensions: &mut (u32, u32),
) -> Result<ClientDisposition, AgentServerError> {
    let [control, input, media] = channels;
    let adapter: Arc<dyn TransportAdapter> = Arc::new(SshTransportAdapter::new(
        SshChannelIo::from_stream(control),
        SshChannelIo::from_stream(input),
        SshChannelIo::from_stream(media),
    ));
    let session = Arc::new(ArppSession::new(adapter));
    let client_id = Uuid::new_v4();
    let mut attached = false;
    let (mut width, mut height) = *dimensions;
    let mut packets = supervisor.subscribe_packets();
    let delivery = crate::delivery::DeliveryWindow::default();
    let mut clipboard_events = supervisor.subscribe_clipboards();
    let mut files = AgentFileReceiver::new(session_directory.join("drops"));
    let mut clipboard = AgentClipboardRelay::default();
    let held = std::sync::Mutex::new(HeldInput::default());
    let (received_tx, mut received_rx) = tokio::sync::mpsc::channel(64);
    // This future continues polling the input channel while file/clipboard
    // handlers await I/O. Overloaded non-input consumers fail explicitly.
    let receive_input = async {
        loop {
            let frame = match session.receive().await {
                Ok(frame) => frame,
                Err(error) => {
                    // Keep EOF behind an already received Close command.
                    let _ = received_tx.send(Err(error)).await;
                    return std::future::pending().await;
                }
            };
            if let ArppMessage::Input(input) = frame.message {
                supervisor.inject_input(&input).await?;
                held.lock().expect("input state").record(&input);
            } else if let ArppMessage::Control(ControlMessage::Acknowledge { sequence }) =
                frame.message
            {
                if let Some(delay) = delivery.acknowledge(sequence) {
                    supervisor.report_delivery(delay, false);
                }
            } else if received_tx.try_send(Ok(frame)).is_err() {
                return Err::<ClientDisposition, AgentServerError>(
                    TransportError::Connection("Agent control queue overloaded".into()).into(),
                );
            }
        }
    };

    // Poll the media writer alongside input/control. A slow media channel must
    // never hold the control loop while a whole encoded frame is being written.
    let media_output = async {
        let mut needs_keyframe = true;
        loop {
            tokio::time::timeout(Duration::from_secs(15), delivery.wait_ready())
                .await
                .map_err(|_| {
                    TransportError::Connection("Video receipt acknowledgement timed out".into())
                })?;
            match packets.recv().await {
                Ok(packet) => {
                    if needs_keyframe && !packet.key_frame {
                        continue;
                    }
                    needs_keyframe = false;
                    let started = std::time::Instant::now();
                    let acknowledged_delivery = delivery.sent(packet.sequence, packet.data.len());
                    session
                        .send(ArppMessage::Display(DisplayMessage::EncodedFrame {
                            frame_id: packet.sequence,
                            presentation_time_us: packet.presentation_time_ns / 1_000,
                            codec: packet.codec,
                            key_frame: packet.key_frame,
                            width: packet.width,
                            height: packet.height,
                            data: packet.data,
                        }))
                        .await?;
                    if !acknowledged_delivery {
                        supervisor.report_delivery(started.elapsed(), false);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Discard old history and resume only with a fresh independent frame.
                    packets = packets.resubscribe();
                    needs_keyframe = true;
                    supervisor.request_refresh().await?;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Err::<(), AgentServerError>(AgentServerError::SupervisorStopped)
                }
            }
        }
    };
    tokio::pin!(media_output);

    let control_loop = async {
        let disposition = loop {
            tokio::select! {
                clipboard_event = clipboard_events.recv(), if attached => match clipboard_event {
                    Ok(value) => clipboard.publish_remote(&session, value.mime_type, value.data).await?,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Err(AgentServerError::SupervisorStopped),
                },
                received = received_rx.recv() => {
                    let frame = match received {
                        Some(Ok(frame)) => frame,
                        Some(Err(TransportError::Closed | TransportError::Io(_))) => break ClientDisposition::Detached,
                        Some(Err(error)) => return Err(error.into()),
                        None => break ClientDisposition::Detached,
                    };
                    match frame.message {
                        ArppMessage::Control(ControlMessage::ClientHello { protocol_version, .. }) => {
                            if protocol_version != PROTOCOL_VERSION {
                                break ClientDisposition::Detached;
                            }
                            session.send(ArppMessage::Control(ControlMessage::ServerHello {
                                protocol_version: PROTOCOL_VERSION,
                                server_nonce: random_token()?,
                            })).await?;
                            session.send(ArppMessage::Control(ControlMessage::Capabilities(capabilities()))).await?;
                        }
                        ArppMessage::Control(ControlMessage::StartSession(parameters)) => {
                            ensure_session(supervisor, parameters.session_id)?;
                            width = parameters.width;
                            height = parameters.height;
                            supervisor.resize(width, height).await?;
                            if !attached {
                                supervisor.attach_client(client_id).await?;
                                attached = true;
                            }
                            session.send(ArppMessage::Control(ControlMessage::SessionReady {
                                session_id: supervisor.session_id(),
                                resumed: false,
                            })).await?;
                        }
                        ArppMessage::Control(ControlMessage::ResumeSession { session_id, .. }) => {
                            ensure_session(supervisor, session_id)?;
                            if !attached {
                                supervisor.attach_client(client_id).await?;
                                attached = true;
                            }
                            session.send(ArppMessage::Control(ControlMessage::ResumeState {
                                session_id,
                                cache_epoch: 0,
                                width,
                                height,
                                full_refresh_required: true,
                            })).await?;
                        }
                        ArppMessage::Control(ControlMessage::Resize { width: next_width, height: next_height, .. }) => {
                            if attached && ad_stream_protocol::valid_desktop_size(next_width, next_height) {
                                width = next_width;
                                height = next_height;
                                supervisor.set_monitors(width, height, vec![ad_stream_protocol::Rectangle { x: 0, y: 0, width, height }]).await?;
                            }
                        }
                        ArppMessage::Control(ControlMessage::RequestKeyFrame { .. }) => {
                            if attached { delivery.recover(); supervisor.request_refresh().await?; }
                        }
                        ArppMessage::Control(ControlMessage::KeyboardLayout { layout, variant }) => {
                            if attached && ad_stream_protocol::valid_keyboard_layout(&layout, &variant) { supervisor.set_keyboard_layout(layout, variant).await?; }
                        }
                        ArppMessage::Control(ControlMessage::MonitorLayout { width: w, height: h, monitors }) => {
                            if attached && ad_stream_protocol::valid_monitor_layout(w, h, &monitors) {
                                supervisor.set_monitors(w, h, monitors).await?; width = w; height = h;
                            }
                        }
                        ArppMessage::Control(ControlMessage::Close { reason_code, .. }) => {
                            break if reason_code == 0 { ClientDisposition::Terminate(session.clone()) } else { ClientDisposition::Detached };
                        }
                        ArppMessage::Control(message @ ControlMessage::FileOffer { .. }) => {
                            files.offer(&session, message).await?;
                        }
                        ArppMessage::Input(_) => unreachable!("input is handled by its own receiver"),
                        ArppMessage::Clipboard(message) => {
                            if let Some((mime_type, data)) = clipboard.handle(&session, message).await? {
                                supervisor.set_clipboard(mime_type, data).await?;
                            }
                        }
                        ArppMessage::Bulk(message) => files.handle(&session, message).await?,
                        ArppMessage::Metrics(MetricsMessage::Ping { nonce, sent_at_us }) => {
                            session.send(ArppMessage::Metrics(MetricsMessage::Pong { nonce, sent_at_us, received_at_us: sent_at_us })).await?;
                        }
                        ArppMessage::Metrics(MetricsMessage::DecoderLoad { queue_depth, dropped_frames, .. }) if attached => {
                            supervisor.report_delivery(Duration::ZERO, queue_depth > 0 || dropped_frames > 0);
                        }
                        ArppMessage::Control(_) | ArppMessage::Display(_) | ArppMessage::Audio(_) | ArppMessage::Metrics(_) => {}
                    }
                }
            }
        };
        Ok::<_, AgentServerError>(disposition)
    };
    let result = {
        tokio::pin!(control_loop);
        tokio::select! {
            result = &mut control_loop => result,
            result = receive_input => result,
            result = &mut media_output => {
                result.and(Err(AgentServerError::SupervisorStopped))
            }
        }
    };
    *dimensions = (width, height);
    // Termination stops the whole supervisor. Releasing held input and
    // pausing media are detach-only work; a failed pipeline here must not
    // discard an already accepted request to end the desktop.
    if attached && !matches!(&result, Ok(ClientDisposition::Terminate(_))) {
        let releases = held.lock().expect("input state").releases();
        for release in releases {
            supervisor.inject_input(&release).await?;
        }
        supervisor.detach_client(client_id).await?;
    }
    result
}

#[derive(Default)]
struct HeldInput {
    keys: HashSet<u32>,
    buttons: HashSet<u8>,
    position: (i32, i32),
}

impl HeldInput {
    fn record(&mut self, input: &InputMessage) {
        match *input {
            InputMessage::Keyboard {
                scan_code, pressed, ..
            } => {
                if pressed {
                    self.keys.insert(scan_code);
                } else {
                    self.keys.remove(&scan_code);
                }
            }
            InputMessage::PointerButton {
                button,
                pressed,
                x,
                y,
            } => {
                self.position = (x, y);
                if pressed {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
            }
            InputMessage::PointerMove { x, y, .. } => self.position = (x, y),
            _ => {}
        }
    }

    fn releases(&mut self) -> Vec<InputMessage> {
        let mut events: Vec<_> = self
            .keys
            .drain()
            .map(|scan_code| InputMessage::Keyboard {
                scan_code,
                pressed: false,
                modifiers: 0,
                repeat: false,
            })
            .collect();
        events.extend(
            self.buttons
                .drain()
                .map(|button| InputMessage::PointerButton {
                    button,
                    pressed: false,
                    x: self.position.0,
                    y: self.position.1,
                }),
        );
        events
    }
}

fn ensure_session(supervisor: &AgentSupervisor, session_id: Uuid) -> Result<(), AgentServerError> {
    if supervisor.session_id() == session_id {
        Ok(())
    } else {
        Err(AgentServerError::SessionMismatch)
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        protocol_version: PROTOCOL_VERSION,
        video_codecs: vec![VideoCodec::H264, VideoCodec::Av1],
        audio_codecs: vec![AudioCodec::Opus],
        capture_backends: vec![CaptureBackend::Wayland, CaptureBackend::X11],
        maximum_width: 16_384,
        maximum_height: 16_384,
        maximum_monitors: 16,
        clipboard: true,
        audio: false,
        dynamic_resize: true,
        quic_media: false,
    }
}

fn random_token() -> Result<[u8; 32], AgentServerError> {
    let mut value = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| AgentServerError::Random)?;
    Ok(value)
}

#[derive(Default)]
struct AgentClipboardRelay {
    offers: HashMap<u64, Vec<String>>,
    values: HashMap<String, Vec<u8>>,
    published: HashMap<u64, String>,
    pending: HashMap<(u64, String), Vec<u8>>,
    next_revision: u64,
    last_injected: Option<(String, Vec<u8>)>,
    last_sent_at: Option<std::time::Instant>,
}

const CLIPBOARD_LIMIT_BYTES: usize = 10 * 1024 * 1024;
const CLIPBOARD_CHUNK_BYTES: usize = 1024 * 1024;

impl AgentClipboardRelay {
    async fn publish_remote(
        &mut self,
        session: &Arc<ArppSession>,
        mime_type: String,
        data: Vec<u8>,
    ) -> Result<(), AgentServerError> {
        if data.is_empty() || data.len() > CLIPBOARD_LIMIT_BYTES {
            return Err(AgentServerError::UnsupportedClipboard);
        }
        if self
            .last_injected
            .as_ref()
            .is_some_and(|(mime, value)| mime == &mime_type && value == &data)
        {
            self.last_injected = None;
            return Ok(());
        }
        if self
            .last_sent_at
            .is_some_and(|instant| instant.elapsed() < Duration::from_millis(100))
        {
            return Ok(());
        }
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        self.values.insert(mime_type.clone(), data);
        self.published.insert(self.next_revision, mime_type.clone());
        self.last_sent_at = Some(std::time::Instant::now());
        session
            .send(ArppMessage::Clipboard(ClipboardMessage::Offer {
                revision: self.next_revision,
                mime_types: vec![mime_type],
            }))
            .await?;
        Ok(())
    }

    async fn handle(
        &mut self,
        session: &Arc<ArppSession>,
        message: ClipboardMessage,
    ) -> Result<Option<(String, Vec<u8>)>, AgentServerError> {
        match message {
            ClipboardMessage::Offer {
                revision,
                mime_types,
            } => {
                let requested = mime_types
                    .iter()
                    .find(|mime| {
                        matches!(
                            mime.as_str(),
                            "text/plain;charset=utf-8" | "text/plain" | "text/html" | "image/png"
                        )
                    })
                    .cloned()
                    .ok_or(AgentServerError::UnsupportedClipboard)?;
                self.offers.insert(revision, mime_types);
                session
                    .send(ArppMessage::Clipboard(ClipboardMessage::Request {
                        revision,
                        mime_type: requested,
                    }))
                    .await?;
                Ok(None)
            }
            ClipboardMessage::Data {
                revision,
                mime_type,
                data,
            } => {
                let offered = self
                    .offers
                    .remove(&revision)
                    .ok_or(AgentServerError::ClipboardRevision)?;
                if !offered.contains(&mime_type) || data.len() > CLIPBOARD_LIMIT_BYTES {
                    return Err(AgentServerError::UnsupportedClipboard);
                }
                self.values.insert(mime_type.clone(), data.clone());
                self.last_injected = Some((mime_type.clone(), data.clone()));
                Ok(Some((mime_type, data)))
            }
            ClipboardMessage::DataChunk {
                revision,
                mime_type,
                offset,
                total_size,
                data,
            } => {
                if total_size == 0
                    || total_size > CLIPBOARD_LIMIT_BYTES as u64
                    || data.is_empty()
                    || data.len() > CLIPBOARD_CHUNK_BYTES
                    || !self
                        .offers
                        .get(&revision)
                        .is_some_and(|formats| formats.contains(&mime_type))
                {
                    return Err(AgentServerError::UnsupportedClipboard);
                }
                let key = (revision, mime_type.clone());
                let value = self.pending.entry(key.clone()).or_default();
                if offset != value.len() as u64
                    || offset.saturating_add(data.len() as u64) > total_size
                {
                    self.pending.remove(&key);
                    return Err(AgentServerError::ClipboardRevision);
                }
                value.extend_from_slice(&data);
                if value.len() as u64 != total_size {
                    return Ok(None);
                }
                let complete = self
                    .pending
                    .remove(&key)
                    .ok_or(AgentServerError::ClipboardRevision)?;
                self.offers.remove(&revision);
                self.values.insert(mime_type.clone(), complete.clone());
                self.last_injected = Some((mime_type.clone(), complete.clone()));
                Ok(Some((mime_type, complete)))
            }
            ClipboardMessage::Request {
                revision,
                mime_type,
            } => {
                let requested = self
                    .published
                    .remove(&revision)
                    .ok_or(AgentServerError::ClipboardRevision)?;
                if requested != mime_type {
                    return Err(AgentServerError::ClipboardRevision);
                }
                if let Some(data) = self.values.get(&mime_type) {
                    if data.len() <= CLIPBOARD_CHUNK_BYTES {
                        session
                            .send(ArppMessage::Clipboard(ClipboardMessage::Data {
                                revision,
                                mime_type,
                                data: data.clone(),
                            }))
                            .await?;
                    } else {
                        let total_size = data.len() as u64;
                        for (index, chunk) in data.chunks(CLIPBOARD_CHUNK_BYTES).enumerate() {
                            session
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
                Ok(None)
            }
        }
    }
}

struct PendingFile {
    name: String,
    size: u64,
    digest: [u8; 32],
    relative: String,
    absolute: PathBuf,
}

struct ActiveFile {
    file: File,
    pending: PendingFile,
    offset: u64,
}

struct AgentFileReceiver {
    directory: PathBuf,
    pending: HashMap<Uuid, PendingFile>,
    active: HashMap<Uuid, ActiveFile>,
}

impl AgentFileReceiver {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            pending: HashMap::new(),
            active: HashMap::new(),
        }
    }

    async fn offer(
        &mut self,
        session: &Arc<ArppSession>,
        message: ControlMessage,
    ) -> Result<(), AgentServerError> {
        let ControlMessage::FileOffer {
            transfer_id,
            file_name,
            size,
            mime_types,
            sha256,
        } = message
        else {
            return Ok(());
        };
        if size > MAX_FILE_BYTES
            || !safe_file_name(&file_name)
            || self.pending.contains_key(&transfer_id)
        {
            session
                .send(ArppMessage::Control(ControlMessage::FileReject {
                    transfer_id,
                    reason: "invalid file offer".into(),
                }))
                .await?;
            return Ok(());
        }
        let Some(mime_type) = mime_types.into_iter().find(|mime| safe_mime(mime)) else {
            session
                .send(ArppMessage::Control(ControlMessage::FileReject {
                    transfer_id,
                    reason: "unsupported MIME type".into(),
                }))
                .await?;
            return Ok(());
        };
        tokio::fs::create_dir_all(&self.directory).await?;
        let relative = format!("drops/{transfer_id}.part");
        let absolute = self.directory.join(format!("{transfer_id}.part"));
        let resume_offset = tokio::fs::metadata(&absolute)
            .await
            .map(|m| m.len().min(size))
            .unwrap_or(0);
        self.pending.insert(
            transfer_id,
            PendingFile {
                name: file_name,
                size,
                digest: sha256,
                relative: relative.clone(),
                absolute,
            },
        );
        session
            .send(ArppMessage::Control(ControlMessage::FileAccept {
                transfer_id,
                mime_type,
                resume_offset,
                staging_path: relative,
            }))
            .await?;
        Ok(())
    }

    async fn handle(
        &mut self,
        session: &Arc<ArppSession>,
        message: BulkMessage,
    ) -> Result<(), AgentServerError> {
        match message {
            BulkMessage::FileOpen {
                transfer_id,
                path,
                size,
                ..
            } => {
                let pending = self
                    .pending
                    .remove(&transfer_id)
                    .ok_or(AgentServerError::UnknownTransfer)?;
                if pending.relative != path || pending.size != size {
                    return Err(AgentServerError::UnsafeFilePath);
                }
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&pending.absolute)
                    .await?;
                let offset = file.metadata().await?.len().min(size);
                self.active.insert(
                    transfer_id,
                    ActiveFile {
                        file,
                        pending,
                        offset,
                    },
                );
            }
            BulkMessage::FileChunk {
                transfer_id,
                offset,
                data,
            } => {
                let active = self
                    .active
                    .get_mut(&transfer_id)
                    .ok_or(AgentServerError::UnknownTransfer)?;
                if offset != active.offset
                    || data.len() > MAX_FILE_CHUNK_BYTES
                    || offset
                        .checked_add(data.len() as u64)
                        .is_none_or(|end| end > active.pending.size)
                {
                    return Err(AgentServerError::InvalidFileChunk);
                }
                active.file.seek(std::io::SeekFrom::Start(offset)).await?;
                active.file.write_all(&data).await?;
                active.offset += data.len() as u64;
            }
            BulkMessage::FileClose {
                transfer_id,
                sha256,
            } => {
                let mut active = self
                    .active
                    .remove(&transfer_id)
                    .ok_or(AgentServerError::UnknownTransfer)?;
                active.file.flush().await?;
                drop(active.file);
                if active.offset != active.pending.size
                    || sha256 != active.pending.digest
                    || digest_file(&active.pending.absolute).await? != sha256
                {
                    return Err(AgentServerError::FileDigest);
                }
                let destination = self.directory.join(&active.pending.name);
                tokio::fs::rename(&active.pending.absolute, &destination).await?;
                session
                    .send(ArppMessage::Control(ControlMessage::FileReady {
                        transfer_id,
                        remote_uri: format!("file://{}", destination.to_string_lossy()),
                    }))
                    .await?;
            }
            BulkMessage::FileCancel { transfer_id, .. } => {
                if let Some(active) = self.active.remove(&transfer_id) {
                    drop(active.file);
                    let _ = tokio::fs::remove_file(active.pending.absolute).await;
                }
                self.pending.remove(&transfer_id);
            }
            BulkMessage::FileAcknowledge { .. } => {}
        }
        Ok(())
    }
}

async fn digest_file(path: &Path) -> Result<[u8; 32], AgentServerError> {
    let mut file = File::open(path).await?;
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
        .map_err(|_| AgentServerError::FileDigest)
}

fn safe_file_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', '\0', '\n', '\r'])
}

fn safe_mime(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn validate_session_directory(path: &Path) -> Result<(), AgentServerError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        Err(AgentServerError::UnsafeFilePath)
    } else {
        Ok(())
    }
}

async fn remove_tree_if_present(path: &Path) -> Result<(), AgentServerError> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Error)]
pub enum AgentServerError {
    #[error("agent I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent supervisor failed: {0}")]
    Supervisor(#[from] SupervisorError),
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] TransportError),
    #[error("secure random generation failed")]
    Random,
    #[error("agent channel preface is invalid")]
    InvalidChannelPrelude,
    #[error("agent channel authentication failed")]
    Authentication,
    #[error("agent channel is duplicated or invalid")]
    DuplicateChannel,
    #[error("agent channel set timed out")]
    ChannelTimeout,
    #[error("ARPP session identifier does not match")]
    SessionMismatch,
    #[error("agent supervisor stopped")]
    SupervisorStopped,
    #[error("clipboard format or revision is invalid")]
    UnsupportedClipboard,
    #[error("clipboard revision was not offered")]
    ClipboardRevision,
    #[error("file transfer is unknown")]
    UnknownTransfer,
    #[error("file transfer path is unsafe")]
    UnsafeFilePath,
    #[error("file transfer chunk is invalid")]
    InvalidFileChunk,
    #[error("file transfer digest does not match")]
    FileDigest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_releases_only_held_keys_and_buttons_once() {
        let mut held = HeldInput::default();
        for (scan_code, pressed) in [(30, true), (30, true), (31, true), (31, false)] {
            held.record(&InputMessage::Keyboard {
                scan_code,
                pressed,
                modifiers: 0,
                repeat: false,
            });
        }
        for (button, pressed) in [(1, true), (2, true), (2, false)] {
            held.record(&InputMessage::PointerButton {
                button,
                pressed,
                x: 10,
                y: 20,
            });
        }
        held.record(&InputMessage::PointerMove {
            x: 50,
            y: 60,
            monitor: 0,
        });
        let releases = held.releases();
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().any(|event| matches!(
            event,
            InputMessage::Keyboard {
                scan_code: 30,
                pressed: false,
                repeat: false,
                ..
            }
        )));
        assert!(releases.iter().any(|event| matches!(
            event,
            InputMessage::PointerButton {
                button: 1,
                pressed: false,
                x: 50,
                y: 60
            }
        )));
        assert!(held.releases().is_empty());
    }

    #[tokio::test]
    async fn detached_desktop_keeps_computing_and_reopens_with_its_previous_size() {
        use agent_wayland::compositor::{ApplicationSpec, CompositorConfig};
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("session");
        let runtime = directory.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let progress = directory.join("calculation");
        let child_progress = root.path().join("background-calculation");
        let mut application = ApplicationSpec::new("/bin/sh");
        application.arguments = vec![
            "-c".into(),
            "(while :; do printf x >> \"$2\"; sleep 0.1; done) & printf started > \"$1\"; sleep 1; printf completed > \"$1\"; wait".into(),
            "retained-desktop-test".into(),
            progress.as_os_str().into(),
            child_progress.as_os_str().into(),
        ];
        let session_id = Uuid::new_v4();
        let server = AgentServer::bind(AgentServerConfig {
            supervisor: SupervisorConfig {
                session_id,
                compositor: CompositorConfig {
                    width: 320,
                    height: 200,
                    refresh_millihz: 30_000,
                    runtime_dir: runtime,
                    enable_xwayland: false,
                    host_window_manager: false,
                },
                input: agent_input::InputConfig::default(),
                encoder: agent_encoder::EncoderConfig {
                    width: 320,
                    height: 200,
                    framerate: 30,
                    bitrate_kbps: 512,
                    keyframe_interval: 30,
                    codec: VideoCodec::H264,
                },
                application,
            },
            session_directory: directory.clone(),
        })
        .await
        .unwrap();
        let endpoint = server.endpoint().clone();
        let task = tokio::spawn(server.run());
        async fn open(endpoint: &AgentServerEndpoint) -> Arc<ArppSession> {
            let mut streams = Vec::new();
            for channel in 0..3 {
                let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port))
                    .await
                    .unwrap();
                let mut prelude = [0; 38];
                prelude[..4].copy_from_slice(&CHANNEL_MAGIC);
                prelude[4] = CHANNEL_VERSION;
                prelude[5] = channel;
                prelude[6..].copy_from_slice(&endpoint.token);
                stream.write_all(&prelude).await.unwrap();
                streams.push(stream);
            }
            let mut streams = streams.into_iter();
            let session = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
                SshChannelIo::from_stream(streams.next().unwrap()),
                SshChannelIo::from_stream(streams.next().unwrap()),
                SshChannelIo::from_stream(streams.next().unwrap()),
            ))));
            session
                .send(ArppMessage::Control(ControlMessage::ClientHello {
                    protocol_version: PROTOCOL_VERSION,
                    client_nonce: [1; 32],
                }))
                .await
                .unwrap();
            for _ in 0..2 {
                timeout(Duration::from_secs(5), session.receive())
                    .await
                    .unwrap()
                    .unwrap();
            }
            session
        }
        let first = open(&endpoint).await;
        first
            .send(ArppMessage::Control(ControlMessage::ResumeSession {
                session_id,
                last_presented_frame: 0,
                cache_epoch: 0,
                cache_digest: [0; 32],
            }))
            .await
            .unwrap();
        timeout(Duration::from_secs(5), first.receive())
            .await
            .unwrap()
            .unwrap();
        first
            .send(ArppMessage::Control(ControlMessage::Resize {
                width: 640,
                height: 400,
                scale_milli: 1000,
            }))
            .await
            .unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                if let ArppMessage::Display(DisplayMessage::EncodedFrame {
                    width: 640,
                    height: 400,
                    ..
                }) = first.receive().await.unwrap().message
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        first
            .send(ArppMessage::Control(ControlMessage::Close {
                reason_code: 1,
                detail: "client detached".into(),
            }))
            .await
            .unwrap();
        first.close("test client closed").await.unwrap();
        drop(first);
        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert_eq!(
            std::fs::read_to_string(&progress).unwrap(),
            "completed",
            "calculation stopped while the client was closed"
        );
        assert!(
            !task.is_finished(),
            "detaching terminated the desktop server"
        );
        let second = open(&endpoint).await;
        second
            .send(ArppMessage::Control(ControlMessage::ResumeSession {
                session_id,
                last_presented_frame: 0,
                cache_epoch: 0,
                cache_digest: [0; 32],
            }))
            .await
            .unwrap();
        let reply = timeout(Duration::from_secs(5), second.receive())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(reply.message,ArppMessage::Control(ControlMessage::ResumeState {session_id:id,width:640,height:400,..}) if id==session_id)
        );
        // An unexpected transport loss also retains work and its dimensions.
        second.close("simulated network loss").await.unwrap();
        drop(second);
        let third = open(&endpoint).await;
        third
            .send(ArppMessage::Control(ControlMessage::ResumeSession {
                session_id,
                last_presented_frame: 0,
                cache_epoch: 0,
                cache_digest: [0; 32],
            }))
            .await
            .unwrap();
        let reply = timeout(Duration::from_secs(5), third.receive())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            reply.message,
            ArppMessage::Control(ControlMessage::ResumeState {
                width: 640,
                height: 400,
                ..
            })
        ));
        third
            .send(ArppMessage::Control(ControlMessage::Close {
                reason_code: 0,
                detail: "explicit end session".into(),
            }))
            .await
            .unwrap();
        timeout(Duration::from_secs(10), async {
            loop {
                if matches!(
                    third.receive().await.unwrap().message,
                    ArppMessage::Control(ControlMessage::Close { reason_code: 0, .. })
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            !directory.exists(),
            "termination acknowledged before session cleanup"
        );
        let stopped = std::fs::metadata(&child_progress).unwrap().len();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(
            std::fs::metadata(&child_progress).unwrap().len(),
            stopped,
            "background calculation survived explicit session termination"
        );
        third.close("termination acknowledged").await.unwrap();
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn rejects_parent_components_in_session_storage() {
        assert!(validate_session_directory(Path::new("safe/session")).is_ok());
        assert!(validate_session_directory(Path::new("safe/../escape")).is_err());
    }

    #[test]
    fn restricts_uploaded_file_names() {
        assert!(safe_file_name("report.png"));
        assert!(!safe_file_name("../report.png"));
        assert!(!safe_file_name("nested/report.png"));
    }
}
