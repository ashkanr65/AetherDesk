//! Best-effort last-chance metadata, independent of the asynchronous log queue.
//! Never dump process memory, chat/terminal contents, passwords or API keys.
use std::{fs::File, io::Write, path::Path, sync::OnceLock};
static REPORT: OnceLock<File> = OnceLock::new();
pub fn init(directory: &Path) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let _ = std::fs::create_dir_all(directory);
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join(format!("crash-{}.log", std::process::id())))
        {
            let _ = REPORT.set(file);
        }
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(mut file) = REPORT.get() {
                // Payload text can contain user data; record only the source location.
                if let Some(location) = info.location() {
                    let _ = writeln!(
                        file,
                        "Rust panic at {}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    );
                    let _ = file.sync_data();
                }
            }
            original(info);
        }));
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(
                native_exception,
            ));
        }
    });
}
#[cfg(any(windows, test))]
fn native_record(mut output: impl Write, code: u32, address: usize, thread: u32) {
    let _ = writeln!(
        output,
        "Native exception: code=0x{code:08x} address=0x{address:x} thread={thread}"
    );
}
#[cfg(windows)]
unsafe extern "system" fn native_exception(
    pointers: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ENTERED: AtomicBool = AtomicBool::new(false);
    // No recovery, allocation, mutex or async logger in a damaged process.
    if !ENTERED.swap(true, Ordering::Relaxed) && !pointers.is_null() {
        let record = (*pointers).ExceptionRecord;
        if !record.is_null() {
            if let Some(file) = REPORT.get() {
                native_record(
                    file,
                    (*record).ExceptionCode as u32,
                    (*record).ExceptionAddress as usize,
                    windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                );
                let _ = file.sync_data();
            }
        }
    }
    // Preserve Windows Error Reporting. Native failures are never swallowed.
    windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_CONTINUE_SEARCH
}
#[cfg(test)]
mod tests {
    #[test]
    fn native_crash_record_contains_only_exception_metadata() {
        let mut bytes = Vec::new();
        super::native_record(&mut bytes, 0xc0000005, 0x1234, 9);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "Native exception: code=0xc0000005 address=0x1234 thread=9\n"
        );
    }
}
