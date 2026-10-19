//! Windows Credential Manager; keys never go into JSON settings or chat history.
#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    #[repr(C)]
    struct Credential {
        flags: u32,
        kind: u32,
        target: *mut u16,
        comment: *mut u16,
        time: [u32; 2],
        size: u32,
        blob: *mut u8,
        persist: u32,
        count: u32,
        attributes: *mut c_void,
        alias: *mut u16,
        username: *mut u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredReadW(
            target: *const u16,
            kind: u32,
            flags: u32,
            result: *mut *mut Credential,
        ) -> i32;
        fn CredWriteW(credential: *const Credential, flags: u32) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }
    fn target(id: &str) -> Result<Vec<u16>, String> {
        if id.is_empty() || id.len() > 200 || id.contains('\0') {
            return Err("Invalid credential identifier".into());
        }
        Ok(format!("AetherDesk-Rust/provider/{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect())
    }
    fn legacy_target(target: &str) -> Result<Vec<u16>, String> {
        if target.is_empty() || target.len() > 300 || target.contains('\0') {
            return Err("Invalid legacy credential identifier".into());
        }
        Ok(target.encode_utf16().chain(Some(0)).collect())
    }
    struct Owned(*mut Credential);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                CredFree(self.0.cast());
            }
        }
    }
    pub fn read(id: &str) -> Result<Option<String>, String> {
        let name = target(id)?;
        let mut pointer = std::ptr::null_mut();
        if unsafe { CredReadW(name.as_ptr(), 1, 0, &mut pointer) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1168) {
                return Ok(None);
            }
            return Err(format!(
                "Credential Manager could not read this key ({})",
                error.raw_os_error().unwrap_or(0)
            ));
        }
        let owned = Owned(pointer);
        let credential = unsafe { &*owned.0 };
        if credential.size > 5120 || credential.blob.is_null() {
            return Err("Invalid stored API key".into());
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(credential.blob, credential.size as usize) };
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|_| "Invalid stored API key encoding".into())
    }
    pub fn read_legacy(target: &str) -> Result<Option<(String, Vec<u8>)>, String> {
        let name = legacy_target(target)?;
        let mut pointer = std::ptr::null_mut();
        if unsafe { CredReadW(name.as_ptr(), 1, 0, &mut pointer) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1168) {
                return Ok(None);
            }
            return Err(format!(
                "Windows could not read an existing credential ({})",
                error.raw_os_error().unwrap_or(0)
            ));
        }
        let owned = Owned(pointer);
        let credential = unsafe { &*owned.0 };
        if credential.size > 16_384 || credential.blob.is_null() {
            return Err("Invalid legacy credential record".into());
        }
        let username = if credential.username.is_null() {
            String::new()
        } else {
            let mut len = 0;
            unsafe {
                while *credential.username.add(len) != 0 && len < 1024 {
                    len += 1;
                }
            };
            String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(credential.username, len)
            })
        };
        let blob = unsafe { std::slice::from_raw_parts(credential.blob, credential.size as usize) }
            .to_vec();
        Ok(Some((username, blob)))
    }
    pub fn write(id: &str, key: &str) -> Result<(), String> {
        if key.len() > 5120 || key.contains(['\r', '\n', '\0']) {
            return Err("API key is invalid or too long".into());
        }
        if key.is_empty() {
            return delete(id);
        }
        let mut name = target(id)?;
        let mut username: Vec<u16> = "AetherDesk".encode_utf16().chain(Some(0)).collect();
        let credential = Credential {
            flags: 0,
            kind: 1,
            target: name.as_mut_ptr(),
            comment: std::ptr::null_mut(),
            time: [0; 2],
            size: key.len() as u32,
            blob: key.as_ptr() as *mut u8,
            persist: 2,
            count: 0,
            attributes: std::ptr::null_mut(),
            alias: std::ptr::null_mut(),
            username: username.as_mut_ptr(),
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err("Windows Credential Manager could not store the API key".into());
        }
        Ok(())
    }
    pub fn delete(id: &str) -> Result<(), String> {
        let name = target(id)?;
        if unsafe { CredDeleteW(name.as_ptr(), 1, 0) } == 0
            && std::io::Error::last_os_error().raw_os_error() != Some(1168)
        {
            return Err("Windows Credential Manager could not remove this API key".into());
        }
        Ok(())
    }
}
#[cfg(not(windows))]
mod platform {
    pub fn read(_: &str) -> Result<Option<String>, String> {
        Err("Credential storage requires Windows".into())
    }
    pub fn write(_: &str, _: &str) -> Result<(), String> {
        Err("Credential storage requires Windows".into())
    }
    pub fn delete(_: &str) -> Result<(), String> {
        Err("Credential storage requires Windows".into())
    }
    pub fn read_legacy(_: &str) -> Result<Option<(String, Vec<u8>)>, String> {
        Err("Credential migration requires Windows".into())
    }
}
pub use platform::{delete, read, read_legacy, write};
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn native_credentials_round_trip_delete_and_remain_separate_from_other_targets() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = delete(&self.0);
            }
        }
        let first = Cleanup(format!("test-{}", uuid::Uuid::new_v4()));
        let second = Cleanup(format!("test-{}", uuid::Uuid::new_v4()));
        assert_eq!(read(&first.0).unwrap(), None);
        write(&first.0, "fixture-key-one").unwrap();
        write(&second.0, "fixture-key-two").unwrap();
        assert_eq!(read(&first.0).unwrap().as_deref(), Some("fixture-key-one"));
        delete(&first.0).unwrap();
        delete(&first.0).unwrap();
        assert_eq!(read(&first.0).unwrap(), None);
        assert_eq!(read(&second.0).unwrap().as_deref(), Some("fixture-key-two"));
        assert!(write("invalid\0id", "fixture-key").is_err());
    }
}
