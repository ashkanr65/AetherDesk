//! Native Windows file picker; only the path is returned, never config secrets.
#[cfg(windows)]
pub(crate) fn wireguard() -> Result<Option<String>, String> {
    use windows_sys::Win32::{
        System::Com::*,
        UI::{Controls::Dialogs::*, WindowsAndMessaging::*},
    };
    unsafe {
        let initialized = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let result = (|| {
            let filter: Vec<u16> = "WireGuard configuration (*.conf)\0*.conf\0All files\0*.*\0\0"
                .encode_utf16()
                .collect();
            let title: Vec<u16> = "Select WireGuard configuration"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut path = vec![0_u16; 32768];
            let mut options: OPENFILENAMEW = std::mem::zeroed();
            options.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            let foreground = GetForegroundWindow();
            let mut process = 0;
            GetWindowThreadProcessId(foreground, &mut process);
            if process == std::process::id() {
                options.hwndOwner = foreground;
            }
            options.lpstrFilter = filter.as_ptr();
            options.lpstrTitle = title.as_ptr();
            options.lpstrFile = path.as_mut_ptr();
            options.nMaxFile = path.len() as u32;
            options.Flags = OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_DONTADDTORECENT;
            if GetOpenFileNameW(&mut options) == 0 {
                let code = CommDlgExtendedError();
                return if code == 0 {
                    Ok(None)
                } else {
                    Err(format!("The file picker could not open (code {code})"))
                };
            }
            let length = path
                .iter()
                .position(|c| *c == 0)
                .ok_or("Invalid file selection")?;
            String::from_utf16(&path[..length])
                .map(Some)
                .map_err(|_| "The file path is not valid Unicode".into())
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}
#[cfg(not(windows))]
pub(crate) fn wireguard() -> Result<Option<String>, String> {
    Err("Native file browsing is available on Windows".into())
}
