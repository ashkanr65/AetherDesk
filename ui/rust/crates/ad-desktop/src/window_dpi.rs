//! Coalesce native DPI changes during a Windows move/size transaction.
//! Winit otherwise resizes and nudges the window at each monitor boundary,
//! before the frame-level geometry settling policy can run.
#[derive(Default)]
struct Transition {
    moving: bool,
    pending: Option<usize>,
}
impl Transition {
    fn begin(&mut self) {
        self.moving = true;
    }
    fn defer(&mut self, dpi: usize) -> bool {
        if self.moving {
            self.pending = Some(dpi);
            true
        } else {
            self.pending = None;
            false
        }
    }
    fn end(&mut self) -> bool {
        self.moving = false;
        self.pending.is_some()
    }
    fn take(&mut self) -> Option<usize> {
        if self.moving {
            None
        } else {
            self.pending.take()
        }
    }
}

#[cfg(windows)]
mod native {
    use super::Transition;
    use std::{cell::RefCell, collections::BTreeMap};
    use windows_sys::Win32::{
        Foundation::*,
        System::Threading::GetCurrentThreadId,
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    };
    const APPLY: u32 = WM_APP + 0x3d1;
    struct WindowState {
        transition: Transition,
        previous: WNDPROC,
    }
    thread_local! {
        static WINDOWS: RefCell<BTreeMap<isize, WindowState>> = RefCell::new(BTreeMap::new());
    }
    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let key = hwnd as isize;
        // Copy the previous procedure before forwarding. Never retain a RefCell
        // borrow across a native call, which can synchronously reenter us.
        let previous =
            WINDOWS.with(|windows| windows.borrow().get(&key).and_then(|state| state.previous));
        match message {
            WM_ENTERSIZEMOVE => WINDOWS.with(|windows| {
                if let Some(state) = windows.borrow_mut().get_mut(&key) {
                    state.transition.begin();
                }
            }),
            WM_DPICHANGED => {
                let deferred = WINDOWS.with(|windows| {
                    windows
                        .borrow_mut()
                        .get_mut(&key)
                        .is_some_and(|state| state.transition.defer(wparam))
                });
                if deferred {
                    return 0;
                }
            }
            WM_EXITSIZEMOVE => {
                let pending = WINDOWS.with(|windows| {
                    windows
                        .borrow_mut()
                        .get_mut(&key)
                        .is_some_and(|state| state.transition.end())
                });
                // Let winit leave its native drag first. Never resize from a
                // paint callback, or replay a pointer borrowed from Windows.
                let result = unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) };
                if pending {
                    unsafe {
                        PostMessageW(hwnd, APPLY, 0, 0);
                    }
                }
                return result;
            }
            APPLY => {
                let pending = WINDOWS.with(|windows| {
                    windows
                        .borrow_mut()
                        .get_mut(&key)
                        .and_then(|state| state.transition.take())
                });
                if let Some(dpi) = pending {
                    let current = unsafe { GetDpiForWindow(hwnd) };
                    let dpi = if current > 0 {
                        current as usize | ((current as usize) << 16)
                    } else {
                        dpi
                    };
                    let mut bounds: RECT = unsafe { std::mem::zeroed() };
                    if unsafe { GetWindowRect(hwnd, &mut bounds) } != 0 {
                        // Use the final drop position, not the stale rectangle
                        // suggested when the cursor first crossed the boundary.
                        return unsafe {
                            CallWindowProcW(
                                previous,
                                hwnd,
                                WM_DPICHANGED,
                                dpi,
                                (&bounds as *const RECT) as LPARAM,
                            )
                        };
                    }
                }
                return 0;
            }
            WM_NCDESTROY => {
                WINDOWS.with(|windows| {
                    windows.borrow_mut().remove(&key);
                });
                unsafe {
                    if GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == window_proc as *const () as isize {
                        SetWindowLongPtrW(
                            hwnd,
                            GWLP_WNDPROC,
                            previous.map_or(0, |proc| proc as *const () as isize),
                        );
                    }
                }
            }
            _ => {}
        }
        unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
    }
    unsafe extern "system" fn install_window(hwnd: HWND, _: LPARAM) -> BOOL {
        // EnumThreadWindows restricts this to our own GUI thread. Menu windows
        // have no caption and retain their explicit owner-relative placement.
        if unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32 & WS_CAPTION != WS_CAPTION {
            return 1;
        }
        if WINDOWS.with(|windows| windows.borrow().contains_key(&(hwnd as isize))) {
            return 1;
        }
        // Use the standard User32 procedure chain; it needs no optional
        // Common Controls manifest or DLL export on the user's installation.
        unsafe {
            SetLastError(0);
        }
        let previous =
            unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, window_proc as *const () as isize) };
        if previous != 0 || unsafe { GetLastError() } == 0 {
            let previous: WNDPROC = unsafe { std::mem::transmute(previous) };
            WINDOWS.with(|windows| {
                windows.borrow_mut().insert(
                    hwnd as isize,
                    WindowState {
                        transition: Transition::default(),
                        previous,
                    },
                );
            });
        }
        1
    }
    pub(super) fn install() {
        unsafe {
            EnumThreadWindows(GetCurrentThreadId(), Some(install_window), 0);
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;
        thread_local! { static DELIVERED: Cell<u32> = const { Cell::new(0) }; }
        unsafe extern "system" fn fixture(
            hwnd: HWND,
            message: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            if message == WM_DPICHANGED {
                DELIVERED.with(|count| count.set(count.get() + 1));
                return 0;
            }
            unsafe { DefWindowProcW(hwnd, message, w, l) }
        }
        #[test]
        fn native_window_defers_boundary_changes_and_releases_its_subclass_on_close() {
            let name: Vec<u16> = format!("AetherDesk DPI fixture {}", uuid::Uuid::new_v4())
                .encode_utf16()
                .chain([0])
                .collect();
            let instance = unsafe {
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null())
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(fixture),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..unsafe { std::mem::zeroed() }
            };
            assert_ne!(unsafe { RegisterClassW(&class) }, 0);
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    name.as_ptr(),
                    name.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    100,
                    100,
                    800,
                    600,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            install();
            DELIVERED.with(|count| count.set(0));
            let bounds = RECT {
                left: 100,
                top: 100,
                right: 900,
                bottom: 700,
            };
            unsafe {
                SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            }
            for dpi in [192usize, 96, 192, 144] {
                unsafe {
                    SendMessageW(
                        hwnd,
                        WM_DPICHANGED,
                        dpi | (dpi << 16),
                        (&bounds as *const RECT) as LPARAM,
                    );
                }
            }
            assert_eq!(
                DELIVERED.with(Cell::get),
                0,
                "framework resized while straddling monitors"
            );
            unsafe {
                SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
            }
            let mut message: MSG = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe { PeekMessageW(&mut message, hwnd, APPLY, APPLY, PM_REMOVE) },
                0
            );
            unsafe {
                DispatchMessageW(&message);
            }
            assert_eq!(DELIVERED.with(Cell::get), 1);
            unsafe {
                DestroyWindow(hwnd);
            }
            assert!(WINDOWS.with(|windows| !windows.borrow().contains_key(&(hwnd as isize))));
            unsafe {
                UnregisterClassW(name.as_ptr(), instance);
            }
        }
    }
}
pub(super) fn install() {
    #[cfg(windows)]
    native::install();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_dpi_changes_coalesce_until_the_native_drag_ends() {
        let mut state = Transition::default();
        assert!(!state.defer(96));
        state.begin();
        for dpi in [192, 96, 192, 96, 144] {
            assert!(state.defer(dpi));
            assert!(state.take().is_none());
        }
        assert!(state.end());
        assert_eq!(state.take(), Some(144));
        assert!(state.take().is_none());
        assert!(
            !state.defer(192),
            "non-drag DPI changes must retain normal native handling"
        );
    }
    #[test]
    fn a_new_drag_preserves_a_pending_transition_without_replaying_it_mid_move() {
        let mut state = Transition::default();
        state.begin();
        state.defer(192);
        assert!(state.end());
        state.begin();
        assert!(state.take().is_none());
        state.defer(96);
        assert!(state.end());
        assert_eq!(state.take(), Some(96));
    }
}
