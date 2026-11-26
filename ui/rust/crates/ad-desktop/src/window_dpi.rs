//! Coalesce native DPI changes during a Windows move/size transaction.
//! Winit otherwise resizes and nudges the window at each monitor boundary,
//! before the frame-level geometry settling policy can run.
#[derive(Default)]
struct Transition {
    moving: bool,
    pending: Option<usize>,
}
fn usable_bounds(bounds: [i32; 4], title_height: i32) -> bool {
    let title = i64::from(title_height.max(16));
    i64::from(bounds[2]) - i64::from(bounds[0]) >= title * 3
        && i64::from(bounds[3]) - i64::from(bounds[1]) >= title * 2
}

// A minimized/icon-sized rectangle is not a user's preferred normal size.
// This also repairs already-corrupted placement loaded from an older build.
fn normal_bounds(
    bounds: [i32; 4],
    work: &[[i32; 4]],
    title_height: i32,
    last_good: Option<[i32; 4]>,
    topology_changed: bool,
) -> Option<[i32; 4]> {
    if !usable_bounds(bounds, title_height) {
        let candidate = last_good
            .filter(|b| usable_bounds(*b, title_height))
            .unwrap_or_else(|| {
                let scale = i64::from(title_height.max(16));
                [
                    bounds[0],
                    bounds[1],
                    (i64::from(bounds[0]) + 960 * scale / 32)
                        .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                    (i64::from(bounds[1]) + 640 * scale / 32)
                        .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                ]
            });
        let area = work
            .iter()
            .filter(|a| a[2] > a[0] && a[3] > a[1])
            .min_by_key(|a| {
                let dx = i64::from(candidate[0]).clamp(i64::from(a[0]), i64::from(a[2]))
                    - i64::from(candidate[0]);
                let dy = i64::from(candidate[1]).clamp(i64::from(a[1]), i64::from(a[3]))
                    - i64::from(candidate[1]);
                i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy)
            })?;
        let width = (i64::from(candidate[2]) - i64::from(candidate[0]))
            .max(1)
            .min(i64::from(area[2]) - i64::from(area[0])) as i32;
        let height = (i64::from(candidate[3]) - i64::from(candidate[1]))
            .max(1)
            .min(i64::from(area[3]) - i64::from(area[1])) as i32;
        let x = candidate[0].clamp(area[0], area[2] - width);
        let y = candidate[1].clamp(area[1], area[3] - height);
        return Some([x, y, x + width, y + height]);
    }
    if topology_changed {
        topology_bounds(bounds, work, title_height)
    } else {
        recover_bounds(bounds, work, title_height)
    }
}

fn offset_bounds(bounds: [i32; 4], offset: [i32; 2]) -> [i32; 4] {
    [
        bounds[0].saturating_add(offset[0]),
        bounds[1].saturating_add(offset[1]),
        bounds[2].saturating_add(offset[0]),
        bounds[3].saturating_add(offset[1]),
    ]
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
    const RESTORE: u32 = WM_APP + 0x3d3;
    const RECOVERY_TIMER: usize = 0x4144_4450;
    const APPLICATION_POPUP: *const u16 =
        windows_sys::w!("AetherDesk.WindowGeometry.ApplicationPopup");

    pub(crate) fn exclude_application_popup(hwnd: isize) -> Result<(), String> {
        let hwnd = hwnd as HWND;
        // The flag belongs to this HWND for its entire lifetime. The egui
        // viewport can outlive its popup binding for a frame while it closes.
        if unsafe { GetPropW(hwnd, APPLICATION_POPUP) }.is_null()
            && unsafe { SetPropW(hwnd, APPLICATION_POPUP, 1 as HANDLE) } == 0
        {
            return Err(format!(
                "Cannot exclude the application popup from window recovery: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn can_recover(hwnd: HWND) -> bool {
        // Winit can retain WS_CAPTION even for visually undecorated windows.
        unsafe {
            GetPropW(hwnd, APPLICATION_POPUP).is_null()
                && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION == WS_CAPTION
        }
    }
    struct WindowState {
        transition: Transition,
        previous: WNDPROC,
        recovery_posted: bool,
        checked: std::time::Instant,
        topology_changed: bool,
        last_normal: Option<[i32; 4]>,
        repairing: bool,
        restore: Option<(WPARAM, LPARAM)>,
    }
    thread_local! {
        static WINDOWS: RefCell<BTreeMap<isize, WindowState>> = RefCell::new(BTreeMap::new());
    }
    #[cfg(test)]
    thread_local! {
        static TEST_WORK: RefCell<Option<Vec<[i32;4]>>> = const { RefCell::new(None) };
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
            WM_TIMER if wparam == RECOVERY_TIMER => {
                unsafe {
                    PostMessageW(hwnd, RECOVER, 0, 0);
                }
                return 0;
            }
            WM_SYSCOMMAND if wparam as u32 & 0xfff0 == SC_RESTORE => {
                let defer = WINDOWS.with(|windows| {
                    let mut windows = windows.borrow_mut();
                    let Some(state) = windows.get_mut(&key) else {
                        return false;
                    };
                    if state.repairing {
                        return false;
                    }
                    state.restore = Some((wparam, lparam));
                    true
                });
                if defer {
                    unsafe {
                        PostMessageW(hwnd, RESTORE, 0, 0);
                    }
                    return 0;
                }
            }
            RESTORE => {
                let restore = WINDOWS.with(|windows| {
                    let mut windows = windows.borrow_mut();
                    let state = windows.get_mut(&key)?;
                    if state.transition.moving {
                        return None;
                    }
                    state.restore.take()
                });
                if let Some((w, l)) = restore {
                    unsafe {
                        recover_window(hwnd, true);
                        return CallWindowProcW(previous, hwnd, WM_SYSCOMMAND, w, l);
                    }
                }
                return 0;
            }
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
                unsafe {
                    PostMessageW(hwnd, RECOVER, 0, 0);
                    PostMessageW(hwnd, RESTORE, 0, 0);
                }
                return result;
            }
            APPLY => {
                // A minimized icon has no usable client size. Deliver its
                // pending scale after Restore, rather than adopting icon bounds.
                if unsafe { IsIconic(hwnd) } != 0 {
                    return 0;
                }
                let pending = WINDOWS.with(|windows| {
                    windows
                        .borrow_mut()
                        .get_mut(&key)
                        .and_then(|state| state.transition.take())
                });
                if let Some(dpi) = pending {
                    let saved = if unsafe { IsZoomed(hwnd) } != 0 {
                        unsafe {
                            recover_window(hwnd, true);
                        }
                        let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
                        p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                        (unsafe { GetWindowPlacement(hwnd, &mut p) } != 0).then_some(p)
                    } else {
                        None
                    };
                    let current = unsafe { GetDpiForWindow(hwnd) };
                    let dpi = if current > 0 {
                        current as usize | ((current as usize) << 16)
                    } else {
                        dpi
                    };
                    let mut bounds: RECT = unsafe { std::mem::zeroed() };
                    if unsafe { GetWindowRect(hwnd, &mut bounds) } != 0 {
                        let title = (32 * current.max(96) / 96) as i32;
                        if !super::usable_bounds(
                            [bounds.left, bounds.top, bounds.right, bounds.bottom],
                            title,
                        ) {
                            unsafe {
                                recover_window(hwnd, false);
                                GetWindowRect(hwnd, &mut bounds);
                            }
                            if !super::usable_bounds(
                                [bounds.left, bounds.top, bounds.right, bounds.bottom],
                                title,
                            ) {
                                return 0;
                            }
                        }
                        // Use the final drop position, not the stale rectangle
                        // suggested when the cursor first crossed the boundary.
                        let previous_repairing = WINDOWS.with(|windows| {
                            let mut windows = windows.borrow_mut();
                            let state = windows.get_mut(&key)?;
                            let previous = state.repairing;
                            if saved.is_some() {
                                state.repairing = true;
                            }
                            Some(previous)
                        });
                        let result = unsafe {
                            CallWindowProcW(
                                previous,
                                hwnd,
                                WM_DPICHANGED,
                                dpi,
                                (&bounds as *const RECT) as LPARAM,
                            )
                        };
                        // Winit must learn the new scale even when maximized,
                        // but its resize callback must not replace the saved
                        // normal rectangle with the maximized client bounds.
                        if let Some(saved) = saved {
                            let mut after: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
                            after.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                            if unsafe { GetWindowPlacement(hwnd, &mut after) } != 0
                                && (after.showCmd != saved.showCmd
                                    || after.flags != saved.flags
                                    || [
                                        after.rcNormalPosition.left,
                                        after.rcNormalPosition.top,
                                        after.rcNormalPosition.right,
                                        after.rcNormalPosition.bottom,
                                    ] != [
                                        saved.rcNormalPosition.left,
                                        saved.rcNormalPosition.top,
                                        saved.rcNormalPosition.right,
                                        saved.rcNormalPosition.bottom,
                                    ])
                            {
                                unsafe {
                                    SetWindowPlacement(hwnd, &saved);
                                }
                            }
                        }
                        if let Some(previous) = previous_repairing {
                            WINDOWS.with(|windows| {
                                if let Some(state) = windows.borrow_mut().get_mut(&key) {
                                    state.repairing = previous;
                                }
                            });
                        }
                        return result;
                    }
                }
                return 0;
            }
            WM_NCDESTROY => {
                unsafe {
                    KillTimer(hwnd, RECOVERY_TIMER);
                }
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
            WM_SIZE if wparam == SIZE_RESTORED as usize || wparam == SIZE_MAXIMIZED as usize => {
                let post = WINDOWS.with(|windows| {
                    windows
                        .borrow()
                        .get(&key)
                        .is_some_and(|s| !s.repairing && !s.transition.moving)
                });
                if post {
                    unsafe {
                        PostMessageW(hwnd, APPLY, 0, 0);
                        PostMessageW(hwnd, RECOVER, 0, 0);
                    }
                }
            }
            _ => {}
        }
        unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
    }
    unsafe extern "system" fn install_window(hwnd: HWND, _: LPARAM) -> BOOL {
        // Apply recovery only to normal windows. Popup geometry is owned by
        // its view, including when the window still carries WS_CAPTION.
        if !can_recover(hwnd) {
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
                        last_normal: None,
                        repairing: false,
                        restore: None,
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
        // Recheck the role when servicing messages already queued for this
        // window before its native popup binding was available.
        if unsafe { IsWindowVisible(hwnd) } == 0 || !can_recover(hwnd) {
            unsafe {
                KillTimer(hwnd, RECOVERY_TIMER);
            }
            return;
        }
        let key = hwnd as isize;
        let admitted = WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            let Some(state) = windows.get_mut(&key) else {
                return false;
            };
            if state.repairing || state.transition.moving {
                return false;
            }
            state.repairing = true;
            true
        });
        if !admitted {
            return;
        }
        struct Guard(isize);
        impl Drop for Guard {
            fn drop(&mut self) {
                WINDOWS.with(|windows| {
                    if let Some(state) = windows.borrow_mut().get_mut(&self.0) {
                        state.repairing = false;
                    }
                });
            }
        }
        let _guard = Guard(key);
        let mut work = Vec::<[i32; 4]>::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_work),
                &mut work as *mut _ as LPARAM,
            );
        }
        #[cfg(test)]
        TEST_WORK.with(|override_work| {
            if let Some(override_work) = &*override_work.borrow() {
                work = override_work.clone();
            }
        });
        if work.is_empty() {
            // Display reconfiguration may temporarily expose no monitors.
            // Retain the pending topology change for the next deferred check.
            unsafe {
                SetTimer(hwnd, RECOVERY_TIMER, 200, None);
            }
            return;
        }
        unsafe {
            KillTimer(hwnd, RECOVERY_TIMER);
        }
        if topology_changed {
            WINDOWS.with(|windows| {
                if let Some(state) = windows.borrow_mut().get_mut(&key) {
                    state.topology_changed = false;
                }
            });
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let title = (32 * dpi / 96) as i32;
        let last = WINDOWS.with(|windows| windows.borrow().get(&key).and_then(|s| s.last_normal));
        let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        let valid_placement = unsafe { GetWindowPlacement(hwnd, &mut placement) } != 0;
        if unsafe { IsIconic(hwnd) } != 0 || unsafe { IsZoomed(hwnd) } != 0 {
            if valid_placement {
                // rcNormalPosition is in workspace coordinates for ordinary
                // top-level windows, unlike GetWindowRect/SetWindowPos.
                let normal = placement.rcNormalPosition;
                let screen = super::offset_bounds(
                    [normal.left, normal.top, normal.right, normal.bottom],
                    unsafe { placement_offset(hwnd, &normal) },
                );
                if let Some(repaired) =
                    super::normal_bounds(screen, &work, title, last, topology_changed)
                {
                    let screen = RECT {
                        left: repaired[0],
                        top: repaired[1],
                        right: repaired[2],
                        bottom: repaired[3],
                    };
                    let offset = unsafe { placement_offset(hwnd, &screen) };
                    let normal = super::offset_bounds(repaired, [-offset[0], -offset[1]]);
                    placement.rcNormalPosition = RECT {
                        left: normal[0],
                        top: normal[1],
                        right: normal[2],
                        bottom: normal[3],
                    };
                    // Preserve showCmd, flags and maximized/minimized state.
                    // In particular never SetWindowPos a maximized rectangle
                    // and leave its invalid restore placement untouched.
                    unsafe {
                        SetWindowPlacement(hwnd, &placement);
                    }
                }
            }
            return;
        }
        let mut bounds: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
            return;
        }
        let bounds = [bounds.left, bounds.top, bounds.right, bounds.bottom];
        let recovered = super::normal_bounds(bounds, &work, title, last, topology_changed);
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
        let mut actual: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut actual) } != 0 {
            let actual = [actual.left, actual.top, actual.right, actual.bottom];
            if super::usable_bounds(actual, title)
                && super::recover_bounds(actual, &work, title).is_none()
            {
                WINDOWS.with(|windows| {
                    if let Some(state) = windows.borrow_mut().get_mut(&key) {
                        state.last_normal = Some(actual);
                    }
                });
            }
        }
    }
    unsafe fn placement_offset(hwnd: HWND, rect: &RECT) -> [i32; 2] {
        use windows_sys::Win32::Graphics::Gdi::*;
        if unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW != 0 {
            return [0, 0];
        }
        let monitor = unsafe { MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return [0, 0];
        }
        [
            info.rcWork.left - info.rcMonitor.left,
            info.rcWork.top - info.rcMonitor.top,
        ]
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
        fn captioned_application_popup_skips_new_and_queued_window_recovery() {
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!("Application menu"),
                    WS_POPUP | WS_CAPTION,
                    100,
                    100,
                    49,
                    20,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            struct Cleanup(HWND);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe { DestroyWindow(self.0) };
                }
            }
            let _cleanup = Cleanup(hwnd);
            unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            let bounds = || {
                let mut rect: RECT = unsafe { std::mem::zeroed() };
                assert_ne!(unsafe { GetWindowRect(hwnd, &mut rect) }, 0);
                [rect.left, rect.top, rect.right, rect.bottom]
            };
            let initial = bounds();
            let title_height = (32 * unsafe { GetDpiForWindow(hwnd) }.max(96) / 96) as i32;
            assert!(!super::super::usable_bounds(initial, title_height));
            assert!(can_recover(hwnd));
            let is_tracked = || WINDOWS.with(|w| w.borrow().contains_key(&(hwnd as isize)));

            exclude_application_popup(hwnd as isize).unwrap();
            unsafe { install_window(hwnd, 0) };
            assert!(!is_tracked());

            // Simulate a recovery message queued before the popup was marked.
            unsafe { RemovePropW(hwnd, APPLICATION_POPUP) };
            unsafe { install_window(hwnd, 0) };
            assert!(is_tracked());
            exclude_application_popup(hwnd as isize).unwrap();
            unsafe { SendMessageW(hwnd, RECOVER, 0, 0) };
            assert_eq!(bounds(), initial);
        }

        #[test]
        fn native_restore_repairs_saved_placement_and_collapsed_windows_without_ending_sessions() {
            let class: Vec<_> = "STATIC".encode_utf16().chain([0]).collect();
            let title: Vec<_> = "AetherDesk Build 10 geometry fixture"
                .encode_utf16()
                .chain([0])
                .collect();
            let mut work = Vec::<[i32; 4]>::new();
            unsafe {
                windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    Some(monitor_work),
                    &mut work as *mut _ as LPARAM,
                );
            }
            let area = work[0];
            let width = 800.min(area[2] - area[0]);
            let height = 520.min(area[3] - area[1]);
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    area[0],
                    area[1],
                    width,
                    height,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            struct Cleanup(HWND);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let _cleanup = Cleanup(hwnd);
            unsafe {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            install();
            unsafe {
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            let good = WINDOWS
                .with(|w| w.borrow()[&(hwnd as isize)].last_normal)
                .unwrap();
            let title_height = (32 * unsafe { GetDpiForWindow(hwnd) }.max(96) / 96) as i32;
            // Retry independently of rendering: a collapsed surface can stop
            // producing GUI frames while Windows reconfigures its displays.
            let mut pending: MSG = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut pending, hwnd, RECOVER, RECOVER, PM_REMOVE) } != 0 {}
            TEST_WORK.with(|work| *work.borrow_mut() = Some(Vec::new()));
            unsafe {
                SendMessageW(hwnd, WM_DISPLAYCHANGE, 0, 0);
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            assert!(WINDOWS.with(|w| w.borrow()[&(hwnd as isize)].topology_changed));
            while unsafe { PeekMessageW(&mut pending, hwnd, RECOVER, RECOVER, PM_REMOVE) } != 0 {}
            TEST_WORK.with(|work| *work.borrow_mut() = None);
            unsafe {
                SendMessageW(hwnd, WM_TIMER, RECOVERY_TIMER, 0);
            }
            assert_ne!(
                unsafe { PeekMessageW(&mut pending, hwnd, RECOVER, RECOVER, PM_REMOVE) },
                0
            );
            unsafe {
                DispatchMessageW(&pending);
            }
            assert!(!WINDOWS.with(|w| w.borrow()[&(hwnd as isize)].topology_changed));
            // A drag can temporarily report a collapsed surface. Do not adopt
            // it as the last normal size or interfere until the drag ends.
            unsafe {
                SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    area[0],
                    area[1],
                    2,
                    30,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING,
                );
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            assert_eq!(
                WINDOWS.with(|w| w.borrow()[&(hwnd as isize)].last_normal),
                Some(good)
            );
            unsafe {
                SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
                SendMessageW(hwnd, RECOVER, 0, 0);
            }
            let mut actual: RECT = unsafe { std::mem::zeroed() };
            unsafe {
                GetWindowRect(hwnd, &mut actual);
            }
            assert!(super::super::usable_bounds(
                [actual.left, actual.top, actual.right, actual.bottom],
                title_height
            ));
            for show in [SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE] {
                unsafe {
                    ShowWindow(hwnd, show);
                }
                let mut p: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
                p.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                assert_ne!(unsafe { GetWindowPlacement(hwnd, &mut p) }, 0);
                p.rcNormalPosition = RECT {
                    left: area[2] - 1,
                    top: area[1],
                    right: area[2] + 1,
                    bottom: area[1] + 30,
                };
                assert_ne!(unsafe { SetWindowPlacement(hwnd, &p) }, 0);
                let zoomed = unsafe { IsZoomed(hwnd) };
                let iconic = unsafe { IsIconic(hwnd) };
                let foreground = unsafe { GetForegroundWindow() };
                unsafe {
                    SendMessageW(hwnd, WM_DISPLAYCHANGE, 0, 0);
                    SendMessageW(hwnd, RECOVER, 0, 0);
                }
                assert_eq!(
                    unsafe { IsZoomed(hwnd) },
                    zoomed,
                    "recovery changed maximize state"
                );
                assert_eq!(
                    unsafe { IsIconic(hwnd) },
                    iconic,
                    "recovery changed minimize state"
                );
                assert_eq!(
                    unsafe { GetForegroundWindow() },
                    foreground,
                    "background recovery stole focus"
                );
                assert_ne!(unsafe { GetWindowPlacement(hwnd, &mut p) }, 0);
                let normal = p.rcNormalPosition;
                let screen = super::super::offset_bounds(
                    [normal.left, normal.top, normal.right, normal.bottom],
                    unsafe { placement_offset(hwnd, &normal) },
                );
                assert!(
                    super::super::usable_bounds(screen, title_height),
                    "normal placement stayed collapsed"
                );
                assert!(
                    super::super::normal_bounds(screen, &work, title_height, None, true).is_none()
                );
                // Corrupt the stored bounds once more, then Restore without
                // giving periodic recovery a chance to run first.
                p.rcNormalPosition = RECT {
                    left: area[2] - 1,
                    top: area[1],
                    right: area[2] + 1,
                    bottom: area[1] + 30,
                };
                assert_ne!(unsafe { SetWindowPlacement(hwnd, &p) }, 0);
                // Exercise the actual system-menu Restore command, not a test
                // shortcut that only calls our geometry helper.
                unsafe {
                    SendMessageW(hwnd, WM_SYSCOMMAND, SC_RESTORE as WPARAM, 0);
                }
                let mut message: MSG = unsafe { std::mem::zeroed() };
                assert_ne!(
                    unsafe { PeekMessageW(&mut message, hwnd, RESTORE, RESTORE, PM_REMOVE) },
                    0
                );
                unsafe {
                    DispatchMessageW(&message);
                    GetWindowRect(hwnd, &mut actual);
                }
                assert_eq!(unsafe { IsZoomed(hwnd) }, 0);
                assert_eq!(unsafe { IsIconic(hwnd) }, 0);
                let restored = [actual.left, actual.top, actual.right, actual.bottom];
                assert!(super::super::usable_bounds(restored, title_height));
                assert!(
                    super::super::normal_bounds(restored, &work, title_height, None, true)
                        .is_none(),
                    "Restore returned to an unreachable screen"
                );
            }
        }
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
        thread_local! {
            static DELIVERED: Cell<u32> = const { Cell::new(0) };
            static REWRITE_MAX_NORMAL: Cell<bool> = const { Cell::new(false) };
        }
        unsafe extern "system" fn fixture(
            hwnd: HWND,
            message: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            if message == WM_DPICHANGED {
                DELIVERED.with(|count| count.set(count.get() + 1));
                if REWRITE_MAX_NORMAL.with(Cell::get) && unsafe { IsZoomed(hwnd) } != 0 {
                    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
                    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                    let mut max: RECT = unsafe { std::mem::zeroed() };
                    unsafe {
                        GetWindowPlacement(hwnd, &mut placement);
                        GetWindowRect(hwnd, &mut max);
                        placement.rcNormalPosition = max;
                        SetWindowPlacement(hwnd, &placement);
                    }
                }
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
            // Finishing a drag and maximizing before the deferred message is
            // dispatched must not feed the maximized rectangle back as a new
            // normal size, or leave the maximized GUI at an obsolete scale.
            unsafe {
                ShowWindow(hwnd, SW_SHOWMAXIMIZED);
                SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            }
            let mut saved: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
            saved.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            unsafe {
                GetWindowPlacement(hwnd, &mut saved);
            }
            REWRITE_MAX_NORMAL.with(|value| value.set(true));
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_DPICHANGED,
                    192usize | (192usize << 16),
                    (&bounds as *const RECT) as LPARAM,
                );
                SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
                SendMessageW(hwnd, APPLY, 0, 0);
            }
            assert_eq!(
                DELIVERED.with(Cell::get),
                2,
                "maximized window did not learn its new DPI"
            );
            assert!(WINDOWS.with(|w| w.borrow()[&(hwnd as isize)].transition.pending.is_none()));
            assert_ne!(unsafe { IsZoomed(hwnd) }, 0);
            REWRITE_MAX_NORMAL.with(|value| value.set(false));
            let mut after = saved;
            unsafe {
                GetWindowPlacement(hwnd, &mut after);
            }
            assert_eq!(
                [
                    after.rcNormalPosition.left,
                    after.rcNormalPosition.top,
                    after.rcNormalPosition.right,
                    after.rcNormalPosition.bottom
                ],
                [
                    saved.rcNormalPosition.left,
                    saved.rcNormalPosition.top,
                    saved.rcNormalPosition.right,
                    saved.rcNormalPosition.bottom
                ],
                "DPI callback replaced Restore bounds with its maximized dimensions"
            );
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
                SendMessageW(hwnd, APPLY, 0, 0);
            }
            assert_eq!(DELIVERED.with(Cell::get), 2);
            assert!(WINDOWS.with(|w| w.borrow()[&(hwnd as isize)].transition.pending.is_none()));
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
#[cfg(windows)]
pub(crate) use native::exclude_application_popup;

pub(super) fn install() {
    #[cfg(windows)]
    native::install();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapsed_restore_bounds_use_last_normal_size_and_fit_the_remaining_monitor() {
        let main = [0, 48, 1920, 1040];
        let good = [150, 100, 1150, 800];
        for collapsed in [
            [0, 0, 2, 30],
            [0, 0, 242, 30],
            [0, 0, 0, 0],
            [100, 100, 100, 100],
        ] {
            assert_eq!(
                normal_bounds(collapsed, &[main], 32, Some(good), true),
                Some(good)
            );
            let repaired = normal_bounds(collapsed, &[main], 32, None, true).unwrap();
            assert!(usable_bounds(repaired, 32));
            assert!(normal_bounds(repaired, &[main], 32, None, true).is_none());
        }
        let left = [-1920, 0, 0, 1040];
        let repaired = normal_bounds([-1700, 100, -1699, 101], &[left], 32, None, true).unwrap();
        assert_eq!(repaired, [-1700, 100, -740, 740]);
        assert!(normal_bounds([0, 0, 1, 1], &[], 32, Some(good), true).is_none());
        assert!(
            normal_bounds([100, 100, 300, 180], &[main], 32, None, false).is_none(),
            "a valid small application must stay small"
        );
        assert!(normal_bounds(
            [100, 100, 3500, 900],
            &[[0, 0, 1920, 1040], [1920, 0, 3840, 1040]],
            32,
            None,
            true
        )
        .is_none());
    }
    #[test]
    fn normal_placement_workspace_offsets_preserve_top_and_left_taskbars_and_negative_origins() {
        for screen in [[100, 100, 900, 700], [-1800, 100, -1000, 700]] {
            for offset in [[0, 48], [80, 0], [64, 48], [0, 0]] {
                let normal = offset_bounds(screen, [-offset[0], -offset[1]]);
                assert_eq!(offset_bounds(normal, offset), screen);
            }
        }
    }
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
