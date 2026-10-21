//! Normalize both client input routes before sending the shared wire format.
use ad_stream_protocol::{pointer, InputMessage};
use eframe::egui;

pub fn gui_button(button: egui::PointerButton) -> u8 {
    match button {
        egui::PointerButton::Primary => pointer::LEFT,
        egui::PointerButton::Secondary => pointer::RIGHT,
        egui::PointerButton::Middle => pointer::MIDDLE,
        egui::PointerButton::Extra1 => pointer::BACK,
        egui::PointerButton::Extra2 => pointer::FORWARD,
    }
}

pub fn gui_wheel(unit: egui::MouseWheelUnit, delta: egui::Vec2) -> InputMessage {
    let scale = match unit {
        egui::MouseWheelUnit::Point => 3.0,
        egui::MouseWheelUnit::Line => 120.0,
        egui::MouseWheelUnit::Page => 360.0,
    };
    // egui/winit express horizontal content movement (positive left).
    InputMessage::PointerWheel {
        horizontal: (-delta.x * scale) as i32,
        vertical: (delta.y * scale) as i32,
    }
}

#[cfg(windows)]
pub fn windows_message(message: u32, data: u32, x: i32, y: i32) -> Option<InputMessage> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let button = match message {
        WM_MOUSEMOVE => return Some(InputMessage::PointerMove { x, y, monitor: 0 }),
        WM_MOUSEWHEEL => {
            return Some(InputMessage::PointerWheel {
                horizontal: 0,
                vertical: (data >> 16) as i16 as i32,
            })
        }
        WM_MOUSEHWHEEL => {
            return Some(InputMessage::PointerWheel {
                horizontal: (data >> 16) as i16 as i32,
                vertical: 0,
            })
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => pointer::LEFT,
        WM_RBUTTONDOWN | WM_RBUTTONUP => pointer::RIGHT,
        WM_MBUTTONDOWN | WM_MBUTTONUP => pointer::MIDDLE,
        WM_XBUTTONDOWN | WM_XBUTTONUP => match data >> 16 {
            1 => pointer::BACK,
            2 => pointer::FORWARD,
            _ => return None,
        },
        _ => return None,
    };
    let pressed = matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    Some(InputMessage::PointerButton {
        button,
        pressed,
        x,
        y,
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn native_and_gui_mouse_routes_agree_on_buttons_releases_and_wheel_directions() {
        for (gui, down, up, data, expected) in [
            (
                egui::PointerButton::Primary,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                0,
                1,
            ),
            (
                egui::PointerButton::Secondary,
                WM_RBUTTONDOWN,
                WM_RBUTTONUP,
                0,
                2,
            ),
            (
                egui::PointerButton::Middle,
                WM_MBUTTONDOWN,
                WM_MBUTTONUP,
                0,
                3,
            ),
            (
                egui::PointerButton::Extra1,
                WM_XBUTTONDOWN,
                WM_XBUTTONUP,
                1 << 16,
                4,
            ),
            (
                egui::PointerButton::Extra2,
                WM_XBUTTONDOWN,
                WM_XBUTTONUP,
                2 << 16,
                5,
            ),
        ] {
            assert_eq!(gui_button(gui), expected);
            for (event, pressed) in [(down, true), (up, false)] {
                assert_eq!(
                    windows_message(event, data, 42, 73),
                    Some(InputMessage::PointerButton {
                        button: expected,
                        pressed,
                        x: 42,
                        y: 73,
                    })
                );
            }
        }
        for delta in [-240_i32, -120, -30, 30, 120, 240] {
            let raw = u32::from(delta as i16 as u16) << 16;
            for (event, gui_delta) in [
                (WM_MOUSEWHEEL, egui::vec2(0.0, delta as f32 / 120.0)),
                (WM_MOUSEHWHEEL, egui::vec2(-delta as f32 / 120.0, 0.0)),
            ] {
                assert_eq!(
                    windows_message(event, raw, 0, 0),
                    Some(gui_wheel(egui::MouseWheelUnit::Line, gui_delta))
                );
            }
        }
        assert_eq!(
            gui_wheel(egui::MouseWheelUnit::Point, egui::vec2(-10.0, 10.0)),
            InputMessage::PointerWheel {
                horizontal: 30,
                vertical: 30
            }
        );
        assert_eq!(
            gui_wheel(egui::MouseWheelUnit::Page, egui::vec2(1.0, -1.0)),
            InputMessage::PointerWheel {
                horizontal: -360,
                vertical: -360
            }
        );
        assert_eq!(windows_message(WM_XBUTTONDOWN, 3 << 16, 0, 0), None);
    }
}
