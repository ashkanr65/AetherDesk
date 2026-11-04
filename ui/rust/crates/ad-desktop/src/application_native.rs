//! Bind once by a private bootstrap token; captions never identify input targets.
#[derive(Default)]
pub(super) struct NativeWindow {
    binding: Option<(isize, usize)>,
    caption: String,
}
pub(super) fn caption(source: &str) -> String {
    let value: String = source
        .chars()
        .filter(|c| !c.is_control())
        .take(240)
        .collect();
    if value.trim().is_empty() {
        "Application".into()
    } else {
        value
    }
}
#[cfg(windows)]
fn property() -> &'static [u16] {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        "AetherDesk.Application.Viewport"
            .encode_utf16()
            .chain(Some(0))
            .collect()
    })
}
impl NativeWindow {
    pub(super) fn handle(&self) -> Option<isize> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            let (hwnd, tag) = self.binding?;
            let mut process = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd as _, &mut process);
            }
            if process == std::process::id()
                && unsafe { GetPropW(hwnd as _, property().as_ptr()) } as usize == tag
            {
                return Some(hwnd);
            }
        }
        None
    }
    pub(super) fn bind(&mut self, token: &str) {
        if self.handle().is_some() {
            return;
        }
        self.binding = None;
        #[cfg(windows)]
        if let Some(hwnd) = crate::remote_geometry::window(token) {
            let tag = token.bytes().fold(1469598103934665603u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(1099511628211)
            }) as usize
                | 1;
            if unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetPropW(
                    hwnd as _,
                    property().as_ptr(),
                    tag as _,
                )
            } != 0
            {
                self.binding = Some((hwnd, tag));
                self.caption.clear();
            }
        }
    }
    pub(super) fn set_caption(&mut self, value: &str) {
        if self.caption == value {
            return;
        }
        #[cfg(windows)]
        if let Some(hwnd) = self.handle() {
            let text: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
            if unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    hwnd as _,
                    text.as_ptr(),
                )
            } != 0
            {
                self.caption = value.into();
            }
        }
    }
    pub(super) fn place(&self, bounds: crate::remote_geometry::PhysicalRect) -> Result<(), String> {
        if let Some(hwnd) = self.handle() {
            crate::remote_geometry::place(hwnd, bounds, &self.caption)
        } else {
            Ok(())
        }
    }
}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.handle() {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::RemovePropW(
                    hwnd as _,
                    property().as_ptr(),
                );
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    #[test]
    fn duplicate_source_captions_and_renames_keep_distinct_native_targets() {
        let create = |name: &str| {
            let class: Vec<_> = "STATIC".encode_utf16().chain(Some(0)).collect();
            let title: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
            unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            }
        };
        let token1 = format!("application-native-test-{}-one", std::process::id());
        let token2 = format!("application-native-test-{}-two", std::process::id());
        let one = create(&token1);
        let two = create(&token2);
        assert!(!one.is_null() && !two.is_null());
        let mut a = NativeWindow::default();
        let mut b = NativeWindow::default();
        a.bind(&token1);
        b.bind(&token2);
        a.set_caption("Library Manager");
        b.set_caption("Library Manager");
        assert_eq!(a.handle(), Some(one as isize));
        assert_eq!(b.handle(), Some(two as isize));
        a.set_caption("Virtuoso Studio — Layout");
        assert_eq!(a.handle(), Some(one as isize));
        unsafe {
            RemovePropW(one, property().as_ptr());
        }
        assert_eq!(
            a.handle(),
            None,
            "a reused/unbound handle must not acquire input"
        );
        unsafe {
            DestroyWindow(one);
            DestroyWindow(two);
        }
        assert_eq!(b.handle(), None);
        assert_eq!(caption("\n\0"), "Application");
    }
}
