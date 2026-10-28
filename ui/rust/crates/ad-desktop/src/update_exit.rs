//! Observe system shutdown without delaying it or installing from a callback.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENDING: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static DISPLAY_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn display_epoch() -> u64 {
    DISPLAY_EPOCH.load(Ordering::Acquire)
}

pub fn system_ending() -> bool {
    #[cfg(all(windows, not(test)))]
    {
        return !READY.load(Ordering::Acquire)
            || ENDING.load(Ordering::Acquire)
            || unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(0x2000) != 0
            };
    }
    #[cfg(any(not(windows), test))]
    {
        ENDING.load(Ordering::Acquire)
    }
}

pub async fn stable_display(cancel: &tokio_util::sync::CancellationToken) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let epoch = display_epoch();
        tokio::select! {
            _ = cancel.cancelled() => return Err("Connection cancelled".into()),
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {},
        }
        if epoch == display_epoch() || tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
    }
}

#[cfg(windows)]
pub fn display_description() -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        format!(
            "monitors={} virtual_origin={},{} virtual_size={}x{} display_epoch={}",
            GetSystemMetrics(SM_CMONITORS),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            display_epoch()
        )
    }
}
#[cfg(not(windows))]
pub fn display_description() -> String {
    "native display information unavailable".into()
}

#[cfg(all(windows, not(test)))]
pub fn watch_shutdown() {
    use windows_sys::Win32::{
        Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
    };
    static START: std::sync::Once = std::sync::Once::new();
    unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match message {
            WM_QUERYENDSESSION => {
                ENDING.store(true, Ordering::Release);
                1
            }
            WM_ENDSESSION => {
                ENDING.store(w != 0, Ordering::Release);
                0
            }
            WM_DISPLAYCHANGE => {
                DISPLAY_EPOCH.fetch_add(1, Ordering::AcqRel);
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("aetherdesk-system-events".into())
            .spawn(|| unsafe {
                let name: Vec<u16> = "AetherDeskSystemEvents"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let instance = GetModuleHandleW(std::ptr::null());
                let class = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance,
                    lpszClassName: name.as_ptr(),
                    ..std::mem::zeroed()
                };
                if RegisterClassW(&class) == 0 {
                    return;
                }
                // A message-only window does not receive shutdown/display broadcasts.
                // This top-level tool window remains hidden and owns no user content.
                let hwnd = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    name.as_ptr(),
                    name.as_ptr(),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return;
                }
                READY.store(true, Ordering::Release);
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                READY.store(false, Ordering::Release);
                DestroyWindow(hwnd);
            });
    });
}
#[cfg(any(not(windows), test))]
pub fn watch_shutdown() {
    READY.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn display_stabilization_is_cancellable_without_starting_a_server() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            super::stable_display(&cancel).await.unwrap_err(),
            "Connection cancelled"
        );
    }
}
