//! AetherDesk FFI Library for Flutter Integration
//! 
//! This library provides C-compatible functions for the Flutter desktop UI
//! to interact with the Rust core.

use ffi_support::{define_string_destructor, ByteBuffer, FfiStr};
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize the AetherDesk core
#[no_mangle]
pub extern "C" fn ad_initialize() -> i32 {
    INIT.call_once(|| {
        // Only initialize if not already done by the application
        let _ = tracing_subscriber::fmt::try_init();
    });
    
    tracing::info!("AetherDesk core initialized");
    0
}

/// Create a new SSH session
/// Returns a session ID as a string
#[no_mangle]
pub extern "C" fn ad_create_ssh_session(host: FfiStr, port: i32, username: FfiStr) -> *mut c_char {
    let host_str = host.into_string();
    let username_str = username.into_string();
    
    tracing::info!("Creating SSH session: {}@{}:{}", username_str, host_str, port);
    
    // In a real implementation, this would create an actual session
    let session_id = format!("ssh-{}-{}-{}", host_str, port, username_str);
    
    match CString::new(session_id) {
        Ok(c_string) => c_string.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Connect to a session
#[no_mangle]
pub extern "C" fn ad_connect_session(session_id: FfiStr) -> i32 {
    let id = session_id.into_string();
    tracing::info!("Connecting to session: {}", id);
    
    // Simulate connection success
    0
}

/// Send data to a session
#[no_mangle]
pub extern "C" fn ad_send_data(session_id: FfiStr, data: ByteBuffer) -> i32 {
    let id = session_id.into_string();
    let bytes = data.destroy_into_vec();
    
    tracing::debug!("Sending {} bytes to session: {}", bytes.len(), id);
    0
}

/// Receive data from a session
#[no_mangle]
pub extern "C" fn ad_receive_data(session_id: FfiStr) -> ByteBuffer {
    let id = session_id.into_string();
    tracing::debug!("Receiving data from session: {}", id);
    
    // Return empty buffer for now
    ByteBuffer::from_vec(vec![])
}

/// Close a session
#[no_mangle]
pub extern "C" fn ad_close_session(session_id: FfiStr) -> i32 {
    let id = session_id.into_string();
    tracing::info!("Closing session: {}", id);
    0
}

/// Get error message (if any)
#[no_mangle]
pub extern "C" fn ad_get_error() -> *mut c_char {
    std::ptr::null_mut()
}

/// Free a string returned by the library
define_string_destructor!(ad_free_string);

/// Get version information
#[no_mangle]
pub extern "C" fn ad_get_version() -> *mut c_char {
    let version = CString::new(env!("CARGO_PKG_VERSION")).unwrap();
    version.into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_initialize() {
        let result = ad_initialize();
        assert_eq!(result, 0);
    }
    
    #[test]
    fn test_create_ssh_session() {
        use std::ffi::CString;
        let host_cstr = CString::new("localhost").unwrap();
        let username_cstr = CString::new("admin").unwrap();
        let result = ad_create_ssh_session(ffi_support::FfiStr::from_cstr(host_cstr.as_c_str()), 22, ffi_support::FfiStr::from_cstr(username_cstr.as_c_str()));
        assert!(!result.is_null());
        
        unsafe {
            ad_free_string(result);
        }
    }
}
