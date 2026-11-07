//! macOS 구현.

mod capture;
mod input;
mod keymap;

pub use capture::{CaptureHandle, list_displays, request_screen_capture, screen_capture_allowed, start_capture};
pub use input::{Injector, accessibility_trusted, request_accessibility};
