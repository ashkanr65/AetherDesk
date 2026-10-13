//! Four-byte big-endian length prefixes followed by bincode message payloads.
//!
//! This framing applies to [`crate::messages::ArppMessage`]. It is separate from
//! the versioned [`crate::ArppFrame`] wire format.

use bincode::Options;
use thiserror::Error;

use crate::messages::ArppMessage;
use crate::MAX_PAYLOAD_LEN;

const PREFIX_LEN: usize = 4;

pub type Result<T> = std::result::Result<T, FramingError>;

#[derive(Debug, Error)]
pub enum FramingError {
    #[error("more bytes are needed to read a complete frame")]
    Incomplete,
    #[error("frame payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("frame length cannot be represented")]
    LengthOverflow,
    #[error("invalid bincode message payload: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
}

fn codec() -> impl Options {
    // Match bincode::serialize's fixed-width integers, but reject unused bytes
    // inside a declared payload and bound decoding work by the frame size.
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_PAYLOAD_LEN as u64)
        .reject_trailing_bytes()
}

/// Append a frame, preserving existing buffer contents on serialization errors.
///
/// Payloads are limited to [`MAX_PAYLOAD_LEN`], as in the existing wire codec.
pub fn write_frame(msg: &ArppMessage, buf: &mut Vec<u8>) -> Result<()> {
    let payload_len = codec().serialized_size(msg)?;
    let payload_len = usize::try_from(payload_len).map_err(|_| FramingError::LengthOverflow)?;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FramingError::PayloadTooLarge(payload_len));
    }
    let wire_len = u32::try_from(payload_len).map_err(|_| FramingError::LengthOverflow)?;
    let payload = codec().serialize(msg)?;
    buf.extend_from_slice(&wire_len.to_be_bytes());
    buf.extend_from_slice(&payload);
    Ok(())
}

/// Read the first frame and return its message and total bytes consumed.
///
/// A missing prefix or payload returns [`FramingError::Incomplete`]. A complete
/// frame with an invalid payload returns a serialization error. Bytes belonging
/// to subsequent frames are left for the caller to process.
pub fn read_frame(buf: &[u8]) -> Result<(ArppMessage, usize)> {
    let prefix: [u8; PREFIX_LEN] = buf
        .get(..PREFIX_LEN)
        .ok_or(FramingError::Incomplete)?
        .try_into()
        .map_err(|_| FramingError::Incomplete)?;
    let payload_len =
        usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| FramingError::LengthOverflow)?;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FramingError::PayloadTooLarge(payload_len));
    }
    let consumed = PREFIX_LEN
        .checked_add(payload_len)
        .ok_or(FramingError::LengthOverflow)?;
    let payload = buf
        .get(PREFIX_LEN..consumed)
        .ok_or(FramingError::Incomplete)?;
    let message = codec()
        .with_limit(payload_len as u64)
        .deserialize(payload)?;
    Ok((message, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{
        ClipboardPayload, ControlAction, ControlPayload, DamageRegion, DisplayPayload,
        EncodingType, InputData, InputPayload, InputType, MetricsPayload,
    };
    use bytes::Bytes;
    use uuid::Uuid;

    fn control() -> ArppMessage {
        ArppMessage::Control(ControlPayload {
            session_id: Uuid::from_u128(0x00112233_4455_6677_8899_aabbccddeeff),
            action: ControlAction::Start,
            capabilities: 0xa5a5_1234,
        })
    }

    fn assert_round_trip(message: ArppMessage) {
        let mut buf = Vec::new();
        write_frame(&message, &mut buf).unwrap();
        let payload = bincode::serialize(&message).unwrap();
        assert_eq!(&buf[..PREFIX_LEN], &(payload.len() as u32).to_be_bytes());
        assert_eq!(&buf[PREFIX_LEN..], payload.as_slice());
        let (actual, consumed) = read_frame(&buf).unwrap();
        assert_eq!(actual, message);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn round_trips_control() {
        assert_round_trip(control());
    }

    #[test]
    fn round_trips_input() {
        assert_round_trip(ArppMessage::Input(InputPayload {
            timestamp_us: u64::MAX,
            input_type: InputType::Mouse,
            data: InputData::PointerMove {
                x: -100,
                y: 200,
                monitor: 1,
            },
        }));
    }

    #[test]
    fn round_trips_display() {
        assert_round_trip(ArppMessage::Display(DisplayPayload {
            frame_id: 123,
            regions: vec![DamageRegion {
                x: 10,
                y: 20,
                width: 640,
                height: 480,
            }],
            encoding: EncodingType::H264,
        }));
    }

    #[test]
    fn round_trips_clipboard() {
        assert_round_trip(ArppMessage::Clipboard(ClipboardPayload {
            mime_type: "application/octet-stream".into(),
            revision: 42,
            data: Bytes::from_static(&[0, 0xff, 0x80, 13, 10]),
        }));
    }

    #[test]
    fn round_trips_metrics() {
        assert_round_trip(ArppMessage::Metrics(MetricsPayload::NetworkSample {
            interval_ms: 1_000,
            delivered_bytes: 4_000_000,
            delivered_packets: 4_000,
            lost_packets: 2,
            rtt_micros: 30_000,
            jitter_micros: 1_000,
        }));
    }

    #[test]
    fn empty_buffer_is_incomplete() {
        assert!(matches!(read_frame(&[]), Err(FramingError::Incomplete)));
    }

    #[test]
    fn every_partial_prefix_and_payload_is_incomplete() {
        let mut buf = Vec::new();
        write_frame(&control(), &mut buf).unwrap();
        for end in 1..buf.len() {
            assert!(
                matches!(read_frame(&buf[..end]), Err(FramingError::Incomplete)),
                "expected incomplete at byte {end}"
            );
        }
    }

    #[test]
    fn appends_and_reads_concatenated_frames() {
        let first = control();
        let second = ArppMessage::Metrics(MetricsPayload::Ping {
            nonce: 9,
            sent_at_us: 10,
        });
        let mut buf = Vec::new();
        write_frame(&first, &mut buf).unwrap();
        let first_bytes = buf.clone();
        write_frame(&second, &mut buf).unwrap();
        assert_eq!(&buf[..first_bytes.len()], first_bytes.as_slice());
        let (actual, consumed) = read_frame(&buf).unwrap();
        assert_eq!(actual, first);
        assert_eq!(consumed, first_bytes.len());
        let (actual, next_consumed) = read_frame(&buf[consumed..]).unwrap();
        assert_eq!(actual, second);
        assert_eq!(consumed + next_consumed, buf.len());
    }

    #[test]
    fn corrupt_variant_returns_serialization_error() {
        // The outer enum has no variant with index u32::MAX.
        let buf = [0, 0, 0, 4, 0xff, 0xff, 0xff, 0xff];
        assert!(matches!(
            read_frame(&buf),
            Err(FramingError::Serialization(_))
        ));
    }

    #[test]
    fn zero_length_payload_is_corrupt_not_incomplete() {
        assert!(matches!(
            read_frame(&[0; PREFIX_LEN]),
            Err(FramingError::Serialization(_))
        ));
    }

    #[test]
    fn complete_frame_with_truncated_message_is_corrupt() {
        let mut buf = Vec::new();
        write_frame(&control(), &mut buf).unwrap();
        buf.pop();
        let declared_len = (buf.len() - PREFIX_LEN) as u32;
        buf[..PREFIX_LEN].copy_from_slice(&declared_len.to_be_bytes());
        assert!(matches!(
            read_frame(&buf),
            Err(FramingError::Serialization(_))
        ));
    }

    #[test]
    fn rejects_trailing_bytes_inside_payload() {
        let mut buf = Vec::new();
        write_frame(&control(), &mut buf).unwrap();
        buf.push(0);
        let declared_len = (buf.len() - PREFIX_LEN) as u32;
        buf[..PREFIX_LEN].copy_from_slice(&declared_len.to_be_bytes());
        assert!(matches!(
            read_frame(&buf),
            Err(FramingError::Serialization(_))
        ));
    }

    #[test]
    fn rejects_oversized_prefix_before_waiting_for_payload() {
        let prefix = u32::MAX.to_be_bytes();
        assert!(matches!(
            read_frame(&prefix),
            Err(FramingError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn oversized_write_preserves_destination() {
        let message = ArppMessage::Clipboard(ClipboardPayload {
            mime_type: "application/octet-stream".into(),
            revision: 1,
            data: Bytes::from(vec![0; MAX_PAYLOAD_LEN]),
        });
        let mut buf = vec![1, 2, 3];
        assert!(write_frame(&message, &mut buf).is_err());
        assert_eq!(buf, [1, 2, 3]);
    }
}
