//! Read-only native window geometry. Never move a window from a native callback.
use eframe::egui;
#[derive(Clone, Copy)]
pub struct Area {
    pub work: egui::Rect,
    pub identity: egui::Rect,
    pub coherent: bool,
}
#[cfg(all(windows, not(test)))]
pub fn moving() -> bool {
    use windows_sys::Win32::{System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*};
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    unsafe {
        GetGUIThreadInfo(GetCurrentThreadId(), &mut info) != 0 && info.flags & GUI_INMOVESIZE != 0
    }
}
#[cfg(any(not(windows), test))]
pub fn moving() -> bool {
    false
}

#[cfg(all(windows, not(test)))]
pub fn area(ctx: &egui::Context) -> Option<Area> {
    use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::*, UI::HiDpi::*};
    let viewport = ctx.input(|i| i.viewport().clone());
    let outer = viewport.outer_rect?;
    let scale = ctx.pixels_per_point();
    if !outer.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let rect = RECT {
        left: (outer.left() * scale).round() as i32,
        top: (outer.top() * scale).round() as i32,
        right: (outer.right() * scale).round() as i32,
        bottom: (outer.bottom() * scale).round() as i32,
    };
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let physical = egui::Rect::from_min_max(
        egui::pos2(info.rcWork.left as f32, info.rcWork.top as f32),
        egui::pos2(info.rcWork.right as f32, info.rcWork.bottom as f32),
    );
    let mut x = 0;
    let mut y = 0;
    let dpi_ok = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } == 0;
    let native = viewport.native_pixels_per_point.unwrap_or(1.0);
    let coherent = (!dpi_ok || (x as f32 / 96.0 - native).abs() < 0.05)
        && (scale - native * ctx.zoom_factor()).abs() < 0.05;
    Some(Area {
        work: egui::Rect::from_min_max(physical.min / scale, physical.max / scale),
        identity: physical,
        coherent,
    })
}
#[cfg(any(not(windows), test))]
pub fn area(_: &egui::Context) -> Option<Area> {
    None
}
