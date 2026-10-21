//! Aether Remote Presentation Protocol (ARPP) wire contract.
//!
//! ARPP uses a bounded, length-prefixed binary frame. The fixed header carries
//! redundant class and priority fields so a receiver can reject incorrectly
//! routed or malicious frames before dispatching them to a subsystem.

pub mod framing;
pub mod lossless;
pub mod messages;
pub mod pointer;
pub mod transport;

use bincode::Options;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 3;
pub const FRAME_MAGIC: [u8; 4] = *b"ARPP";
pub const MAX_PAYLOAD_LEN: usize = 32 * 1024 * 1024;
const FIXED_HEADER_LEN: usize = 4 + 2 + 1 + 1 + 2 + 8 + 4;
pub const MAX_FRAME_BODY_LEN: usize = FIXED_HEADER_LEN + MAX_PAYLOAD_LEN;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessagePriority {
    Highest = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

impl TryFrom<u8> for MessagePriority {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Highest),
            1 => Ok(Self::High),
            2 => Ok(Self::Medium),
            3 => Ok(Self::Low),
            _ => Err(ProtocolError::InvalidPriority(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageClass {
    Control = 0,
    Input = 1,
    Display = 2,
    Clipboard = 3,
    Audio = 4,
    Metrics = 5,
    Bulk = 6,
}

impl MessageClass {
    pub const fn priority(self) -> MessagePriority {
        match self {
            Self::Input => MessagePriority::Highest,
            Self::Control | Self::Clipboard => MessagePriority::High,
            Self::Display | Self::Audio => MessagePriority::Medium,
            Self::Metrics | Self::Bulk => MessagePriority::Low,
        }
    }

    pub const fn ssh_channel(self) -> SshChannelId {
        match self {
            Self::Input => SshChannelId::Input,
            Self::Display | Self::Clipboard | Self::Audio => SshChannelId::Media,
            Self::Control | Self::Metrics | Self::Bulk => SshChannelId::Control,
        }
    }
}

impl TryFrom<u8> for MessageClass {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::Input),
            2 => Ok(Self::Display),
            3 => Ok(Self::Clipboard),
            4 => Ok(Self::Audio),
            5 => Ok(Self::Metrics),
            6 => Ok(Self::Bulk),
            _ => Err(ProtocolError::InvalidClass(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SshChannelId {
    Control = 0,
    Input = 1,
    Media = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Jpeg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AudioCodec {
    Opus,
    PcmS16Le,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CaptureBackend {
    PipeWire,
    Wayland,
    X11,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PixelFormat {
    Bgra8888,
    Rgba8888,
    Nv12,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CacheNamespace {
    Tile,
    Glyph,
}

pub fn cache_content_key(
    namespace: CacheNamespace,
    format: PixelFormat,
    width: u32,
    height: u32,
    data: &[u8],
) -> [u8; 32] {
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    digest.update(&[namespace as u8, format as u8]);
    digest.update(&width.to_be_bytes());
    digest.update(&height.to_be_bytes());
    digest.update(data);
    digest
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub protocol_version: u16,
    pub video_codecs: Vec<VideoCodec>,
    pub audio_codecs: Vec<AudioCodec>,
    pub capture_backends: Vec<CaptureBackend>,
    pub maximum_width: u32,
    pub maximum_height: u32,
    pub maximum_monitors: u8,
    pub clipboard: bool,
    pub audio: bool,
    pub dynamic_resize: bool,
    pub quic_media: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionParameters {
    pub session_id: Uuid,
    pub desktop_environment: String,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u16,
    pub monitors: u8,
    pub video_codec: VideoCodec,
    pub audio_codec: Option<AudioCodec>,
    pub target_fps: u16,
    pub target_bitrate_kbps: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuicBootstrap {
    pub session_id: Uuid,
    pub one_time_token: [u8; 32],
    pub host: String,
    pub port: u16,
    pub server_name: String,
    pub expires_unix_ms: u64,
}

impl QuicBootstrap {
    pub fn generate(
        session_id: Uuid,
        host: String,
        port: u16,
        server_name: String,
        now_unix_ms: u64,
        lifetime_ms: u64,
    ) -> Result<Self, ProtocolError> {
        if lifetime_ms == 0 || lifetime_ms > 120_000 {
            return Err(ProtocolError::InvalidBootstrap(
                "token lifetime must be between 1 ms and 120 seconds",
            ));
        }
        let mut one_time_token = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut one_time_token)
            .map_err(|_| ProtocolError::RandomGeneration)?;
        let expires_unix_ms = now_unix_ms
            .checked_add(lifetime_ms)
            .ok_or(ProtocolError::LengthOverflow)?;
        let bootstrap = Self {
            session_id,
            one_time_token,
            host,
            port,
            server_name,
            expires_unix_ms,
        };
        bootstrap.validate_at(now_unix_ms)?;
        Ok(bootstrap)
    }

    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), ProtocolError> {
        if self.port == 0 || self.host.is_empty() || self.server_name.is_empty() {
            return Err(ProtocolError::InvalidBootstrap("missing QUIC endpoint"));
        }
        if self.one_time_token.iter().all(|byte| *byte == 0) {
            return Err(ProtocolError::InvalidBootstrap("token is all zeroes"));
        }
        if self.expires_unix_ms <= now_unix_ms {
            return Err(ProtocolError::ExpiredBootstrap);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ControlMessage {
    ClientHello {
        protocol_version: u16,
        client_nonce: [u8; 32],
    },
    ServerHello {
        protocol_version: u16,
        server_nonce: [u8; 32],
    },
    Capabilities(Capabilities),
    StartSession(SessionParameters),
    SessionReady {
        session_id: Uuid,
        resumed: bool,
    },
    ResumeSession {
        session_id: Uuid,
        last_presented_frame: u64,
        cache_epoch: u64,
        cache_digest: [u8; 32],
    },
    ResumeState {
        session_id: Uuid,
        cache_epoch: u64,
        width: u32,
        height: u32,
        full_refresh_required: bool,
    },
    StateSnapshotComplete {
        session_id: Uuid,
        frame_id: u64,
        cache_epoch: u64,
    },
    Resize {
        width: u32,
        height: u32,
        scale_milli: u16,
    },
    RequestKeyFrame {
        last_decoded_sequence: u64,
    },
    FileOffer {
        transfer_id: Uuid,
        file_name: String,
        size: u64,
        mime_types: Vec<String>,
        sha256: [u8; 32],
    },
    FileAccept {
        transfer_id: Uuid,
        mime_type: String,
        resume_offset: u64,
        staging_path: String,
    },
    FileReject {
        transfer_id: Uuid,
        reason: String,
    },
    FileReady {
        transfer_id: Uuid,
        remote_uri: String,
    },
    QuicBootstrap(QuicBootstrap),
    QuicAuthenticate {
        session_id: Uuid,
        one_time_token: [u8; 32],
    },
    Acknowledge {
        /// Receipt of an EncodedFrame's frame_id; zero opts into receipt flow control.
        sequence: u64,
    },
    Close {
        reason_code: u16,
        detail: String,
    },
    /// XKB layout corresponding to the client's active input language.
    KeyboardLayout {
        layout: String,
        variant: String,
    },
    /// A single desktop canvas partitioned across the client's physical monitors.
    MonitorLayout {
        width: u32,
        height: u32,
        monitors: Vec<Rectangle>,
    },
    /// Extended layout; appended to preserve the legacy bincode variant and payload.
    MonitorLayoutWithProperties {
        width: u32,
        height: u32,
        monitors: Vec<Rectangle>,
        properties: Vec<MonitorProperties>,
    },
}

/// Client monitor metadata applies only to this session's virtual displays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorProperties {
    pub name: String,
    pub dpi: u16,
    pub width_mm: u32,
    pub height_mm: u32,
    pub refresh_millihz: u32,
    pub primary: bool,
}
pub fn valid_monitor_properties(count: usize, properties: &[MonitorProperties]) -> bool {
    properties.is_empty()
        || (properties.len() == count
            && properties.iter().all(|p| {
                !p.name.is_empty()
                    && p.name.len() <= 128
                    && !p.name.chars().any(char::is_control)
                    && (48..=768).contains(&p.dpi)
                    && (10..=10000).contains(&p.width_mm)
                    && (10..=10000).contains(&p.height_mm)
                    && (1000..=240000).contains(&p.refresh_millihz)
            }))
}
/// Bound desktop allocations while permitting wide multi-monitor arrangements.
pub fn valid_desktop_size(width: u32, height: u32) -> bool {
    width >= 64
        && height >= 64
        && width <= 16_384
        && height <= 16_384
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
        && u64::from(width) * u64::from(height) <= 33_554_432
}

pub fn valid_monitor_layout(width: u32, height: u32, monitors: &[Rectangle]) -> bool {
    valid_desktop_size(width, height)
        && !monitors.is_empty()
        && monitors.len() <= 16
        && monitors.iter().all(|m| {
            m.width > 0
                && m.height > 0
                && m.x.checked_add(m.width).is_some_and(|right| right <= width)
                && m.y
                    .checked_add(m.height)
                    .is_some_and(|bottom| bottom <= height)
        })
}

pub fn valid_keyboard_layout(layout: &str, variant: &str) -> bool {
    !layout.is_empty()
        && layout.len() <= 32
        && variant.len() <= 48
        && layout
            .bytes()
            .chain(variant.bytes())
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

/// Keyboard modifier bit indicating that Num Lock is enabled on the client.
pub const MODIFIER_NUM_LOCK: u16 = 1 << 4;
/// Keyboard modifier bit indicating that Caps Lock is enabled on the client.
pub const MODIFIER_CAPS_LOCK: u16 = 1 << 5;
/// Lock-state bits are authoritative only when this bit is set. Older clients
/// continue to toggle locks by sending physical key presses.
pub const MODIFIER_LOCK_STATE_VALID: u16 = 1 << 15;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InputMessage {
    Keyboard {
        scan_code: u32,
        pressed: bool,
        /// Bits 0..3: Shift, Ctrl, Alt, Super. Bits 4..5: Num Lock, Caps Lock;
        /// lock bits apply only with MODIFIER_LOCK_STATE_VALID.
        modifiers: u16,
        repeat: bool,
    },
    PointerMove {
        x: i32,
        y: i32,
        monitor: u8,
    },
    PointerButton {
        /// IDs in the shared `pointer` module; not X11 button numbers.
        button: u8,
        pressed: bool,
        x: i32,
        y: i32,
    },
    PointerWheel {
        /// 120 units per detent, positive to the right.
        horizontal: i32,
        /// 120 units per detent, positive upward. Wayland inverts this.
        vertical: i32,
    },
    Touch {
        contact_id: u32,
        phase: TouchPhase,
        x: i32,
        y: i32,
        pressure: u16,
    },
    UnicodeText {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TouchPhase {
    Begin,
    Update,
    End,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DisplayMessage {
    EncodedFrame {
        frame_id: u64,
        presentation_time_us: u64,
        codec: VideoCodec,
        key_frame: bool,
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    DamageRegions {
        frame_id: u64,
        regions: Vec<Rectangle>,
    },
    TileUpdate {
        frame_id: u64,
        tile_id: u32,
        region: Rectangle,
        format: PixelFormat,
        data: Vec<u8>,
    },
    CacheStore {
        frame_id: u64,
        cache_key: [u8; 32],
        namespace: CacheNamespace,
        region: Rectangle,
        format: PixelFormat,
        data: Vec<u8>,
    },
    CacheReference {
        frame_id: u64,
        cache_key: [u8; 32],
        namespace: CacheNamespace,
        region: Rectangle,
    },
    CacheReset {
        epoch: u64,
    },
    CursorUpdate {
        frame_id: u64,
        x: i32,
        y: i32,
        hotspot_x: u32,
        hotspot_y: u32,
        width: u32,
        height: u32,
        visible: bool,
        rgba: Vec<u8>,
    },
    /// Atomic lossless transaction, sent only after explicit client opt-in.
    /// base_frame_id=0 replaces the surface (including unused monitor gaps).
    LosslessBegin {
        frame_id: u64,
        base_frame_id: u64,
        width: u32,
        height: u32,
        tile_count: u32,
    },
    LosslessTile {
        frame_id: u64,
        tile: lossless::LosslessTile,
    },
    LosslessCommit {
        frame_id: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClipboardMessage {
    Offer {
        revision: u64,
        mime_types: Vec<String>,
    },
    Request {
        revision: u64,
        mime_type: String,
    },
    Data {
        revision: u64,
        mime_type: String,
        data: Vec<u8>,
    },
    DataChunk {
        revision: u64,
        mime_type: String,
        offset: u64,
        total_size: u64,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AudioMessage {
    Configure {
        codec: AudioCodec,
        sample_rate: u32,
        channels: u8,
    },
    Packet {
        sequence: u64,
        presentation_time_us: u64,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricsMessage {
    Ping {
        nonce: u64,
        sent_at_us: u64,
    },
    Pong {
        nonce: u64,
        sent_at_us: u64,
        received_at_us: u64,
    },
    NetworkSample {
        interval_ms: u32,
        delivered_bytes: u64,
        delivered_packets: u32,
        lost_packets: u32,
        rtt_micros: u64,
        jitter_micros: u64,
    },
    DecoderLoad {
        queue_depth: u16,
        decode_time_us: u32,
        dropped_frames: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BulkMessage {
    FileOpen {
        transfer_id: Uuid,
        path: String,
        size: u64,
        overwrite: bool,
    },
    FileChunk {
        transfer_id: Uuid,
        offset: u64,
        data: Vec<u8>,
    },
    FileAcknowledge {
        transfer_id: Uuid,
        contiguous_offset: u64,
    },
    FileClose {
        transfer_id: Uuid,
        sha256: [u8; 32],
    },
    FileCancel {
        transfer_id: Uuid,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArppMessage {
    Control(ControlMessage),
    Input(InputMessage),
    Display(DisplayMessage),
    Clipboard(ClipboardMessage),
    Audio(AudioMessage),
    Metrics(MetricsMessage),
    Bulk(BulkMessage),
}

impl ArppMessage {
    pub const fn class(&self) -> MessageClass {
        match self {
            Self::Control(_) => MessageClass::Control,
            Self::Input(_) => MessageClass::Input,
            Self::Display(_) => MessageClass::Display,
            Self::Clipboard(_) => MessageClass::Clipboard,
            Self::Audio(_) => MessageClass::Audio,
            Self::Metrics(_) => MessageClass::Metrics,
            Self::Bulk(_) => MessageClass::Bulk,
        }
    }

    pub const fn priority(&self) -> MessagePriority {
        self.class().priority()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArppFrame {
    pub sequence: u64,
    pub flags: u16,
    pub message: ArppMessage,
}

impl ArppFrame {
    pub const fn new(sequence: u64, message: ArppMessage) -> Self {
        Self {
            sequence,
            flags: 0,
            message,
        }
    }

    pub const fn class(&self) -> MessageClass {
        self.message.class()
    }

    pub const fn priority(&self) -> MessagePriority {
        self.message.priority()
    }
}

pub fn encode_frame(frame: &ArppFrame) -> Result<Bytes, ProtocolError> {
    let codec = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_PAYLOAD_LEN as u64);
    let payload = codec.serialize(&frame.message)?;
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(ProtocolError::PayloadTooLarge(payload.len()));
    }
    let body_len = FIXED_HEADER_LEN
        .checked_add(payload.len())
        .ok_or(ProtocolError::LengthOverflow)?;
    let body_len_u32 = u32::try_from(body_len).map_err(|_| ProtocolError::LengthOverflow)?;
    let payload_len_u32 =
        u32::try_from(payload.len()).map_err(|_| ProtocolError::LengthOverflow)?;
    let mut output = BytesMut::with_capacity(4 + body_len);
    output.put_u32(body_len_u32);
    output.extend_from_slice(&FRAME_MAGIC);
    output.put_u16(PROTOCOL_VERSION);
    output.put_u8(frame.class() as u8);
    output.put_u8(frame.priority() as u8);
    output.put_u16(frame.flags);
    output.put_u64(frame.sequence);
    output.put_u32(payload_len_u32);
    output.extend_from_slice(&payload);
    Ok(output.freeze())
}

pub fn decode_frame(input: &mut BytesMut) -> Result<Option<ArppFrame>, ProtocolError> {
    if input.len() < 4 {
        return Ok(None);
    }
    let body_len = u32::from_be_bytes(
        input[..4]
            .try_into()
            .map_err(|_| ProtocolError::TruncatedHeader)?,
    ) as usize;
    if body_len < FIXED_HEADER_LEN {
        return Err(ProtocolError::TruncatedHeader);
    }
    if body_len > MAX_FRAME_BODY_LEN {
        return Err(ProtocolError::PayloadTooLarge(body_len - FIXED_HEADER_LEN));
    }
    if input.len() < 4 + body_len {
        return Ok(None);
    }

    let mut body = input.split_to(4 + body_len);
    body.advance(4);
    let magic = body.split_to(4);
    if magic.as_ref() != FRAME_MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }
    let version = body.get_u16();
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch(version));
    }
    let wire_class = MessageClass::try_from(body.get_u8())?;
    let wire_priority = MessagePriority::try_from(body.get_u8())?;
    let flags = body.get_u16();
    let sequence = body.get_u64();
    let payload_len = body.get_u32() as usize;
    if payload_len != body.remaining() {
        return Err(ProtocolError::LengthMismatch {
            declared: payload_len,
            actual: body.remaining(),
        });
    }
    let codec = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_PAYLOAD_LEN as u64)
        .reject_trailing_bytes();
    let message: ArppMessage = codec.deserialize(body.as_ref())?;
    if message.class() != wire_class {
        return Err(ProtocolError::ClassMismatch);
    }
    if message.priority() != wire_priority {
        return Err(ProtocolError::PriorityMismatch);
    }
    Ok(Some(ArppFrame {
        sequence,
        flags,
        message,
    }))
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("ARPP frame has an invalid magic value")]
    InvalidMagic,
    #[error("ARPP protocol version {0} is unsupported")]
    VersionMismatch(u16),
    #[error("ARPP message class {0} is invalid")]
    InvalidClass(u8),
    #[error("ARPP message priority {0} is invalid")]
    InvalidPriority(u8),
    #[error("ARPP payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("ARPP frame length overflow")]
    LengthOverflow,
    #[error("ARPP frame header is truncated")]
    TruncatedHeader,
    #[error("ARPP payload length mismatch: declared {declared}, actual {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("ARPP message class does not match its wire header")]
    ClassMismatch,
    #[error("ARPP message priority does not match its wire header")]
    PriorityMismatch,
    #[error("ARPP QUIC bootstrap is invalid: {0}")]
    InvalidBootstrap(&'static str),
    #[error("ARPP QUIC bootstrap has expired")]
    ExpiredBootstrap,
    #[error("ARPP could not generate a cryptographically secure bootstrap token")]
    RandomGeneration,
    #[error("ARPP binary serialization failed: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_layout_limits_support_wide_monitors_without_unbounded_allocations() {
        use super::*;
        assert!(valid_desktop_size(11520, 2160));
        assert!(!valid_desktop_size(16384, 16384));
        assert!(!valid_monitor_layout(1920, 1080, &[]));
        assert!(!valid_monitor_layout(
            1920,
            1080,
            &[Rectangle {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 2
            }]
        ));
        assert!(valid_keyboard_layout("es", ""));
        assert!(!valid_keyboard_layout("../../xkb", ""));
        assert!(!valid_keyboard_layout("us", "intl;command"));
    }

    use super::*;

    #[test]
    fn round_trips_fragmented_input_frame() {
        let expected = ArppFrame::new(
            42,
            ArppMessage::Input(InputMessage::Keyboard {
                scan_code: 30,
                pressed: true,
                modifiers: 2,
                repeat: false,
            }),
        );
        let encoded = encode_frame(&expected).unwrap();
        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(&encoded[..7]);
        assert!(decode_frame(&mut buffer).unwrap().is_none());
        buffer.extend_from_slice(&encoded[7..]);
        assert_eq!(decode_frame(&mut buffer).unwrap(), Some(expected));
        assert!(buffer.is_empty());
    }

    #[test]
    fn channel_and_priority_are_derived_from_message_class() {
        let input = ArppMessage::Input(InputMessage::PointerMove {
            x: 1,
            y: 2,
            monitor: 0,
        });
        let display = ArppMessage::Display(DisplayMessage::DamageRegions {
            frame_id: 1,
            regions: vec![Rectangle {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            }],
        });
        assert_eq!(input.priority(), MessagePriority::Highest);
        assert_eq!(input.class().ssh_channel(), SshChannelId::Input);
        assert_eq!(display.priority(), MessagePriority::Medium);
        assert_eq!(display.class().ssh_channel(), SshChannelId::Media);
    }

    #[test]
    fn round_trips_cache_and_resumption_messages() {
        let session_id = Uuid::new_v4();
        let messages = [
            ArppMessage::Control(ControlMessage::ResumeSession {
                session_id,
                last_presented_frame: 91,
                cache_epoch: 7,
                cache_digest: [3; 32],
            }),
            ArppMessage::Display(DisplayMessage::CacheStore {
                frame_id: 92,
                cache_key: [5; 32],
                namespace: CacheNamespace::Glyph,
                region: Rectangle {
                    x: 1,
                    y: 2,
                    width: 8,
                    height: 12,
                },
                format: PixelFormat::Bgra8888,
                data: vec![0; 8 * 12 * 4],
            }),
        ];
        for (sequence, message) in messages.into_iter().enumerate() {
            let expected = ArppFrame::new(sequence as u64, message);
            let mut bytes = BytesMut::from(encode_frame(&expected).unwrap().as_ref());
            assert_eq!(decode_frame(&mut bytes).unwrap(), Some(expected));
        }
    }

    #[test]
    fn monitor_metadata_is_bounded_and_round_trips_without_changing_legacy_layouts() {
        let properties = MonitorProperties {
            name: "Display 1".into(),
            dpi: 144,
            width_mm: 600,
            height_mm: 340,
            refresh_millihz: 144000,
            primary: true,
        };
        assert!(valid_monitor_properties(
            1,
            std::slice::from_ref(&properties)
        ));
        assert!(valid_monitor_properties(2, &[]));
        assert!(!valid_monitor_properties(
            2,
            std::slice::from_ref(&properties)
        ));
        let mut invalid = properties.clone();
        invalid.width_mm = u32::MAX;
        assert!(!valid_monitor_properties(1, &[invalid]));
        let frame = ArppFrame::new(
            1,
            ArppMessage::Control(ControlMessage::MonitorLayoutWithProperties {
                width: 1920,
                height: 1080,
                monitors: vec![Rectangle {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                }],
                properties: vec![properties],
            }),
        );
        let mut bytes = BytesMut::from(encode_frame(&frame).unwrap().as_ref());
        assert_eq!(decode_frame(&mut bytes).unwrap(), Some(frame));
    }
    #[test]
    fn round_trips_keyboard_and_monitor_settings() {
        for control in [
            ControlMessage::KeyboardLayout {
                layout: "es".into(),
                variant: "".into(),
            },
            ControlMessage::MonitorLayout {
                width: 3840,
                height: 1080,
                monitors: vec![
                    Rectangle {
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    Rectangle {
                        x: 1920,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                ],
            },
        ] {
            let expected = ArppFrame::new(10, ArppMessage::Control(control));
            let mut bytes = BytesMut::from(encode_frame(&expected).unwrap().as_ref());
            assert_eq!(decode_frame(&mut bytes).unwrap(), Some(expected));
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn round_trips_file_bridge_negotiation() {
        let expected = ArppFrame::new(
            17,
            ArppMessage::Control(ControlMessage::FileOffer {
                transfer_id: Uuid::new_v4(),
                file_name: "design.png".into(),
                size: 4096,
                mime_types: vec!["image/png".into()],
                sha256: [0x5a; 32],
            }),
        );
        let mut bytes = BytesMut::from(encode_frame(&expected).unwrap().as_ref());
        assert_eq!(decode_frame(&mut bytes).unwrap(), Some(expected));
    }

    #[test]
    fn round_trips_a_chunked_clipboard_payload() {
        let expected = ArppFrame::new(
            18,
            ArppMessage::Clipboard(ClipboardMessage::DataChunk {
                revision: 4,
                mime_type: "image/png".into(),
                offset: 1024 * 1024,
                total_size: 2 * 1024 * 1024,
                data: vec![0x5a; 4096],
            }),
        );
        let mut bytes = BytesMut::from(encode_frame(&expected).unwrap().as_ref());
        assert_eq!(decode_frame(&mut bytes).unwrap(), Some(expected));
    }

    #[test]
    fn rejects_a_tampered_priority_header() {
        let frame = ArppFrame::new(
            1,
            ArppMessage::Input(InputMessage::PointerWheel {
                horizontal: 0,
                vertical: 120,
            }),
        );
        let mut encoded = encode_frame(&frame).unwrap().to_vec();
        encoded[4 + 4 + 2 + 1] = MessagePriority::Low as u8;
        let mut buffer = BytesMut::from(encoded.as_slice());
        assert!(matches!(
            decode_frame(&mut buffer),
            Err(ProtocolError::PriorityMismatch)
        ));
    }

    #[test]
    fn bootstrap_rejects_expired_or_empty_tokens() {
        let mut bootstrap = QuicBootstrap {
            session_id: Uuid::new_v4(),
            one_time_token: [0; 32],
            host: "desktop.example".into(),
            port: 4433,
            server_name: "desktop.example".into(),
            expires_unix_ms: 2000,
        };
        assert!(matches!(
            bootstrap.validate_at(1000),
            Err(ProtocolError::InvalidBootstrap(_))
        ));
        bootstrap.one_time_token = [7; 32];
        assert!(matches!(
            bootstrap.validate_at(2000),
            Err(ProtocolError::ExpiredBootstrap)
        ));
        assert!(bootstrap.validate_at(1999).is_ok());
    }

    #[test]
    fn bootstrap_generation_uses_a_bounded_nonzero_token() {
        let bootstrap = QuicBootstrap::generate(
            Uuid::new_v4(),
            "desktop.example".into(),
            4433,
            "desktop.example".into(),
            1_000,
            30_000,
        )
        .unwrap();
        assert!(bootstrap.one_time_token.iter().any(|byte| *byte != 0));
        assert_eq!(bootstrap.expires_unix_ms, 31_000);
    }
}
