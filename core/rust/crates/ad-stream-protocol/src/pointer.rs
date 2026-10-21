//! Shared pointer contract. Button IDs follow the existing evdev backend:
//! 1 left, 2 right, 3 middle, 4 side/back, 5 extra/forward.
//! Wheel units are 120 per detent: positive horizontal is right, positive
//! vertical is up. Wayland's vertical axis needs the opposite sign.
pub const LEFT: u8 = 1;
pub const RIGHT: u8 = 2;
pub const MIDDLE: u8 = 3;
pub const BACK: u8 = 4;
pub const FORWARD: u8 = 5;

pub fn evdev_button(button: u8) -> Option<u16> {
    Some(match button {
        LEFT => 0x110,
        RIGHT => 0x111,
        MIDDLE => 0x112,
        BACK | 8 => 0x113, // Legacy client side-button IDs.
        FORWARD | 9 => 0x114,
        _ => return None,
    })
}

pub fn wayland_wheel(horizontal: i32, vertical: i32) -> (i32, i32) {
    (horizontal, vertical.saturating_neg())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, ArppFrame, ArppMessage, InputMessage};
    use bytes::BytesMut;

    #[test]
    fn pointer_contract_preserves_all_buttons_and_wheel_signs_on_the_wire() {
        for (button, code) in [
            (LEFT, 0x110),
            (RIGHT, 0x111),
            (MIDDLE, 0x112),
            (BACK, 0x113),
            (FORWARD, 0x114),
        ] {
            assert_eq!(evdev_button(button), Some(code));
            for pressed in [true, false] {
                let frame = ArppFrame::new(
                    1,
                    ArppMessage::Input(InputMessage::PointerButton {
                        button,
                        pressed,
                        x: 42,
                        y: 73,
                    }),
                );
                let mut bytes = BytesMut::from(encode_frame(&frame).unwrap().as_ref());
                assert_eq!(decode_frame(&mut bytes).unwrap(), Some(frame));
            }
        }
        assert_eq!(evdev_button(8), evdev_button(BACK));
        assert_eq!(evdev_button(9), evdev_button(FORWARD));
        assert_eq!(evdev_button(0), None);
        assert_eq!(evdev_button(255), None);
        for (horizontal, vertical) in [(0, 120), (0, -120), (120, 0), (-120, 0), (30, -60)] {
            let frame = ArppFrame::new(
                2,
                ArppMessage::Input(InputMessage::PointerWheel {
                    horizontal,
                    vertical,
                }),
            );
            let mut bytes = BytesMut::from(encode_frame(&frame).unwrap().as_ref());
            assert_eq!(decode_frame(&mut bytes).unwrap(), Some(frame));
            assert_eq!(wayland_wheel(horizontal, vertical), (horizontal, -vertical));
        }
        assert_eq!(wayland_wheel(i32::MIN, i32::MIN), (i32::MIN, i32::MAX));
    }
}
