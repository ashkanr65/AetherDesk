//! Data-only ARPP messages, separate from the existing versioned wire contract.
//!
//! These types use Serde's default external enum tags for bincode round-trips.
//! `#[serde(tag = "type")]` requires a self-describing deserializer and therefore
//! cannot be used with bincode's derived deserialization.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Reuse the established keyboard, pointer, touch, and text event definitions.
pub use crate::InputMessage as InputData;
pub use crate::MetricsMessage as MetricsPayload;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ArppMessage {
    Control(ControlPayload),
    Input(InputPayload),
    Display(DisplayPayload),
    Clipboard(ClipboardPayload),
    Metrics(MetricsPayload),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ControlPayload {
    pub session_id: Uuid,
    pub action: ControlAction,
    pub capabilities: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct InputPayload {
    pub timestamp_us: u64,
    pub input_type: InputType,
    pub data: InputData,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DisplayPayload {
    pub frame_id: u64,
    pub regions: Vec<DamageRegion>,
    pub encoding: EncodingType,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClipboardPayload {
    pub mime_type: String,
    pub revision: u64,
    pub data: Bytes,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DamageRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum EncodingType {
    Lossless,
    Jpeg,
    H264,
    Av1,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum InputType {
    Keyboard,
    Mouse,
    Touch,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ControlAction {
    Start,
    Stop,
    Resize,
    Resume,
    Capabilities,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TouchPhase;
    use bincode::Options;

    fn assert_round_trip(expected: ArppMessage) {
        let bytes = bincode::serialize(&expected).expect("serialize message");
        let actual: ArppMessage = bincode::deserialize(&bytes).expect("deserialize message");
        assert_eq!(actual, expected);

        // Also exercise the options used by the existing ARPP frame codec.
        let codec = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(crate::MAX_PAYLOAD_LEN as u64)
            .reject_trailing_bytes();
        let bytes = codec
            .serialize(&expected)
            .expect("serialize with wire options");
        let actual: ArppMessage = codec
            .deserialize(&bytes)
            .expect("deserialize with wire options");
        assert_eq!(actual, expected);
    }

    #[test]
    fn control_round_trips_every_action() {
        for action in [
            ControlAction::Start,
            ControlAction::Stop,
            ControlAction::Resize,
            ControlAction::Resume,
            ControlAction::Capabilities,
        ] {
            assert_round_trip(ArppMessage::Control(ControlPayload {
                session_id: Uuid::from_u128(0x00112233_4455_6677_8899_aabbccddeeff),
                action,
                capabilities: u32::MAX,
            }));
        }
    }

    #[test]
    fn input_round_trips_every_event() {
        let inputs = [
            (
                InputType::Keyboard,
                InputData::Keyboard {
                    scan_code: 30,
                    pressed: true,
                    modifiers: 2,
                    repeat: false,
                },
            ),
            (
                InputType::Keyboard,
                InputData::UnicodeText {
                    text: "Hello, \u{4e16}\u{754c}".into(),
                },
            ),
            (
                InputType::Mouse,
                InputData::PointerMove {
                    x: -1920,
                    y: 1080,
                    monitor: 1,
                },
            ),
            (
                InputType::Mouse,
                InputData::PointerButton {
                    button: 1,
                    pressed: false,
                    x: 20,
                    y: -30,
                },
            ),
            (
                InputType::Mouse,
                InputData::PointerWheel {
                    horizontal: -120,
                    vertical: 120,
                },
            ),
            (
                InputType::Touch,
                InputData::Touch {
                    contact_id: 7,
                    phase: TouchPhase::Begin,
                    x: 400,
                    y: 300,
                    pressure: u16::MAX,
                },
            ),
        ];
        for (input_type, data) in inputs {
            assert_round_trip(ArppMessage::Input(InputPayload {
                timestamp_us: u64::MAX,
                input_type,
                data,
            }));
        }
    }

    #[test]
    fn display_round_trips_every_encoding() {
        for encoding in [
            EncodingType::Lossless,
            EncodingType::Jpeg,
            EncodingType::H264,
            EncodingType::Av1,
        ] {
            assert_round_trip(ArppMessage::Display(DisplayPayload {
                frame_id: u64::MAX,
                regions: vec![
                    DamageRegion {
                        x: 10,
                        y: 20,
                        width: 640,
                        height: 480,
                    },
                    DamageRegion {
                        x: 650,
                        y: 500,
                        width: 100,
                        height: 200,
                    },
                ],
                encoding,
            }));
        }
        assert_round_trip(ArppMessage::Display(DisplayPayload {
            frame_id: 0,
            regions: vec![],
            encoding: EncodingType::Lossless,
        }));
    }

    #[test]
    fn clipboard_round_trips_binary_and_empty_data() {
        for data in [Bytes::from_static(&[0, 0xff, 0x80, 13, 10]), Bytes::new()] {
            assert_round_trip(ArppMessage::Clipboard(ClipboardPayload {
                mime_type: "application/octet-stream".into(),
                revision: u64::MAX,
                data,
            }));
        }
    }

    #[test]
    fn metrics_round_trips_every_sample() {
        for payload in [
            MetricsPayload::Ping {
                nonce: 42,
                sent_at_us: 1_000,
            },
            MetricsPayload::Pong {
                nonce: 42,
                sent_at_us: 1_000,
                received_at_us: 2_000,
            },
            MetricsPayload::NetworkSample {
                interval_ms: 1_000,
                delivered_bytes: 4_000_000,
                delivered_packets: 4_000,
                lost_packets: 2,
                rtt_micros: 30_000,
                jitter_micros: 1_000,
            },
            MetricsPayload::DecoderLoad {
                queue_depth: 2,
                decode_time_us: 5_000,
                dropped_frames: 3,
            },
        ] {
            assert_round_trip(ArppMessage::Metrics(payload));
        }
    }

    #[test]
    fn rejects_truncated_clipboard_data() {
        let message = ArppMessage::Clipboard(ClipboardPayload {
            mime_type: "text/plain".into(),
            revision: 1,
            data: Bytes::from_static(b"hello"),
        });
        let mut bytes = bincode::serialize(&message).unwrap();
        bytes.pop();
        assert!(bincode::deserialize::<ArppMessage>(&bytes).is_err());
    }
}
