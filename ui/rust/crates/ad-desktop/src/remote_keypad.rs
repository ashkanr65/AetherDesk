//! Preserve physical keypad and clipboard-key identity on the GUI fallback path. Egui folds
//! keypad operators into ordinary punctuation (and has no Multiply key).
//! Only bounded in-memory metadata for the armed remote viewport is retained.
use eframe::egui;
#[derive(Clone, Debug)]
pub(crate) struct KeypadEvent {
    pub scan_code: u32,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: u16,
    alias: Option<egui::Key>,
}
pub(crate) fn evdev(vk: u32, scan: u32, extended: bool) -> Option<u32> {
    Some(match vk {
        0x60 => 82,
        0x61 => 79,
        0x62 => 80,
        0x63 => 81,
        0x64 => 75,
        0x65 => 76,
        0x66 => 77,
        0x67 => 71,
        0x68 => 72,
        0x69 => 73,
        0x6a => 55,
        0x6b => 78,
        0x6c => 121,
        0x6d => 74,
        0x6e => 83,
        0x6f => 98,
        0x0d if extended => 96,
        0x0c | 0x21..=0x28 | 0x2d | 0x2e if !extended && (0x47..=0x53).contains(&scan) => scan,
        _ => return None,
    })
}
fn alias(vk: u32) -> Option<egui::Key> {
    use egui::Key::*;
    Some(match vk {
        0x43 => C,
        0x56 => V,
        0x58 => X,
        0x60 => Num0,
        0x61 => Num1,
        0x62 => Num2,
        0x63 => Num3,
        0x64 => Num4,
        0x65 => Num5,
        0x66 => Num6,
        0x67 => Num7,
        0x68 => Num8,
        0x69 => Num9,
        0x6b => Plus,
        0x6c => Comma,
        0x6d => Minus,
        0x6e => Period,
        0x6f => Slash,
        0x0d => Enter,
        0x21 => PageUp,
        0x22 => PageDown,
        0x23 => End,
        0x24 => Home,
        0x25 => ArrowLeft,
        0x26 => ArrowUp,
        0x27 => ArrowRight,
        0x28 => ArrowDown,
        0x2d => Insert,
        0x2e => Delete,
        _ => return None,
    })
}
fn fallback_evdev(vk: u32, scan: u32, extended: bool) -> Option<u32> {
    // Keep physical identity even for keys egui cannot represent (ISO <>,
    // AltGr), and preserve modifier/key ordering during native-hook recovery.
    // Ordinary modifiers still use the existing reconciled snapshot path.
    crate::remote_keyboard::evdev(scan, extended, vk)
        .filter(|code| !matches!(code, 29 | 42 | 54 | 56 | 58 | 69 | 70 | 97 | 125 | 126))
}
fn matching(events: &[KeypadEvent], event: &egui::Event) -> Option<usize> {
    events.iter().position(|e| match event {
        egui::Event::Key {
            key,
            physical_key,
            pressed,
            ..
        } => {
            e.pressed == *pressed
                && e.alias
                    .is_some_and(|alias| alias == *key || Some(alias) == *physical_key)
                || (e.pressed == *pressed
                    && crate::remote_view::scan_code(physical_key.unwrap_or(*key))
                        == Some(e.scan_code))
        }
        egui::Event::Text(text) => {
            e.pressed
                && ((e.scan_code == 55 && text == "*")
                    || (e.scan_code == 86 && matches!(text.as_str(), "<" | ">" | "|")))
        }
        egui::Event::Copy => e.pressed && matches!(e.alias, Some(egui::Key::C | egui::Key::Insert)),
        egui::Event::Cut => e.pressed && matches!(e.alias, Some(egui::Key::X | egui::Key::Delete)),
        egui::Event::Paste(_) => {
            e.pressed && matches!(e.alias, Some(egui::Key::V | egui::Key::Insert))
        }
        _ => false,
    })
}
#[cfg(test)]
pub(crate) fn take_gui(events: &mut Vec<KeypadEvent>, event: &egui::Event) -> Option<KeypadEvent> {
    let index = matching(events, event)?;
    Some(events.remove(index))
}
pub(crate) fn take_gui_ordered(
    events: &mut Vec<KeypadEvent>,
    event: &egui::Event,
) -> Vec<KeypadEvent> {
    matching(events, event)
        .map(|index| events.drain(..=index).collect())
        .unwrap_or_default()
}
#[derive(Default)]
pub(crate) struct Batch {
    pub events: Vec<KeypadEvent>,
    pub overflowed: bool,
}
pub(crate) fn install() {
    #[cfg(windows)]
    windows::install();
}
pub(crate) fn take(owner: egui::ViewportId, window: isize) -> Batch {
    #[cfg(windows)]
    {
        windows::take(owner, window)
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, window);
        Batch::default()
    }
}
pub(crate) fn release(owner: egui::ViewportId) {
    #[cfg(windows)]
    windows::release(owner);
    #[cfg(not(windows))]
    let _ = owner;
}
#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        cell::RefCell,
        collections::VecDeque,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::Threading::GetCurrentThreadId,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    };
    #[derive(Default)]
    struct State {
        hook: isize,
        owner: Option<(egui::ViewportId, isize)>,
        events: VecDeque<(Instant, KeypadEvent)>,
        overflowed: bool,
    }
    impl Drop for State {
        fn drop(&mut self) {
            if self.hook != 0 {
                unsafe {
                    UnhookWindowsHookEx(self.hook as HHOOK);
                }
            }
        }
    }
    thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
    pub(super) fn install() {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.hook == 0 {
                s.hook = unsafe {
                    SetWindowsHookExW(
                        WH_GETMESSAGE,
                        Some(message),
                        std::ptr::null_mut(),
                        GetCurrentThreadId(),
                    )
                } as isize;
                if s.hook == 0 {
                    tracing::warn!("Physical keypad GUI fallback could not be installed");
                }
            }
        });
    }
    unsafe extern "system" fn message(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code >= 0 && wp as u32 == PM_REMOVE && lp != 0 {
            let message = &*(lp as *const MSG);
            if matches!(
                message.message,
                WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
            ) {
                let vk = message.wParam as u32;
                let scan = ((message.lParam as usize >> 16) & 0xff) as u32;
                let extended = message.lParam as usize & (1 << 24) != 0;
                if let Some(scan_code) = fallback_evdev(vk, scan, extended) {
                    STATE.with(|state| {
                        if let Ok(mut state) = state.try_borrow_mut() {
                            if state
                                .owner
                                .is_some_and(|(_, hwnd)| hwnd == message.hwnd as isize)
                                && GetForegroundWindow() == message.hwnd
                            {
                                use ad_stream_protocol::*;
                                let down = |vk| GetKeyState(vk) & (0x8000u16 as i16) != 0;
                                let altgr = down(VK_RMENU as i32);
                                let modifiers = MODIFIER_LOCK_STATE_VALID
                                    | if GetKeyState(VK_NUMLOCK as i32) & 1 != 0 {
                                        MODIFIER_NUM_LOCK
                                    } else {
                                        0
                                    }
                                    | if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
                                        MODIFIER_CAPS_LOCK
                                    } else {
                                        0
                                    }
                                    | u16::from(down(VK_SHIFT as i32))
                                    | (u16::from(
                                        down(VK_RCONTROL as i32)
                                            || (down(VK_LCONTROL as i32) && !altgr),
                                    ) << 1)
                                    | (u16::from(down(VK_LMENU as i32)) << 2)
                                    | (u16::from(down(VK_LWIN as i32) || down(VK_RWIN as i32))
                                        << 3);
                                if state.events.len() >= 128 {
                                    state.events.pop_front();
                                    state.overflowed = true;
                                }
                                state.events.push_back((
                                    Instant::now(),
                                    KeypadEvent {
                                        scan_code,
                                        pressed: matches!(
                                            message.message,
                                            WM_KEYDOWN | WM_SYSKEYDOWN
                                        ),
                                        repeat: message.lParam as usize & (1 << 30) != 0
                                            && matches!(
                                                message.message,
                                                WM_KEYDOWN | WM_SYSKEYDOWN
                                            ),
                                        modifiers,
                                        alias: alias(vk),
                                    },
                                ));
                            }
                        }
                    });
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
    }
    pub(super) fn take(owner: egui::ViewportId, window: isize) -> Batch {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.owner != Some((owner, window)) {
                s.events.clear();
                s.overflowed = false;
                s.owner = Some((owner, window));
            }
            let mut output = Batch {
                events: Vec::new(),
                overflowed: std::mem::take(&mut s.overflowed),
            };
            for (time, event) in s.events.drain(..) {
                if time.elapsed() <= Duration::from_secs(2) {
                    output.events.push(event);
                } else {
                    output.overflowed = true;
                }
            }
            output
        })
    }
    pub(super) fn release(owner: egui::ViewportId) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.owner.is_some_and(|(id, _)| id == owner) {
                s.owner = None;
                s.events.clear();
                s.overflowed = false;
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn altgr_and_iso_keys_keep_order_and_do_not_become_left_alt_shortcuts() {
        let event = |scan_code, pressed| KeypadEvent {
            scan_code,
            pressed,
            repeat: false,
            modifiers: 0,
            alias: None,
        };
        let mut pending = vec![
            event(100, true),
            event(2, true),
            event(2, false),
            event(100, false),
        ];
        let press = egui::Event::Key {
            key: egui::Key::Num1,
            physical_key: Some(egui::Key::Num1),
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let ordered = take_gui_ordered(&mut pending, &press);
        assert_eq!(
            ordered
                .iter()
                .map(|e| (e.scan_code, e.pressed))
                .collect::<Vec<_>>(),
            [(100, true), (2, true)]
        );
        assert_eq!(pending.len(), 2);
        let mut iso = vec![event(86, true), event(86, false)];
        assert_eq!(
            take_gui_ordered(&mut iso, &egui::Event::Text("<".into()))[0].scan_code,
            86
        );
        assert_eq!(iso.len(), 1);
    }
    #[test]
    fn clipboard_keys_survive_semantic_conversion_without_duplicate_edges_or_lost_modifiers() {
        for (vk, scan, event) in [
            (0x43, 46, egui::Event::Copy),
            (0x56, 47, egui::Event::Paste("clipboard payload".into())),
            (0x58, 45, egui::Event::Cut),
            (0x2d, 110, egui::Event::Copy),
            (0x2d, 110, egui::Event::Paste(String::new())),
            (0x2e, 111, egui::Event::Cut),
        ] {
            let extended = scan >= 110;
            let physical = if scan == 110 {
                0x52
            } else if scan == 111 {
                0x53
            } else {
                scan
            };
            assert_eq!(fallback_evdev(vk, physical, extended), Some(scan));
            let mut pending = vec![
                KeypadEvent {
                    scan_code: scan,
                    pressed: true,
                    repeat: false,
                    modifiers: 3,
                    alias: alias(vk),
                },
                KeypadEvent {
                    scan_code: scan,
                    pressed: false,
                    repeat: false,
                    modifiers: 0,
                    alias: alias(vk),
                },
            ];
            let down = take_gui(&mut pending, &event).unwrap();
            assert_eq!(
                (down.scan_code, down.pressed, down.modifiers),
                (scan, true, 3)
            );
            assert!(take_gui(&mut pending, &event).is_none());
            let key = alias(vk).unwrap();
            let up = take_gui(
                &mut pending,
                &egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: false,
                    repeat: false,
                    modifiers: Default::default(),
                },
            )
            .unwrap();
            assert!(!up.pressed);
            assert!(pending.is_empty());
        }
        // A host using the same non-US layout needs the original scan, not
        // the US physical position of the shortcut's logical C.
        assert_eq!(fallback_evdev(0x43, 23, false), Some(23));
        assert_eq!(fallback_evdev(0x41, 30, false), Some(30));
        assert_eq!(fallback_evdev(0xe2, 0x56, false), Some(86));
        assert_eq!(fallback_evdev(0xa5, 0x38, true), Some(100));
        assert_eq!(fallback_evdev(0x10, 0x2a, false), None);
    }
    #[test]
    fn keypad_operators_keep_physical_identity_and_gui_aliases_are_consumed_once() {
        for (vk, scan, code, key) in [
            (0x6a, 0x37, 55, None),
            (0x6b, 0x4e, 78, Some(egui::Key::Plus)),
            (0x6d, 0x4a, 74, Some(egui::Key::Minus)),
            (0x6e, 0x53, 83, Some(egui::Key::Period)),
            (0x6f, 0x35, 98, Some(egui::Key::Slash)),
        ] {
            assert_eq!(evdev(vk, scan, false), Some(code));
            assert_eq!(evdev(vk, 0, false), Some(code));
            assert_eq!(alias(vk), key);
            if let Some(key) = key {
                let event = egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                };
                let mut pending = vec![KeypadEvent {
                    scan_code: code,
                    pressed: true,
                    repeat: false,
                    modifiers: 0,
                    alias: Some(key),
                }];
                assert!(take_gui(&mut pending, &event).is_some());
                assert!(!take_gui(&mut pending, &event).is_some());
            }
        }
        assert_eq!(evdev(0x24, 0x47, false), Some(71));
        assert_eq!(evdev(0x24, 0x47, true), None);
        assert_eq!(evdev(0x0d, 0x1c, true), Some(96));
        assert_eq!(evdev(0x0d, 0x1c, false), None);
    }
}
