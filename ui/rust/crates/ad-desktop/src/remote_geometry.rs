//! Monitor snapshots and window placement are in physical Windows pixels.
//! Egui points are used only inside one viewport, never to join monitor origins.
use ad_stream_protocol::Rectangle;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub device: String,
    pub rect: PhysicalRect,
    pub dpi: u32,
    pub primary: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub bounds: PhysicalRect,
    pub width: u32,
    pub height: u32,
    pub monitors: Vec<Rectangle>,
}
pub enum LayoutTarget<'a> {
    Tab(u32, u32),
    Current(Option<&'a str>),
    All,
}
pub fn session_layout(
    monitors: &[Monitor],
    target: LayoutTarget<'_>,
    scale: f32,
) -> Result<Layout, String> {
    let layout = match target {
        LayoutTarget::Tab(width, height) => arrange(&[Monitor {
            device: String::new(),
            rect: PhysicalRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            dpi: 96,
            primary: true,
        }]),
        LayoutTarget::Current(device) => select(monitors, device)
            .ok_or_else(|| "No client monitor is available".to_owned())
            .and_then(|m| arrange(&[m])),
        LayoutTarget::All => arrange(monitors),
    }?;
    resolution(&layout, scale)
}
pub fn arrange(monitors: &[Monitor]) -> Result<Layout, String> {
    if monitors.is_empty() || monitors.len() > 16 {
        return Err("Select between one and sixteen monitors".into());
    }
    if monitors
        .iter()
        .any(|m| m.rect.width == 0 || m.rect.height == 0)
    {
        return Err("Monitor dimensions must be positive".into());
    }
    let x = monitors.iter().map(|m| i64::from(m.rect.x)).min().unwrap();
    let y = monitors.iter().map(|m| i64::from(m.rect.y)).min().unwrap();
    let right = monitors
        .iter()
        .map(|m| i64::from(m.rect.x) + i64::from(m.rect.width))
        .max()
        .unwrap();
    let bottom = monitors
        .iter()
        .map(|m| i64::from(m.rect.y) + i64::from(m.rect.height))
        .max()
        .unwrap();
    let (w, h) = ((right - x) as f64, (bottom - y) as f64);
    if w > i32::MAX as f64 || h > i32::MAX as f64 {
        return Err("Physical monitor arrangement is too large".into());
    }
    let scale = 1.0_f64
        .min(16384.0 / w)
        .min(16384.0 / h)
        .min((33554432.0 / (w * h)).sqrt());
    let width = ((w * scale) as u32 / 2 * 2).max(64);
    let height = ((h * scale) as u32 / 2 * 2).max(64);
    let regions = monitors
        .iter()
        .map(|m| {
            let left = ((i64::from(m.rect.x) - x) as f64 * width as f64 / w).round() as u32;
            let top = ((i64::from(m.rect.y) - y) as f64 * height as f64 / h).round() as u32;
            let right = ((i64::from(m.rect.x) + i64::from(m.rect.width) - x) as f64 * width as f64
                / w)
                .round() as u32;
            let bottom =
                ((i64::from(m.rect.y) + i64::from(m.rect.height) - y) as f64 * height as f64 / h)
                    .round() as u32;
            Rectangle {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            }
        })
        .collect::<Vec<_>>();
    if !ad_stream_protocol::valid_monitor_layout(width, height, &regions) {
        return Err("Monitor layout exceeds the desktop protocol limits".into());
    }
    Ok(Layout {
        bounds: PhysicalRect {
            x: x as i32,
            y: y as i32,
            width: w as u32,
            height: h as u32,
        },
        width,
        height,
        monitors: regions,
    })
}
pub fn resolution(layout: &Layout, scale: f32) -> Result<Layout, String> {
    if !scale.is_finite() || !(0.5..=2.0).contains(&scale) {
        return Err("Invalid desktop resolution scale".into());
    }
    let width = ((layout.width as f32 * scale).round() as u32 / 2 * 2).max(64);
    let height = ((layout.height as f32 * scale).round() as u32 / 2 * 2).max(64);
    let x = |n: u32| (u64::from(n) * u64::from(width) / u64::from(layout.width)) as u32;
    let y = |n: u32| (u64::from(n) * u64::from(height) / u64::from(layout.height)) as u32;
    let monitors = layout
        .monitors
        .iter()
        .map(|m| Rectangle {
            x: x(m.x),
            y: y(m.y),
            width: x(m.x + m.width) - x(m.x),
            height: y(m.y + m.height) - y(m.y),
        })
        .collect::<Vec<_>>();
    if !ad_stream_protocol::valid_monitor_layout(width, height, &monitors) {
        return Err("Selected resolution exceeds the supported desktop size".into());
    }
    Ok(Layout {
        bounds: layout.bounds,
        width,
        height,
        monitors,
    })
}
pub fn monitor_properties(monitor: &Monitor) -> ad_stream_protocol::MonitorProperties {
    let dpi = monitor.dpi.clamp(48, 768) as u16;
    let (width_mm, height_mm, refresh_millihz) = display_dimensions(monitor).unwrap_or_else(|| {
        (
            (monitor.rect.width * 254 / (u32::from(dpi) * 10)).clamp(10, 10000),
            (monitor.rect.height * 254 / (u32::from(dpi) * 10)).clamp(10, 10000),
            60000,
        )
    });
    ad_stream_protocol::MonitorProperties {
        name: monitor.device.clone(),
        dpi,
        width_mm,
        height_mm,
        refresh_millihz,
        primary: monitor.primary,
    }
}
pub fn single_size(width: u32, height: u32) -> (u32, u32) {
    let monitor = Monitor {
        device: String::new(),
        rect: PhysicalRect {
            x: 0,
            y: 0,
            width: width.max(64),
            height: height.max(64),
        },
        dpi: 96,
        primary: true,
    };
    arrange(&[monitor])
        .map(|l| (l.width, l.height))
        .unwrap_or((1280, 720))
}
pub fn select(monitors: &[Monitor], device: Option<&str>) -> Option<Monitor> {
    monitors
        .iter()
        .find(|m| Some(m.device.as_str()) == device)
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or(monitors.first())
        .cloned()
}
#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    };
    struct DpiScope(DPI_AWARENESS_CONTEXT);
    impl DpiScope {
        fn physical() -> Self {
            Self(unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            })
        }
    }
    impl Drop for DpiScope {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
    }
    pub fn monitors() -> Vec<Monitor> {
        unsafe extern "system" fn collect(
            handle: HMONITOR,
            _: HDC,
            _: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()) } != 0 {
                let r = info.monitorInfo.rcMonitor;
                let mut x = 96;
                let mut y = 96;
                unsafe {
                    GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y);
                }
                if r.right > r.left && r.bottom > r.top {
                    unsafe {
                        (*(data as *mut Vec<Monitor>)).push(Monitor {
                            device: String::from_utf16_lossy(
                                &info.szDevice
                                    [..info.szDevice.iter().position(|v| *v == 0).unwrap_or(32)],
                            ),
                            rect: PhysicalRect {
                                x: r.left,
                                y: r.top,
                                width: (r.right - r.left) as u32,
                                height: (r.bottom - r.top) as u32,
                            },
                            dpi: x.max(96),
                            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                        });
                    }
                }
            }
            1
        }
        let _dpi = DpiScope::physical();
        let mut result: Vec<Monitor> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                &mut result as *mut Vec<Monitor> as isize,
            );
        }
        result.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.device.cmp(&b.device)));
        result
    }
    pub fn cursor_monitor() -> Option<String> {
        let _dpi = DpiScope::physical();
        let mut point = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) } == 0 {
            return None;
        }
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(
            &info.szDevice[..info.szDevice.iter().position(|v| *v == 0).unwrap_or(32)],
        ))
    }
    pub fn window_monitor(hwnd: isize) -> Option<String> {
        let monitor = unsafe { MonitorFromWindow(hwnd as HWND, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(
            &info.szDevice[..info.szDevice.iter().position(|v| *v == 0).unwrap_or(32)],
        ))
    }
    pub fn foreground() -> Option<isize> {
        let hwnd = unsafe { GetForegroundWindow() };
        let mut process = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut process);
        }
        if !hwnd.is_null() && process == std::process::id() {
            Some(hwnd as isize)
        } else {
            None
        }
    }
    pub fn window(title: &str) -> Option<isize> {
        struct Search<'a> {
            title: &'a str,
            found: Option<isize>,
        }
        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
            let mut process = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut process);
            }
            if process != std::process::id() {
                return 1;
            }
            let mut text = [0u16; 512];
            let len = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), 512) };
            let search = unsafe { &mut *(data as *mut Search<'_>) };
            if len > 0 && String::from_utf16_lossy(&text[..len as usize]) == search.title {
                search.found = Some(hwnd as isize);
                return 0;
            }
            1
        }
        let mut search = Search { title, found: None };
        unsafe {
            EnumWindows(Some(collect), &mut search as *mut Search<'_> as isize);
        }
        search.found
    }
    pub fn display_dimensions(monitor: &Monitor) -> Option<(u32, u32, u32)> {
        let driver: Vec<u16> = "DISPLAY".encode_utf16().chain(Some(0)).collect();
        let device: Vec<u16> = monitor.device.encode_utf16().chain(Some(0)).collect();
        let dc = unsafe {
            CreateDCW(
                driver.as_ptr(),
                device.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if dc.is_null() {
            return None;
        }
        let width = unsafe { GetDeviceCaps(dc, HORZSIZE as i32) };
        let height = unsafe { GetDeviceCaps(dc, VERTSIZE as i32) };
        let refresh = unsafe { GetDeviceCaps(dc, VREFRESH as i32) };
        unsafe {
            DeleteDC(dc);
        }
        if width < 10 || height < 10 || width > 10000 || height > 10000 {
            return None;
        }
        Some((
            width as u32,
            height as u32,
            if (1..=240).contains(&refresh) {
                refresh as u32 * 1000
            } else {
                60000
            },
        ))
    }
    pub fn place(hwnd: isize, rect: PhysicalRect, title: &str) -> Result<(), String> {
        let _dpi = DpiScope::physical();
        let hwnd = hwnd as HWND;
        let mut process = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut process);
        }
        if process != std::process::id() {
            return Err("Remote presentation window disappeared".into());
        }
        let mut text = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), 512) };
        if len <= 0 || String::from_utf16_lossy(&text[..len as usize]) != title {
            return Err("Remote presentation window was replaced".into());
        }
        let mut current: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(hwnd, &mut current) } != 0
            && current.left == rect.x
            && current.top == rect.y
            && current.right - current.left == rect.width as i32
            && current.bottom - current.top == rect.height as i32
        {
            return Ok(());
        }
        if unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width as i32,
                rect.height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        } == 0
        {
            return Err("Cannot position the remote monitor window".into());
        }
        Ok(())
    }
    pub fn client_size(hwnd: isize) -> Option<(u32, u32)> {
        let _dpi = DpiScope::physical();
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetClientRect(hwnd as HWND, &mut rect) } == 0
            || rect.right <= 0
            || rect.bottom <= 0
        {
            None
        } else {
            Some((rect.right as u32, rect.bottom as u32))
        }
    }
    pub fn screen_origin(hwnd: isize) -> Option<(i32, i32)> {
        let _dpi = DpiScope::physical();
        let mut point = POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd as HWND, &mut point) } == 0 {
            None
        } else {
            Some((point.x, point.y))
        }
    }
}
#[cfg(windows)]
pub use windows::*;
#[cfg(not(windows))]
pub fn monitors() -> Vec<Monitor> {
    vec![]
}
#[cfg(not(windows))]
pub fn cursor_monitor() -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn window_monitor(_: isize) -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn foreground() -> Option<isize> {
    None
}
#[cfg(not(windows))]
pub fn window(_: &str) -> Option<isize> {
    None
}
#[cfg(not(windows))]
pub fn place(_: isize, _: PhysicalRect, _: &str) -> Result<(), String> {
    Ok(())
}
#[cfg(not(windows))]
pub fn client_size(_: isize) -> Option<(u32, u32)> {
    None
}
#[cfg(not(windows))]
pub fn screen_origin(_: isize) -> Option<(i32, i32)> {
    None
}
#[cfg(not(windows))]
fn display_dimensions(_: &Monitor) -> Option<(u32, u32, u32)> {
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    fn monitor(x: i32, y: i32, width: u32, height: u32, dpi: u32) -> Monitor {
        Monitor {
            device: format!("{x},{y}"),
            rect: PhysicalRect {
                x,
                y,
                width,
                height,
            },
            dpi,
            primary: x == 0,
        }
    }
    #[test]
    fn resolution_selection_updates_tab_current_and_all_without_moving_windows() {
        let monitors = [
            monitor(-1920, 0, 1920, 1080, 96),
            monitor(0, 0, 2560, 1440, 144),
        ];
        let tab = session_layout(&monitors, LayoutTarget::Tab(1600, 900), 0.5).unwrap();
        assert_eq!((tab.width, tab.height, tab.monitors.len()), (800, 450, 1));
        assert_eq!((tab.bounds.width, tab.bounds.height), (1600, 900));
        let current = session_layout(
            &monitors,
            LayoutTarget::Current(Some(&monitors[1].device)),
            0.75,
        )
        .unwrap();
        assert_eq!(
            (current.width, current.height, current.monitors.len()),
            (1920, 1080, 1)
        );
        assert_eq!(current.bounds, monitors[1].rect);
        let all = session_layout(&monitors, LayoutTarget::All, 0.5).unwrap();
        assert_eq!((all.width, all.height, all.monitors.len()), (2240, 720, 2));
        assert_eq!(all.bounds, arrange(&monitors).unwrap().bounds);
        assert_eq!(all.monitors[0].x + all.monitors[0].width, all.monitors[1].x);
        assert!(ad_stream_protocol::valid_monitor_layout(
            all.width,
            all.height,
            &all.monitors
        ));
        assert!(session_layout(&monitors, LayoutTarget::All, f32::NAN).is_err());
        assert!(session_layout(&[], LayoutTarget::Current(None), 1.0).is_err());
    }
    #[test]
    fn explicit_virtual_resolution_keeps_monitor_positions_and_physical_window_bounds() {
        let original = arrange(&[
            monitor(-1920, 0, 1920, 1080, 96),
            monitor(0, 0, 2560, 1440, 144),
        ])
        .unwrap();
        let scaled = resolution(&original, 0.75).unwrap();
        assert_eq!(original.bounds, scaled.bounds);
        assert_eq!(
            scaled.monitors[0].x + scaled.monitors[0].width,
            scaled.monitors[1].x
        );
        assert!(scaled.width < original.width && scaled.height < original.height);
        assert!(ad_stream_protocol::valid_monitor_layout(
            scaled.width,
            scaled.height,
            &scaled.monitors
        ));
        assert_eq!(resolution(&original, 1.0).unwrap(), original);
        assert!(resolution(&original, f32::NAN).is_err());
    }
    #[test]
    fn mixed_dpi_negative_origins_use_physical_pixels() {
        let l = arrange(&[
            monitor(-2560, -200, 2560, 1440, 144),
            monitor(0, 0, 1920, 1080, 96),
        ])
        .unwrap();
        assert_eq!(l.bounds.x, -2560);
        assert_eq!(l.width, 4480);
        assert_eq!(l.height, 1440);
        assert_eq!(l.monitors[1].x, 2560);
        assert_eq!(l.monitors[1].y, 200);
        assert_eq!(l.monitors[0].width, 2560);
    }
    #[test]
    fn large_desktops_scale_all_regions_inside_protocol_bounds() {
        let l = arrange(&[
            monitor(-7680, 0, 7680, 4320, 192),
            monitor(0, 0, 7680, 4320, 96),
        ])
        .unwrap();
        assert!(ad_stream_protocol::valid_monitor_layout(
            l.width,
            l.height,
            &l.monitors
        ));
        assert_eq!(l.monitors[0].x + l.monitors[0].width, l.monitors[1].x);
    }
    #[test]
    fn odd_sizes_are_even_and_disappeared_devices_fall_back() {
        assert_eq!(single_size(1921, 1081), (1920, 1080));
        let m = monitor(0, 0, 1920, 1080, 96);
        assert_eq!(select(&[m.clone()], Some("unplugged")), Some(m));
    }
}
