//! Metadata-only hang evidence survives failures that never reach a panic hook.
//! Disk writes and bounded window probes run exclusively on a dedicated thread.
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
#[cfg(windows)]
static READY: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static PHASE: AtomicU8 = AtomicU8::new(0);
#[cfg(windows)]
static PULSE: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
fn elapsed() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}
pub fn pulse(phase: u8) {
    #[cfg(windows)]
    {
        PHASE.store(phase, Ordering::Relaxed);
        PULSE.store(elapsed(), Ordering::Release);
        READY.store(true, Ordering::Release);
    }
    #[cfg(not(windows))]
    let _ = phase;
}
#[cfg(any(windows, test))]
fn phase_name(phase: u8) -> &'static str {
    match phase {
        1 => "main layout",
        2 => "background results",
        3 => "workspace and views",
        4 => "renderer or Windows event loop",
        _ => "application startup",
    }
}
#[cfg(any(windows, test))]
#[derive(Default)]
struct Policy {
    last: Option<u64>,
    failed: u8,
    reported: bool,
}
#[cfg(any(windows, test))]
impl Policy {
    fn observe(&mut self, now: u64, age: u64, responsive: bool) -> Option<&'static str> {
        let gap = self
            .last
            .replace(now)
            .is_some_and(|last| now.saturating_sub(last) > 15_000);
        if age < 20_000 || responsive {
            self.failed = 0;
            return self.reported.then(|| {
                self.reported = false;
                "gui_recovered"
            });
        }
        // Sleep/CPU starvation can delay both threads. Require fresh failed
        // message probes after a gap, never infer a hang from wall-clock age.
        if gap {
            self.failed = 0;
            return None;
        }
        self.failed = self.failed.saturating_add(1);
        if self.failed >= 3 && !self.reported {
            self.reported = true;
            Some("gui_unresponsive")
        } else {
            None
        }
    }
}
pub struct Monitor {
    #[cfg(windows)]
    stop: Option<std::sync::mpsc::Sender<()>>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
pub fn start(directory: &std::path::Path) -> Monitor {
    #[cfg(not(windows))]
    {
        let _ = directory;
        Monitor {}
    }
    #[cfg(windows)]
    {
        use std::{io::Write, time::Duration};
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*,
        };
        let gui = unsafe { GetCurrentThreadId() };
        let path = directory.join(format!("health-{}.log", std::process::id()));
        let (stop, receive) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new().name("aetherdesk-health".into()).spawn(move || {
            let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return };
            let kind = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join("unins000.exe").is_file()));
            let _ = writeln!(file, "{}", serde_json::json!({"event":"session_started", "utc":chrono::Utc::now().to_rfc3339(),
                "pid":std::process::id(),"gui_thread":gui,"version":crate::update_manifest::current().ok(),
                "package_kind":match kind {Some(true)=>"installed",Some(false)=>"portable",None=>"unknown"}}));
            let _ = file.sync_data();
            let mut policy = Policy::default();
            loop {
                match receive.recv_timeout(Duration::from_secs(5)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !READY.load(Ordering::Acquire) { continue; }
                let now = elapsed();
                let age = now.saturating_sub(PULSE.load(Ordering::Acquire));
                let phase = PHASE.load(Ordering::Relaxed);
                let hwnd = window(gui);
                if hwnd.is_null() { continue; }
                // A background/minimized app may legitimately stop painting.
                // Probe its message loop, with a one-second upper bound.
                let responsive = if age < 20_000 { true } else {
                    let mut result = 0;
                    unsafe { SendMessageTimeoutW(hwnd, WM_NULL, 0, 0,
                        SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, 1000, &mut result) != 0 }
                };
                if let Some(event) = policy.observe(now, age, responsive) {
                    let _ = writeln!(file, "{}", serde_json::json!({"event":event,"utc":chrono::Utc::now().to_rfc3339(),
                        "last_gui_ms_ago":age,"phase":phase_name(phase),"gui_thread":gui}));
                    let _ = file.sync_data();
                    if event == "gui_unresponsive" {
                        crate::crash_diagnostics::gui_unresponsive(age, phase_name(phase), gui);
                    }
                }
            }
            let _ = writeln!(file, "{}", serde_json::json!({"event":"monitor_stopped","utc":chrono::Utc::now().to_rfc3339()}));
            let _ = file.sync_data();
        });
        Monitor {
            stop: started.ok().map(|_| stop),
        }
    }
}
#[cfg(windows)]
fn window(thread: u32) -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() && GetWindow(hwnd, GW_OWNER).is_null() {
            *(data as *mut HWND) = hwnd;
            return 0;
        }
        1
    }
    let mut hwnd: HWND = std::ptr::null_mut();
    unsafe {
        EnumThreadWindows(thread, Some(collect), &mut hwnd as *mut HWND as isize);
    }
    hwnd
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_probes_require_repeated_failures_and_ignore_idle_painting_and_sleep_gaps() {
        let mut policy = Policy::default();
        for now in [5_000, 10_000, 15_000, 20_000, 25_000] {
            assert_eq!(
                policy.observe(now, 600_000, true),
                None,
                "idle painting is not a hung message loop"
            );
        }
        assert_eq!(policy.observe(30_000, 25_000, false), None);
        assert_eq!(policy.observe(35_000, 30_000, false), None);
        assert_eq!(
            policy.observe(40_000, 35_000, false),
            Some("gui_unresponsive")
        );
        assert_eq!(policy.observe(45_000, 40_000, false), None);
        assert_eq!(policy.observe(50_000, 0, true), Some("gui_recovered"));
        assert_eq!(
            policy.observe(500_000, 450_000, false),
            None,
            "resume needs fresh observations"
        );
        assert_eq!(policy.observe(505_000, 455_000, false), None);
        assert_eq!(policy.observe(510_000, 460_000, false), None);
        assert_eq!(
            policy.observe(515_000, 465_000, false),
            Some("gui_unresponsive")
        );
        assert_eq!(phase_name(4), "renderer or Windows event loop");
    }
}
