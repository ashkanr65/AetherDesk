//! Coalesce native DPI changes during a Windows move/size transaction.
//! Winit otherwise resizes and nudges the window at each monitor boundary,
//! before the frame-level geometry settling policy can run.
#[derive(Default)]
struct Transition {
    moving: bool,
    pending: Option<usize>,
}
// Physical screen coordinates only: do not mix a removed monitor's old DPI
// with the new viewport scale. Keep a draggable title strip on a real monitor.
fn recover_bounds(bounds: [i32; 4], work: &[[i32; 4]], title_height: i32) -> Option<[i32; 4]> {
    let [left, top, right, bottom] = bounds;
    if right <= left || bottom <= top || work.is_empty() {
        return None;
    }
    let title_bottom = (top + title_height.max(16)).min(bottom);
    if work.iter().any(|area| {
        (right.min(area[2]) - left.max(area[0])) >= 80.min(right - left)
            && (title_bottom.min(area[3]) - top.max(area[1])) >= 16.min(title_bottom - top)
    }) {
        return None;
    }
    let cx = (i64::from(left) + i64::from(right)) / 2;
    let cy = (i64::from(top) + i64::from(bottom)) / 2;
    let area = work
        .iter()
        .filter(|a| a[2] > a[0] && a[3] > a[1])
        .min_by_key(|a| {
            let dx = cx.clamp(i64::from(a[0]), i64::from(a[2])) - cx;
            let dy = cy.clamp(i64::from(a[1]), i64::from(a[3])) - cy;
            dx * dx + dy * dy
        })?;
    let width = (right - left).min(area[2] - area[0]);
    let height = (bottom - top).min(area[3] - area[1]);
    let x = left.clamp(area[0], area[2] - width);
    let y = top.clamp(area[1], area[3] - height);
    Some([x, y, x + width, y + height])
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

fn topology_bounds(bounds: [i32; 4], work: &[[i32; 4]], title_height: i32) -> Option<[i32; 4]> {
    let valid: Vec<_> = work
        .iter()
        .filter(|a| a[2] > a[0] && a[3] > a[1])
        .copied()
        .collect();
    if valid.is_empty() {
        return None;
    }
    let extent = [
        valid.iter().map(|a| a[0]).min()?,
        valid.iter().map(|a| a[1]).min()?,
        valid.iter().map(|a| a[2]).max()?,
        valid.iter().map(|a| a[3]).max()?,
    ];
    if bounds[0] >= extent[0]
        && bounds[1] >= extent[1]
        && bounds[2] <= extent[2]
        && bounds[3] <= extent[3]
    {
        return recover_bounds(bounds, &valid, title_height);
    }
    if let Some(bounds) = recover_bounds(bounds, &valid, title_height) {
        return Some(bounds);
    }
    // A reachable title on a removed high-resolution display can still leave
    // the bottom controls outside every remaining monitor. Fit only when the
    // topology changes, never during an ordinary user move/resize.
    let closest = valid.iter().min_by_key(|a| {
        let x = i64::from(bounds[0]).clamp(i64::from(a[0]), i64::from(a[2])) - i64::from(bounds[0]);
        let y = i64::from(bounds[1]).clamp(i64::from(a[1]), i64::from(a[3])) - i64::from(bounds[1]);
        x * x + y * y
    })?;
    let width = (bounds[2] - bounds[0]).min(closest[2] - closest[0]);
    let height = (bounds[3] - bounds[1]).min(closest[3] - closest[1]);
    let x = bounds[0].clamp(closest[0], closest[2] - width);
    let y = bounds[1].clamp(closest[1], closest[3] - height);
    Some([x, y, x + width, y + height])
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
    const RECOVER: u32 = WM_APP + 0x3d2;
    struct WindowState {
        transition: Transition,
        previous: WNDPROC,
        recovery_posted: bool,
        checked: std::time::Instant,
        topology_changed: bool,
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
            WM_DISPLAYCHANGE => {
                WINDOWS.with(|windows| {
                    if let Some(state) = windows.borrow_mut().get_mut(&key) {
                        state.topology_changed = true;
                    }
                });
                unsafe {
                    PostMessageW(hwnd, RECOVER, 0, 0);
                }
            }
            RECOVER => {
                let (moving, topology_changed) = WINDOWS.with(|windows| {
                    if let Some(state) = windows.borrow_mut().get_mut(&key) {
                        state.recovery_posted = false;
                        let moving = state.transition.moving;
                        let changed = state.topology_changed;
                        if !moving {
                            state.topology_changed = false;
                        }
                        (moving, changed)
                    } else {
                        (true, false)
                    }
                });
                if !moving {
                    unsafe {
                        recover_window(hwnd, topology_changed);
                    }
                }
            }
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
            let post = WINDOWS.with(|windows| {
                let mut windows = windows.borrow_mut();
                let state = windows.get_mut(&(hwnd as isize)).unwrap();
                if !state.transition.moving
                    && !state.recovery_posted
                    && state.checked.elapsed() >= std::time::Duration::from_secs(1)
                {
                    state.checked = std::time::Instant::now();
                    state.recovery_posted = true;
                    true
                } else {
                    false
                }
            });
            if post {
                unsafe {
                    PostMessageW(hwnd, RECOVER, 0, 0);
                }
            }
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
                        recovery_posted: false,
                        checked: std::time::Instant::now() - std::time::Duration::from_secs(2),
                        topology_changed: true,
                    },
                );
            });
            unsafe {
                PostMessageW(hwnd, RECOVER, 0, 0);
            }
        }
        1
    }
    unsafe extern "system" fn monitor_work(
        monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
        _: windows_sys::Win32::Graphics::Gdi::HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        use windows_sys::Win32::Graphics::Gdi::*;
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            unsafe { &mut *(data as *mut Vec<[i32; 4]>) }.push([
                info.rcWork.left,
                info.rcWork.top,
                info.rcWork.right,
                info.rcWork.bottom,
            ]);
        }
        1
    }
    unsafe fn recover_window(hwnd: HWND, topology_changed: bool) {
        use windows_sys::Win32::Graphics::Gdi::*;
        if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
            return;
        }
        let mut bounds: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
            return;
        }
        let mut work = Vec::<[i32; 4]>::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_work),
                &mut work as *mut _ as LPARAM,
            );
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let bounds = [bounds.left, bounds.top, bounds.right, bounds.bottom];
        let recovered = if topology_changed {
            super::topology_bounds(bounds, &work, (32 * dpi / 96) as i32)
        } else {
            super::recover_bounds(bounds, &work, (32 * dpi / 96) as i32)
        };
        if let Some([left, top, right, bottom]) = recovered {
            // Deferred user-message handling, never inside WM_DPICHANGED or
            // WM_DISPLAYCHANGE. Do not activate, steal focus or send input.
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    left,
                    top,
                    right - left,
                    bottom - top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
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
        #[test]
        fn native_offscreen_recovery_waits_for_drag_end_and_returns_its_title_to_a_monitor() {
            let class: Vec<_> = "STATIC".encode_utf16().chain([0]).collect();
            let title: Vec<_> = "AetherDesk monitor recovery fixture"
                .encode_utf16()
                .chain([0])
                .collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    100000,
                    100000,
                    500,
                    300,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            unsafe {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    100000,
                    100000,
                    500,
                    300,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
            install();
            let mut original: RECT = unsafe { std::mem::zeroed() };
            unsafe {
                GetWindowRect(hwnd, &mut original);
            }
            unsafe {
                SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            let mut before: RECT = unsafe { std::mem::zeroed() };
            unsafe {
                GetWindowRect(hwnd, &mut before);
            }
            assert_eq!(
                [before.left, before.top, before.right, before.bottom],
                [original.left, original.top, original.right, original.bottom],
                "recovery must not interfere with an active drag"
            );
            unsafe {
                SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            let mut after: RECT = unsafe { std::mem::zeroed() };
            unsafe {
                GetWindowRect(hwnd, &mut after);
            }
            let mut work = Vec::<[i32; 4]>::new();
            unsafe {
                windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    Some(monitor_work),
                    &mut work as *mut _ as LPARAM,
                );
                DestroyWindow(hwnd);
            }
            assert!(!work.is_empty());
            assert!(
                super::super::recover_bounds(
                    [after.left, after.top, after.right, after.bottom],
                    &work,
                    32
                )
                .is_none(),
                "the recovered title must be reachable"
            );
        }
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
    fn topology_changes_fit_oversized_windows_but_preserve_intentional_multi_monitor_spans() {
        let main = [0, 0, 1920, 1040];
        let right = [1920, 0, 3840, 1040];
        assert!(topology_bounds([100, 50, 3500, 900], &[main, right], 32).is_none());
        assert_eq!(
            topology_bounds([100, 50, 3500, 900], &[main], 32),
            Some([0, 50, 1920, 900])
        );
        assert_eq!(
            topology_bounds([100, 0, 1500, 1400], &[main], 32),
            Some([100, 0, 1500, 1040])
        );
        assert!(recover_bounds([100, 0, 1500, 1400], &[main], 32).is_none());
    }
    #[test]
    fn removed_monitors_recover_left_above_and_oversized_windows_without_moving_reachable_titles() {
        let main = [0, 0, 1920, 1040];
        let left = [-1920, 0, 0, 1080];
        let window = [-1800, 100, -800, 800];
        assert!(recover_bounds(window, &[left, main], 32).is_none());
        assert_eq!(
            recover_bounds(window, &[main], 32),
            Some([0, 100, 1000, 800])
        );
        assert_eq!(
            recover_bounds([100, -1000, 1100, -300], &[main], 32),
            Some([100, 0, 1100, 700])
        );
        assert_eq!(
            recover_bounds([2200, 0, 5200, 1800], &[[0, 0, 1280, 720]], 48),
            Some([0, 0, 1280, 720])
        );
        assert!(
            recover_bounds([-200, 50, 800, 700], &[main], 32).is_none(),
            "a partly off-screen but reachable title is intentional"
        );
        assert!(
            recover_bounds(window, &[], 32).is_none(),
            "an empty topology during a display change must not invent a monitor"
        );
    }
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
