use std::ffi::c_void;
use std::ptr;
use std::thread;
use std::time::Duration;

use async_trait::async_trait;

use crate::clipboard::{ClipboardBackend, ClipboardContent, ClipboardError};

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;
const ERROR_ACCESS_DENIED: i32 = 5;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(window: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalFree(memory: *mut c_void) -> *mut c_void;
    fn GlobalLock(memory: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(memory: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

#[derive(Default)]
pub struct WindowsClipboard;

#[async_trait]
impl ClipboardBackend for WindowsClipboard {
    async fn set_content(&self, content: ClipboardContent) -> Result<(), ClipboardError> {
        tokio::task::spawn_blocking(move || set_content(content))
            .await
            .map_err(|error| ClipboardError::Backend(error.to_string()))?
    }
}

fn set_content(content: ClipboardContent) -> Result<(), ClipboardError> {
    let (format, bytes) = match content.mime_type.split(';').next().map(str::trim) {
        Some("text/plain") => {
            let text = String::from_utf8(content.data).map_err(|_| ClipboardError::InvalidText)?;
            let mut utf16 = text.encode_utf16().collect::<Vec<_>>();
            utf16.push(0);
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    utf16.as_ptr().cast::<u8>(),
                    utf16.len() * std::mem::size_of::<u16>(),
                )
            }
            .to_vec();
            (CF_UNICODETEXT, bytes)
        }
        Some("text/html") => (
            register_format("HTML Format")?,
            cf_html(&String::from_utf8(content.data).map_err(|_| ClipboardError::InvalidText)?),
        ),
        Some("image/png") => (register_format("PNG")?, content.data),
        Some("image/bmp") => (register_format("image/bmp")?, content.data),
        _ => return Err(ClipboardError::UnsupportedMime),
    };

    open_with_retry()?;
    let result = unsafe {
        if EmptyClipboard() == 0 {
            Err(last_os_error("EmptyClipboard"))
        } else {
            copy_to_clipboard(format, &bytes)
        }
    };
    unsafe {
        CloseClipboard();
    }
    result
}

fn open_with_retry() -> Result<(), ClipboardError> {
    for _ in 0..10 {
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            return Ok(());
        }
        if unsafe { GetLastError() } as i32 != ERROR_ACCESS_DENIED {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(last_os_error("OpenClipboard"))
}

unsafe fn copy_to_clipboard(format: u32, bytes: &[u8]) -> Result<(), ClipboardError> {
    let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
    if memory.is_null() {
        return Err(last_os_error("GlobalAlloc"));
    }
    let destination = GlobalLock(memory).cast::<u8>();
    if destination.is_null() {
        GlobalFree(memory);
        return Err(last_os_error("GlobalLock"));
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
    GlobalUnlock(memory);
    if SetClipboardData(format, memory).is_null() {
        GlobalFree(memory);
        return Err(last_os_error("SetClipboardData"));
    }
    Ok(())
}

fn register_format(name: &str) -> Result<u32, ClipboardError> {
    let mut wide = name.encode_utf16().collect::<Vec<_>>();
    wide.push(0);
    let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
    if format == 0 {
        Err(last_os_error("RegisterClipboardFormatW"))
    } else {
        Ok(format)
    }
}

fn cf_html(fragment: &str) -> Vec<u8> {
    const PREFIX: &str = "<html><body><!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment--></body></html>";
    let header = |start_html: usize,
                  end_html: usize,
                  start_fragment: usize,
                  end_fragment: usize| {
        format!("Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n")
    };
    let placeholder = header(0, 0, 0, 0);
    let start_html = placeholder.len();
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();
    let header = header(start_html, end_html, start_fragment, end_fragment);
    format!("{header}{PREFIX}{fragment}{SUFFIX}").into_bytes()
}

fn last_os_error(operation: &str) -> ClipboardError {
    ClipboardError::Backend(format!(
        "{operation} failed with Windows error {}",
        unsafe { GetLastError() }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_clipboard_offsets_select_exact_fragment() {
        let value = cf_html("<b>AetherDesk café 🖥</b>");
        let text = std::str::from_utf8(&value).unwrap();
        let offset = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        let start = offset("StartFragment:");
        let end = offset("EndFragment:");
        assert_eq!(&text[start..end], "<b>AetherDesk café 🖥</b>");
    }
}
