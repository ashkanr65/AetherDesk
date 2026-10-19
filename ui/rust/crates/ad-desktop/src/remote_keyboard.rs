//! Physical Windows key capture is enabled only for the focused remote surface
//! and its exact foreground HWND. The hook queues events; it does no network I/O.
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
#[derive(Clone)]
pub struct Surface {
    pub image: crate::remote_geometry::PhysicalRect,
    pub size: (u32, u32),
    pub toolbar: Option<crate::remote_geometry::PhysicalRect>,
}
impl Surface {
    fn contains(rect: crate::remote_geometry::PhysicalRect, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(rect.x)
            && i64::from(y) >= i64::from(rect.y)
            && i64::from(x) < i64::from(rect.x) + i64::from(rect.width)
            && i64::from(y) < i64::from(rect.y) + i64::from(rect.height)
    }
    fn point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            ((i64::from(x) - i64::from(self.image.x)) as f64 / self.image.width.max(1) as f64
                * self.size.0 as f64)
                .clamp(0.0, self.size.0.saturating_sub(1) as f64) as i32,
            ((i64::from(y) - i64::from(self.image.y)) as f64 / self.image.height.max(1) as f64
                * self.size.1 as f64)
                .clamp(0.0, self.size.1.saturating_sub(1) as f64) as i32,
        )
    }
}
pub fn evdev(scan: u32, extended: bool, vk: u32) -> Option<u32> {
    if vk == 0x90 {
        return Some(69);
    }
    if vk == 0x13 {
        return Some(119);
    }
    if vk == 0x2c {
        return Some(99);
    }
    if !extended {
        return if (1..=0x58).contains(&scan) {
            Some(scan)
        } else {
            match vk {
                0x7c..=0x87 => Some(183 + vk - 0x7c),
                _ => None,
            }
        };
    }
    Some(match scan {
        0x1c => 96,
        0x1d => 97,
        0x35 => 98,
        0x37 => 99,
        0x38 => 100,
        0x47 => 102,
        0x48 => 103,
        0x49 => 104,
        0x4b => 105,
        0x4d => 106,
        0x4f => 107,
        0x50 => 108,
        0x51 => 109,
        0x52 => 110,
        0x53 => 111,
        0x5b => 125,
        0x5c => 126,
        0x5d => 127,
        _ => return None,
    })
}
fn event_key(scan: u32, extended: bool, vk: u32, mapped: u32) -> Option<u32> {
    // Software/accessibility input may provide only a virtual key. The active
    // foreground input layout supplies its scan code, including E0 prefixes.
    let (scan, extended) = if scan == 0 {
        (mapped & 0xff, extended || mapped & 0xff00 == 0xe000)
    } else {
        (scan, extended)
    };
    evdev(scan, extended, vk)
}
pub fn keyboard_layout(id: u32) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x00020409 => ("us", "intl"),
        0x00010409 => ("us", "dvorak"),
        0x00000809 => ("gb", ""),
        0x0000080a => ("latam", ""),
        0x0000040a | 0x00000c0a => ("es", ""),
        _ => {
            if id > 0xffff {
                return None;
            }
            match id {
                0x0409 => ("us", ""),
                0x0809 => ("gb", ""),
                0x040a | 0x0c0a => ("es", ""),
                0x080a => ("latam", ""),
                0x0407 => ("de", ""),
                0x040c => ("fr", ""),
                0x0410 => ("it", ""),
                0x0416 => ("br", ""),
                0x0816 => ("pt", ""),
                0x0419 => ("ru", ""),
                0x0429 => ("ir", ""),
                0x0406 => ("dk", ""),
                0x041d => ("se", ""),
                0x0414 => ("no", ""),
                0x040b => ("fi", ""),
                0x0415 => ("pl", ""),
                0x041f => ("tr", ""),
                0x0405 => ("cz", ""),
                _ => return None,
            }
        }
    })
}
#[derive(Default)]
struct Held {
    keys: BTreeSet<u32>,
    locks: u16,
    last_control: Option<u32>,
    suppress_control_up: bool,
}
impl Held {
    fn exit(&self, key: u32, pressed: bool) -> bool {
        pressed
            && key == 1
            && self.keys.iter().any(|k| matches!(k, 29 | 97))
            && self.keys.iter().any(|k| matches!(k, 56 | 100))
    }
    fn event(
        &mut self,
        key: u32,
        pressed: bool,
        time: u32,
    ) -> Vec<ad_stream_protocol::InputMessage> {
        use ad_stream_protocol::InputMessage;
        let mut result = Vec::new();
        // Windows synthesizes left Control immediately before AltGr with the
        // same timestamp. Remove that synthetic modifier before the next key.
        if key == 100 && pressed && self.last_control == Some(time) && self.keys.remove(&29) {
            self.suppress_control_up = true;
            result.push(InputMessage::Keyboard {
                scan_code: 29,
                pressed: false,
                modifiers: self.locks,
                repeat: false,
            });
        }
        if key == 29 && !pressed && self.suppress_control_up {
            self.suppress_control_up = false;
            return result;
        }
        let repeat = pressed && !self.keys.insert(key);
        if !pressed {
            self.keys.remove(&key);
        }
        if key == 29 && pressed && !repeat {
            self.last_control = Some(time)
        } else if key != 100 {
            self.last_control = None;
        }
        if pressed && !repeat {
            match key {
                58 => self.locks ^= ad_stream_protocol::MODIFIER_CAPS_LOCK,
                69 => self.locks ^= ad_stream_protocol::MODIFIER_NUM_LOCK,
                _ => {}
            }
        }
        // Side-specific modifiers are real physical events. Group flags must
        // stay zero, or the host would additionally synthesize left Alt/Ctrl.
        result.push(InputMessage::Keyboard {
            scan_code: key,
            pressed,
            modifiers: self.locks,
            repeat,
        });
        result
    }
    fn release(&mut self) -> Vec<ad_stream_protocol::InputMessage> {
        let keys = std::mem::take(&mut self.keys);
        self.last_control = None;
        self.suppress_control_up = false;
        keys.into_iter()
            .map(|scan_code| ad_stream_protocol::InputMessage::Keyboard {
                scan_code,
                pressed: false,
                modifiers: self.locks,
                repeat: false,
            })
            .collect()
    }
}
pub struct NativeKeyboard {
    id: uuid::Uuid,
    exit: Arc<AtomicBool>,
}
impl NativeKeyboard {
    pub fn new() -> Self {
        #[cfg(windows)]
        windows::subscribe();
        Self {
            id: uuid::Uuid::new_v4(),
            exit: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn ready(&self) -> bool {
        #[cfg(windows)]
        {
            windows::start();
            windows::ready()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    pub fn active(&self) -> bool {
        #[cfg(windows)]
        {
            windows::active(self.id)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    pub fn activate(
        &self,
        window: isize,
        client: &Arc<crate::remote::RemoteClient>,
        surface: Surface,
        buttons: &BTreeSet<u8>,
    ) -> bool {
        #[cfg(windows)]
        {
            windows::activate(self.id, window, client, &self.exit, surface, buttons)
        }
        #[cfg(not(windows))]
        {
            let _ = (window, client, surface, buttons);
            false
        }
    }
    pub fn release(&self) {
        #[cfg(windows)]
        windows::release(self.id);
    }
    pub fn recover(&self) {
        #[cfg(windows)]
        windows::recover();
    }
    pub fn take_exit(&self) -> bool {
        self.exit.swap(false, Ordering::AcqRel)
    }
    pub fn layout(&self) -> Option<(&'static str, &'static str)> {
        #[cfg(windows)]
        {
            windows::layout()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
    pub fn unavailable(&self) -> bool {
        #[cfg(windows)]
        {
            windows::failed()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    pub fn composing(&self, window: isize) -> bool {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::Ime::*;
            let hwnd = window as windows_sys::Win32::Foundation::HWND;
            let context = unsafe { ImmGetContext(hwnd) };
            if context.is_null() {
                return false;
            }
            let open = unsafe { ImmGetOpenStatus(context) } != 0;
            unsafe {
                ImmReleaseContext(hwnd, context);
            }
            open
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            false
        }
    }
}
impl Drop for NativeKeyboard {
    fn drop(&mut self) {
        self.release();
        #[cfg(windows)]
        windows::unsubscribe();
    }
}
#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicI32, AtomicUsize},
            OnceLock, Weak,
        },
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    };
    struct Route {
        id: uuid::Uuid,
        window: isize,
        client: Weak<crate::remote::RemoteClient>,
        exit: Weak<AtomicBool>,
        held: Held,
        until: Instant,
        surface: Surface,
        buttons: BTreeSet<u8>,
        point: (i32, i32),
    }
    static ROUTE: OnceLock<parking_lot::Mutex<Option<Route>>> = OnceLock::new();
    static STATUS: AtomicI32 = AtomicI32::new(0);
    static REINSTALL: AtomicBool = AtomicBool::new(false);
    static SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);
    fn route() -> &'static parking_lot::Mutex<Option<Route>> {
        ROUTE.get_or_init(|| parking_lot::Mutex::new(None))
    }
    fn send(route: &Route, events: Vec<ad_stream_protocol::InputMessage>) -> bool {
        if let Some(client) = route.client.upgrade() {
            for event in events {
                if client
                    .send(crate::remote::Request::Input { event })
                    .is_err()
                {
                    return false;
                }
            }
            true
        } else {
            false
        }
    }
    fn clear(route: &mut Option<Route>) {
        if let Some(mut previous) = route.take() {
            let mut events = previous.held.release();
            for button in std::mem::take(&mut previous.buttons) {
                events.push(ad_stream_protocol::InputMessage::PointerButton {
                    button,
                    pressed: false,
                    x: previous.point.0,
                    y: previous.point.1,
                });
            }
            send(&previous, events);
        }
    }
    pub fn subscribe() {
        if SUBSCRIBERS.fetch_add(1, Ordering::AcqRel) == 0 && STATUS.load(Ordering::Acquire) == -1 {
            STATUS.store(0, Ordering::Release);
        }
        start();
    }
    pub fn unsubscribe() {
        SUBSCRIBERS.fetch_sub(1, Ordering::AcqRel);
    }
    pub fn ready() -> bool {
        STATUS.load(Ordering::Acquire) == 2
    }
    pub fn failed() -> bool {
        STATUS.load(Ordering::Acquire) == -1
    }
    pub fn recover() {
        // A normal key reaching the GUI proves that the route did not suppress
        // that event. Stop claiming exclusive capture until the hook owner has
        // reinstalled it. The GUI forwards the otherwise discarded key event.
        if STATUS
            .compare_exchange(2, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            REINSTALL.store(true, Ordering::Release);
            tracing::warn!("remote keyboard capture lost exclusivity; using GUI input while reinstalling the hook");
        }
    }
    pub fn start() {
        if STATUS
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if std::thread::Builder::new()
            .name("remote-keyboard".into())
            .spawn(|| unsafe {
                windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                    windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                );
                let mut hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(callback),
                    GetModuleHandleW(std::ptr::null()),
                    0,
                );
                if hook.is_null() {
                    STATUS.store(-1, Ordering::Release);
                    return;
                }
                let mouse = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_callback),
                    GetModuleHandleW(std::ptr::null()),
                    0,
                );
                if mouse.is_null() {
                    UnhookWindowsHookEx(hook);
                    STATUS.store(-1, Ordering::Release);
                    return;
                }
                let timer = SetTimer(std::ptr::null_mut(), 0, 50, None);
                if timer == 0 {
                    UnhookWindowsHookEx(mouse);
                    UnhookWindowsHookEx(hook);
                    STATUS.store(-1, Ordering::Release);
                    return;
                }
                STATUS.store(2, Ordering::Release);
                let mut message: MSG = std::mem::zeroed();
                let mut recovery_failed = false;
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    if message.message == WM_TIMER {
                        if SUBSCRIBERS.load(Ordering::Acquire) == 0 {
                            break;
                        }
                        if REINSTALL.swap(false, Ordering::AcqRel) {
                            clear(&mut route().lock());
                            UnhookWindowsHookEx(hook);
                            hook = SetWindowsHookExW(
                                WH_KEYBOARD_LL,
                                Some(callback),
                                GetModuleHandleW(std::ptr::null()),
                                0,
                            );
                            if hook.is_null() {
                                tracing::warn!("remote keyboard hook recovery failed; GUI input remains enabled");
                                recovery_failed = true;
                                break;
                            } else {
                                STATUS.store(2, Ordering::Release);
                                tracing::info!("remote keyboard hook recovered");
                            }
                        }
                        let mut active = route().lock();
                        if active.as_ref().is_some_and(|r| {
                            GetForegroundWindow() as isize != r.window || Instant::now() > r.until
                        }) {
                            clear(&mut active);
                        }
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                clear(&mut route().lock());
                KillTimer(std::ptr::null_mut(), timer);
                UnhookWindowsHookEx(mouse);
                UnhookWindowsHookEx(hook);
                REINSTALL.store(false, Ordering::Release);
                STATUS.store(if recovery_failed { -1 } else { 0 }, Ordering::Release);
            })
            .is_err()
        {
            STATUS.store(-1, Ordering::Release);
        }
    }
    unsafe extern "system" fn callback(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code != HC_ACTION as i32
            || !matches!(
                wp as u32,
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
            )
        {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        }
        let event = unsafe { &*(lp as *const KBDLLHOOKSTRUCT) };
        // This hook forwards to the remote transport and never calls SendInput.
        // Windows/accessibility-generated key events are therefore safe to route
        // here too; skipping them while suppressing the GUI fallback loses keys.
        // Never block Windows' low-level hook behind the UI thread. A missed
        // route is passed to the GUI, which can forward it and repair capture.
        let Some(mut active) = route().try_lock() else {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        };
        if active.as_ref().is_some_and(
            |r| unsafe { GetForegroundWindow() } as isize != r.window || Instant::now() > r.until,
        ) {
            clear(&mut active);
        }
        let Some(current) = active.as_mut() else {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        };
        let mapped = if event.scanCode == 0 {
            let thread =
                unsafe { GetWindowThreadProcessId(current.window as HWND, std::ptr::null_mut()) };
            unsafe { MapVirtualKeyExW(event.vkCode, MAPVK_VK_TO_VSC_EX, GetKeyboardLayout(thread)) }
        } else {
            0
        };
        let Some(key) = event_key(
            event.scanCode,
            event.flags & LLKHF_EXTENDED != 0,
            event.vkCode,
            mapped,
        ) else {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        };
        let pressed = matches!(wp as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if current.held.exit(key, pressed) {
            if let Some(exit) = current.exit.upgrade() {
                exit.store(true, Ordering::Release);
            }
            clear(&mut active);
            return 1;
        }
        let events = current.held.event(key, pressed, event.time);
        if !send(current, events) {
            clear(&mut active);
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        }
        if matches!(key, 58 | 69 | 70) {
            unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
        } else {
            1
        }
    }
    unsafe extern "system" fn mouse_callback(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        use ad_stream_protocol::InputMessage;
        let next = || unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) };
        if code != HC_ACTION as i32 {
            return next();
        }
        let event = unsafe { &*(lp as *const MSLLHOOKSTRUCT) };
        if event.flags & LLMHF_INJECTED != 0 {
            return next();
        }
        let mut active = route().lock();
        if active.as_ref().is_some_and(
            |r| unsafe { GetForegroundWindow() } as isize != r.window || Instant::now() > r.until,
        ) {
            clear(&mut active);
        }
        let Some(current) = active.as_mut() else {
            return next();
        };
        let (x, y) = (event.pt.x, event.pt.y);
        if current.buttons.is_empty() {
            if !Surface::contains(current.surface.image, x, y)
                || current
                    .surface
                    .toolbar
                    .is_some_and(|r| Surface::contains(r, x, y))
            {
                return next();
            }
            let hit = unsafe { GetAncestor(WindowFromPoint(event.pt), GA_ROOT) };
            if hit as isize != current.window {
                return next();
            }
        }
        let (x, y) = current.surface.point(x, y);
        current.point = (x, y);
        let message = match wp as u32 {
            WM_MOUSEMOVE => InputMessage::PointerMove { x, y, monitor: 0 },
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let button = match wp as u32 {
                    WM_LBUTTONDOWN | WM_LBUTTONUP => 1,
                    WM_RBUTTONDOWN | WM_RBUTTONUP => 3,
                    WM_MBUTTONDOWN | WM_MBUTTONUP => 2,
                    _ => {
                        if event.mouseData >> 16 == 1 {
                            8
                        } else {
                            9
                        }
                    }
                };
                let pressed = matches!(
                    wp as u32,
                    WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
                );
                if pressed {
                    current.buttons.insert(button);
                } else {
                    current.buttons.remove(&button);
                }
                InputMessage::PointerButton {
                    button,
                    pressed,
                    x,
                    y,
                }
            }
            WM_MOUSEWHEEL => InputMessage::PointerWheel {
                horizontal: 0,
                vertical: (event.mouseData >> 16) as i16 as i32,
            },
            WM_MOUSEHWHEEL => InputMessage::PointerWheel {
                horizontal: (event.mouseData >> 16) as i16 as i32,
                vertical: 0,
            },
            _ => return next(),
        };
        if !send(current, vec![message]) {
            clear(&mut active);
        }
        next()
    }
    pub fn activate(
        id: uuid::Uuid,
        window: isize,
        client: &Arc<crate::remote::RemoteClient>,
        exit: &Arc<AtomicBool>,
        surface: Surface,
        buttons: &BTreeSet<u8>,
    ) -> bool {
        if !ready() || crate::remote_geometry::foreground() != Some(window) {
            return false;
        }
        let mut active = route().lock();
        if active
            .as_ref()
            .is_some_and(|r| r.id == id && r.window == window)
        {
            let current = active.as_mut().unwrap();
            current.until = Instant::now() + Duration::from_millis(600);
            current.surface = surface;
            return true;
        }
        clear(&mut active);
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut cursor);
        }
        let point = surface.point(cursor.x, cursor.y);
        let mut current = Route {
            id,
            window,
            client: Arc::downgrade(client),
            exit: Arc::downgrade(exit),
            held: Held::default(),
            until: Instant::now() + Duration::from_millis(600),
            surface,
            buttons: buttons.clone(),
            point,
        };
        current.held.locks = ad_stream_protocol::MODIFIER_LOCK_STATE_VALID
            | if unsafe { GetKeyState(VK_NUMLOCK as i32) } & 1 != 0 {
                ad_stream_protocol::MODIFIER_NUM_LOCK
            } else {
                0
            }
            | if unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0 {
                ad_stream_protocol::MODIFIER_CAPS_LOCK
            } else {
                0
            };
        let right_alt = unsafe { GetAsyncKeyState(VK_RMENU as i32) } < 0;
        for (vk, key) in [
            (VK_LSHIFT, 42),
            (VK_RSHIFT, 54),
            (VK_LCONTROL, 29),
            (VK_RCONTROL, 97),
            (VK_LMENU, 56),
            (VK_RMENU, 100),
            (VK_LWIN, 125),
            (VK_RWIN, 126),
        ] {
            if key == 29 && right_alt {
                continue;
            }
            if unsafe { GetAsyncKeyState(vk as i32) } < 0 {
                let events = current.held.event(key, true, 0);
                if !send(&current, events) {
                    return false;
                }
            }
        }
        *active = Some(current);
        true
    }
    pub fn active(id: uuid::Uuid) -> bool {
        ready()
            && route().lock().as_ref().is_some_and(|r| {
                r.id == id
                    && Instant::now() <= r.until
                    && unsafe { GetForegroundWindow() } as isize == r.window
            })
    }
    pub fn release(id: uuid::Uuid) {
        let mut active = route().lock();
        if active.as_ref().is_some_and(|r| r.id == id) {
            clear(&mut active);
        }
    }
    pub fn layout() -> Option<(&'static str, &'static str)> {
        // Call on the UI thread so the input-language selection is the active
        // AetherDesk window's layout, rather than the hook helper's default.
        let mut name = [0u16; 9];
        if unsafe { GetKeyboardLayoutNameW(name.as_mut_ptr()) } == 0 {
            return None;
        }
        let text = String::from_utf16_lossy(&name[..8]);
        keyboard_layout(u32::from_str_radix(&text, 16).ok()?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtual_only_keys_and_readline_chords_keep_scans_and_modifier_edges() {
        use ad_stream_protocol::{InputMessage, MODIFIER_LOCK_STATE_VALID};
        assert_eq!(event_key(0, false, 0x41, 0x1e), Some(30));
        assert_eq!(event_key(0, false, 0x25, 0xe04b), Some(105));
        assert_eq!(event_key(0x30, false, 0x42, 0x1e), Some(48));
        assert_eq!(event_key(0, false, 0xff, 0), None);
        // Ctrl+A/E/F/B/U/K/W/Y/T/_, Alt+F/B/T, arrows, F-keys, and
        // window-manager chords travel as ordinary physical press/release edges.
        for (modifier, key) in [
            (29, 30),
            (29, 18),
            (29, 33),
            (29, 48),
            (29, 22),
            (29, 37),
            (29, 17),
            (29, 21),
            (29, 20),
            (29, 12),
            (56, 33),
            (56, 48),
            (56, 20),
            (56, 15),
            (29, 103),
            (42, 104),
        ] {
            let mut held = Held {
                locks: MODIFIER_LOCK_STATE_VALID,
                ..Default::default()
            };
            let mut events = held.event(modifier, true, 1);
            events.extend(held.event(key, true, 2));
            events.extend(held.event(key, false, 3));
            events.extend(held.event(modifier, false, 4));
            assert_eq!(events.len(), 4);
            assert!(
                matches!(events[1], InputMessage::Keyboard {scan_code, pressed:true, modifiers: MODIFIER_LOCK_STATE_VALID, ..} if scan_code == key)
            );
            assert!(held.keys.is_empty());
        }
    }
    #[test]
    fn extended_keys_and_numpad_remain_distinct() {
        assert_eq!(evdev(0x1c, false, 13), Some(28));
        assert_eq!(evdev(0x1c, true, 13), Some(96));
        assert_eq!(evdev(0x38, true, 0xa5), Some(100));
        assert_eq!(evdev(0x5b, true, 0x5b), Some(125));
        assert_eq!(evdev(0x47, false, 0x67), Some(71));
        assert_eq!(evdev(0x47, true, 0x24), Some(102));
        assert_eq!(evdev(0x45, false, 0x13), Some(119));
    }
    #[test]
    fn repeat_edges_and_exit_release_all_physical_modifiers() {
        let mut h = Held::default();
        h.event(29, true, 1);
        h.event(56, true, 2);
        assert!(h.exit(1, true));
        assert_eq!(h.release().len(), 2);
        assert!(!h.exit(1, true));
        let a = h.event(30, true, 3);
        let b = h.event(30, true, 4);
        assert!(matches!(
            a[0],
            ad_stream_protocol::InputMessage::Keyboard { repeat: false, .. }
        ));
        assert!(matches!(
            b[0],
            ad_stream_protocol::InputMessage::Keyboard { repeat: true, .. }
        ));
    }
    #[test]
    fn altgr_removes_only_the_adjacent_synthetic_control() {
        let mut h = Held::default();
        h.event(29, true, 7);
        let events = h.event(100, true, 7);
        assert!(matches!(
            events[0],
            ad_stream_protocol::InputMessage::Keyboard {
                scan_code: 29,
                pressed: false,
                ..
            }
        ));
        assert!(!h.keys.contains(&29));
        assert!(h.keys.contains(&100));
        assert!(h.event(29, false, 8).is_empty());
    }
    #[test]
    fn input_layouts_match_reference_and_unknown_ids_do_not_force_us() {
        assert_eq!(keyboard_layout(0x20409), Some(("us", "intl")));
        assert_eq!(keyboard_layout(0x0429), Some(("ir", "")));
        assert_eq!(keyboard_layout(0xdeadbeef), None);
    }
    #[test]
    fn physical_surface_maps_negative_origins_and_clamps_drag_positions() {
        use crate::remote_geometry::PhysicalRect;
        let surface = Surface {
            image: PhysicalRect {
                x: -2560,
                y: -200,
                width: 2560,
                height: 1440,
            },
            size: (1920, 1080),
            toolbar: Some(PhysicalRect {
                x: -1400,
                y: -200,
                width: 600,
                height: 60,
            }),
        };
        assert_eq!(surface.point(-1280, 520), (960, 540));
        assert_eq!(surface.point(-3000, -500), (0, 0));
        assert_eq!(surface.point(500, 2000), (1919, 1079));
        assert!(Surface::contains(surface.image, -2560, -200));
        assert!(!Surface::contains(surface.image, 0, 0));
        assert!(Surface::contains(surface.toolbar.unwrap(), -1300, -180));
    }
    #[test]
    fn genuine_control_altgr_and_sided_modifiers_keep_physical_identity() {
        use ad_stream_protocol::{InputMessage, MODIFIER_LOCK_STATE_VALID};
        let mut h = Held {
            locks: MODIFIER_LOCK_STATE_VALID,
            ..Default::default()
        };
        h.event(29, true, 7);
        assert_eq!(h.event(100, true, 8).len(), 1);
        assert!(h.keys.contains(&29));
        assert!(h.keys.contains(&100));
        for code in [54, 97, 126] {
            assert!(matches!(h.event(code, true, 9)[0], InputMessage::Keyboard {
                scan_code, modifiers: MODIFIER_LOCK_STATE_VALID, ..
            } if scan_code == code));
        }
        assert_eq!(h.release().len(), 5);
        assert!(h.keys.is_empty());
        assert_eq!(evdev(0, false, 0x2c), Some(99));
        assert_eq!(evdev(0x5d, true, 0x5d), Some(127));
    }
}
