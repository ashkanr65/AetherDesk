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
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        let gui = unsafe { GetCurrentThreadId() };
        let path = directory.join(format!("health-{}.log", std::process::id()));
        let hang_path = directory.join(format!("hang-{}.log", std::process::id()));
        let (stop, receive) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new()
            .name("aetherdesk-health".into())
            .spawn(move || monitor(gui, path, hang_path, receive));
        Monitor {
            stop: started.ok().map(|_| stop),
        }
    }
}
#[cfg(windows)]
fn monitor(
    gui: u32,
    path: std::path::PathBuf,
    hang_path: std::path::PathBuf,
    receive: std::sync::mpsc::Receiver<()>,
) {
    use std::{io::Write, time::Duration};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let kind = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("unins000.exe").is_file()));
    let _ = writeln!(
        file,
        "{}",
        serde_json::json!({"event":"session_started", "utc":chrono::Utc::now().to_rfc3339(),
        "pid":std::process::id(),"gui_thread":gui,"version":crate::update_manifest::current().ok(),
        "package_kind":match kind {Some(true)=>"installed",Some(false)=>"portable",None=>"unknown"}})
    );
    let _ = file.sync_data();
    let mut policy = Policy::default();
    let mut last_record = elapsed();
    loop {
        match receive.recv_timeout(Duration::from_secs(5)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !READY.load(Ordering::Acquire) {
            continue;
        }
        let now = elapsed();
        let age = now.saturating_sub(PULSE.load(Ordering::Acquire));
        let phase = PHASE.load(Ordering::Relaxed);
        let windows = windows(gui);
        if windows.is_empty() {
            continue;
        }
        // A background/minimized app may legitimately stop painting.
        // Probe its message loop, with a one-second upper bound.
        let mut failed_window = None;
        let responsive = if age < 20_000 {
            true
        } else {
            windows.iter().all(|&hwnd| {
                let mut result = 0;
                let ok = unsafe {
                    SendMessageTimeoutW(
                        hwnd as _,
                        WM_NULL,
                        0,
                        0,
                        SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
                        1000,
                        &mut result,
                    ) != 0
                };
                if !ok {
                    failed_window = Some(hwnd);
                }
                ok
            })
        };
        // A session-start line alone cannot distinguish a stopped
        // watchdog from a missing hang report. Keep bounded metadata
        // checkpoints even when no GUI frames are requested.
        if now.saturating_sub(last_record) >= 60_000 {
            if file.metadata().is_ok_and(|meta| meta.len() > 1024 * 1024) {
                let _ = file.set_len(0);
            }
            let _ = writeln!(
                file,
                "{}",
                serde_json::json!({"event":"health_checkpoint","utc":chrono::Utc::now().to_rfc3339(),
                "last_gui_ms_ago":age,"phase":phase_name(phase),"gui_thread":gui,"windows":windows.len(),"responsive":responsive})
            );
            let _ = file.sync_data();
            last_record = now;
        }
        if let Some(event) = policy.observe(now, age, responsive) {
            let _ = writeln!(
                file,
                "{}",
                serde_json::json!({"event":event,"utc":chrono::Utc::now().to_rfc3339(),
                "last_gui_ms_ago":age,"phase":phase_name(phase),"gui_thread":gui,"window":failed_window})
            );
            let _ = file.sync_data();
            if event == "gui_unresponsive" {
                crate::crash_diagnostics::gui_unresponsive(age, phase_name(phase), gui);
                // Keep Win32 diagnostics off the GUI and probe threads.
                // Capture once per incident; no suspended threads,
                // process-memory dump, object names or user contents.
                static EVIDENCE_BUSY: AtomicBool = AtomicBool::new(false);
                let path = hang_path.clone();
                if !EVIDENCE_BUSY.swap(true, Ordering::AcqRel) {
                    let spawned = std::thread::Builder::new()
                        .name("aetherdesk-hang-evidence".into())
                        .spawn(move || {
                            struct Done;
                            impl Drop for Done {
                                fn drop(&mut self) {
                                    EVIDENCE_BUSY.store(false, Ordering::Release);
                                }
                            }
                            let _done = Done;
                            let record = wait_chain(gui);
                            if let Ok(mut output) = std::fs::File::create(path) {
                                let _ = writeln!(output, "{record}");
                                let _ = output.sync_data();
                            }
                        });
                    if spawned.is_err() {
                        EVIDENCE_BUSY.store(false, Ordering::Release);
                    }
                }
            }
        }
    }
    let _ = writeln!(
        file,
        "{}",
        serde_json::json!({"event":"monitor_stopped","utc":chrono::Utc::now().to_rfc3339()})
    );
    let _ = file.sync_data();
}
#[cfg(windows)]
fn windows(thread: u32) -> Vec<usize> {
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() && IsWindowVisible(hwnd) != 0 {
            let windows = &mut *(data as *mut Vec<usize>);
            if windows.len() < 16 {
                windows.push(hwnd as usize);
            }
        }
        1
    }
    let mut windows = Vec::<usize>::new();
    unsafe {
        EnumThreadWindows(
            thread,
            Some(collect),
            &mut windows as *mut Vec<usize> as isize,
        );
    }
    windows
}
#[cfg(windows)]
fn wait_chain(thread: u32) -> serde_json::Value {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_MORE_DATA, ERROR_TOO_MANY_THREADS},
        System::Diagnostics::Debug::*,
    };
    let mut record = serde_json::json!({"event":"gui_wait_chain","utc":chrono::Utc::now().to_rfc3339(),"gui_thread":thread});
    unsafe {
        // Flags zero deliberately stop at another process. No debug privilege
        // is enabled, and no remote process or named lock is inspected.
        let session = OpenThreadWaitChainSession(0, None);
        if session.is_null() {
            record["error"] = GetLastError().into();
            return record;
        }
        let mut count = WCT_MAX_NODE_COUNT;
        let mut nodes: [WAITCHAIN_NODE_INFO; WCT_MAX_NODE_COUNT as usize] = std::mem::zeroed();
        let mut cycle = 0;
        let success = GetThreadWaitChain(
            session,
            0,
            0,
            thread,
            &mut count,
            nodes.as_mut_ptr(),
            &mut cycle,
        );
        let error = if success == 0 { GetLastError() } else { 0 };
        CloseThreadWaitChainSession(session);
        record["error"] = error.into();
        record["cycle"] = (cycle != 0).into();
        if success == 0 && error != ERROR_MORE_DATA && error != ERROR_TOO_MANY_THREADS {
            count = 0;
        }
        record["nodes"] = nodes
            .iter()
            .take((count as usize).min(nodes.len()))
            .map(|node| {
                let mut value =
                    serde_json::json!({"kind":node.ObjectType,"status":node.ObjectStatus});
                if node.ObjectType == WctThreadType {
                    let data = node.Anonymous.ThreadObject;
                    value["pid"] = data.ProcessId.into();
                    value["thread"] = data.ThreadId.into();
                    value["wait_ms"] = data.WaitTime.into();
                    value["context_switches"] = data.ContextSwitches.into();
                }
                value
            })
            .collect::<Vec<_>>()
            .into();
    }
    record
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_health_probes_include_owned_windows_and_collect_only_wait_metadata() {
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*,
        };
        unsafe {
            let create = |owner| {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    "STATIC\0".encode_utf16().collect::<Vec<_>>().as_ptr(),
                    std::ptr::null(),
                    WS_POPUP | WS_VISIBLE,
                    -10000,
                    -10000,
                    1,
                    1,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            let root = create(std::ptr::null_mut());
            assert!(!root.is_null());
            let popup = create(root);
            assert!(!popup.is_null());
            let thread = GetCurrentThreadId();
            let found = windows(thread);
            let has_root = found.contains(&(root as usize));
            let has_popup = found.contains(&(popup as usize));
            DestroyWindow(popup);
            DestroyWindow(root);
            assert!(
                has_root && has_popup,
                "owned application windows must not disappear from hang monitoring"
            );
            let record = wait_chain(thread);
            assert!(record["error"].is_u64());
            if let Some(nodes) = record["nodes"].as_array() {
                assert!(nodes.len() <= 16);
                for node in nodes {
                    assert!(node.as_object().unwrap().keys().all(|key| matches!(
                        key.as_str(),
                        "kind" | "status" | "pid" | "thread" | "wait_ms" | "context_switches"
                    )));
                }
            }
        }
    }
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
