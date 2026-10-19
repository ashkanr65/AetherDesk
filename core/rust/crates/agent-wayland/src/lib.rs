#![cfg(target_os = "linux")]

pub mod compositor;
pub mod damage;
mod host_clipboard;
#[cfg(target_os = "linux")]
mod host_desktop;
mod host_windows;
mod terminal_flow_control;
pub mod xwayland;
