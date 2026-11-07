//! macOS 구현.

mod capture;
mod input;
mod keymap;

pub use capture::{CaptureHandle, list_displays, request_screen_capture, screen_capture_allowed, start_capture};
pub use input::{Injector, accessibility_trusted, request_accessibility};

/// 사용자 활동을 알려 잠든 디스플레이를 깨운다 (원격 접속 직후).
pub fn wake_display() {
    let _ = std::process::Command::new("/usr/bin/caffeinate").args(["-u", "-t", "3"]).spawn();
}

/// 호스트가 살아 있는 동안 시스템 유휴 잠자기를 막는다 (디스플레이는 꺼져도 됨).
/// 잠들면 원격 접속을 받을 수 없어서 필요하다. 반환된 프로세스는 호스트가 끝나면 같이 끝난다.
pub fn prevent_sleep() -> Option<std::process::Child> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .spawn()
        .ok()
}
