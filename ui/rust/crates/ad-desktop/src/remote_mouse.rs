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

#[derive(Default)]
pub struct Wheel {
    horizontal: f64,
    vertical: f64,
}
impl Wheel {
    pub fn convert(
        &mut self,
        unit: egui::MouseWheelUnit,
        delta: egui::Vec2,
    ) -> Option<InputMessage> {
        if !delta.is_finite() {
            return None;
        }
        let scale = match unit {
            egui::MouseWheelUnit::Point => 3.0,
            egui::MouseWheelUnit::Line => 120.0,
            egui::MouseWheelUnit::Page => 360.0,
        };
        // Keep sub-unit precision across touchpad/high-resolution wheel events.
        // egui/winit express horizontal content movement (positive left).
        self.horizontal -= f64::from(delta.x) * scale;
        self.vertical += f64::from(delta.y) * scale;
        let horizontal = self.horizontal.trunc() as i32;
        let vertical = self.vertical.trunc() as i32;
        self.horizontal = self.horizontal.fract();
        self.vertical = self.vertical.fract();
        (horizontal != 0 || vertical != 0).then_some(InputMessage::PointerWheel {
            horizontal,
            vertical,
        })
    }
}

/// An explicit scroll activates the image under the pointer, like a click.
/// It must never capture a background viewport, overlay or letterbox margin.
pub fn scroll_interaction(
    ui: &egui::Ui,
    response: &egui::Response,
    image: egui::Rect,
    blocked: bool,
) -> bool {
    !blocked && response.hovered() && ui.input(|i| {
        i.focused && i.pointer.hover_pos().is_some_and(|p| image.contains(p))
            && i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { delta, .. } if delta.is_finite() && *delta != egui::Vec2::ZERO))
    })
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
                    Wheel::default().convert(egui::MouseWheelUnit::Line, gui_delta)
                );
            }
        }
        assert_eq!(
            Wheel::default().convert(egui::MouseWheelUnit::Point, egui::vec2(-10.0, 10.0)),
            Some(InputMessage::PointerWheel {
                horizontal: 30,
                vertical: 30
            })
        );
        assert_eq!(
            Wheel::default().convert(egui::MouseWheelUnit::Page, egui::vec2(1.0, -1.0)),
            Some(InputMessage::PointerWheel {
                horizontal: -360,
                vertical: -360
            })
        );
        assert_eq!(windows_message(WM_XBUTTONDOWN, 3 << 16, 0, 0), None);
    }
}

#[cfg(test)]
mod precision_tests {
    use super::*;
    #[test]
    fn fractional_scrolls_preserve_distance_direction_and_view_isolation() {
        let mut wheel = Wheel::default();
        let (mut horizontal, mut vertical) = (0, 0);
        for _ in 0..64 {
            if let Some(InputMessage::PointerWheel {
                horizontal: x,
                vertical: y,
            }) = wheel.convert(egui::MouseWheelUnit::Point, egui::vec2(0.0625, 0.125))
            {
                horizontal += x;
                vertical += y;
            }
        }
        assert_eq!((horizontal, vertical), (-12, 24));
        assert_eq!(
            wheel.convert(egui::MouseWheelUnit::Line, egui::Vec2::ZERO),
            None
        );
        assert_eq!(
            wheel.convert(egui::MouseWheelUnit::Line, egui::vec2(f32::NAN, 1.0)),
            None
        );
        assert_eq!(
            wheel.convert(egui::MouseWheelUnit::Point, egui::vec2(0.125, 0.125)),
            None
        );
        let mut other = Wheel::default();
        assert_eq!(
            other.convert(egui::MouseWheelUnit::Point, egui::vec2(0.25, 0.25)),
            None
        );
        assert_eq!(
            wheel.convert(egui::MouseWheelUnit::Point, egui::vec2(-0.125, -0.125)),
            None
        );
        assert_eq!(
            wheel.convert(egui::MouseWheelUnit::Line, egui::vec2(-1.0, -1.0)),
            Some(InputMessage::PointerWheel {
                horizontal: 120,
                vertical: -120
            })
        );
    }
}
