//! Windows 구현.

mod capture;
mod desktop;
mod displays;
mod input;
mod keymap;
pub mod service;

pub use capture::{CaptureHandle, start_capture};
pub use displays::list_displays;
pub use input::Injector;

use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

/// Windows 는 화면 캡처 권한이 따로 없다.
pub fn screen_capture_allowed() -> bool {
    true
}
pub fn request_screen_capture() -> bool {
    true
}
/// 일반 창에는 입력할 수 있다. 관리자 권한 창·보안 데스크톱은 서비스 모드가 필요하다.
pub fn accessibility_trusted() -> bool {
    true
}
pub fn request_accessibility() -> bool {
    true
}

/// 고해상도 모니터에서 좌표가 배율로 왜곡되지 않게 모니터별 DPI 인식을 켠다. 시작할 때 한 번.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 잠든 디스플레이를 깨운다 (원격 접속 직후).
pub fn wake_display() {
    unsafe {
        SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
    }
}

/// 잠자기 방지를 유지하는 동안 들고 있는 값. drop 하면 원래대로.
pub struct SleepGuard;

impl Drop for SleepGuard {
    fn drop(&mut self) {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

/// 호스트가 살아 있는 동안 시스템 유휴 잠자기를 막는다 (부른 스레드 기준, 메인 스레드에서 부른다).
pub fn prevent_sleep() -> Option<SleepGuard> {
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
    }
    Some(SleepGuard)
}
