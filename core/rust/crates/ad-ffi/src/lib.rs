//! Native bridge for the Flutter desktop client.

mod bridge;
mod file_workspace;
pub use file_workspace::*;
mod input;
mod logging;
mod remote;
pub use remote::*;

pub use bridge::*;

use std::ffi::CStr;
use std::os::raw::c_char;

/// Installs the Rust core's application-log subscriber (JSONL at the given
/// path, level from `AETHERDESK_LOG_LEVEL`, default `info`). Idempotent; the
/// Flutter host calls this once after loading the library (log_feature.md
/// phase 2).
#[no_mangle]
pub extern "C" fn ad_log_init(path: *const c_char) {
    if path.is_null() {
        return;
    }
    let Ok(text) = (unsafe { CStr::from_ptr(path) }).to_str() else {
        return;
    };
    logging::init(std::path::Path::new(text));
}
