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

use windows::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
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

/// 입력 데스크톱이 기본 데스크톱이 아니면(잠금 화면 · 로그인 화면 · UAC) 잠긴 것으로 본다.
/// 일반 권한이면 보안 데스크톱을 열 수 없어 판단할 수 없다 (None).
pub fn screen_locked() -> Option<bool> {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetUserObjectInformationW, OpenInputDesktop,
        UOI_NAME,
    };
    unsafe {
        // DESKTOP_READOBJECTS = 0x0001
        let desk = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0x0001)).ok()?;
        let mut buf = [0u16; 64];
        let mut needed = 0u32;
        let ok = GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        );
        let _ = CloseDesktop(desk);
        ok.ok()?;
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..len]);
        Some(!name.eq_ignore_ascii_case("Default"))
    }
}

/// 잠금 화면을 깨운다. 비밀번호(PIN) 입력은 원격 키보드로 한다.
pub fn wake_for_unlock() {
    wake_display();
}

/// 원격 세션 동안 디스플레이가 꺼지지 않게 막는다. 실행 상태 플래그는 스레드별이라 전용 스레드가 들고 있는다.
pub struct KeepAwake(Option<std::sync::mpsc::Sender<()>>);

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.0.take();
    }
}

pub fn keep_awake() -> Option<KeepAwake> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("legato-keep-awake".into())
        .spawn(move || unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
            // 보내는 쪽이 사라질 때까지 기다린다.
            let _ = rx.recv();
            SetThreadExecutionState(ES_CONTINUOUS);
        })
        .ok()?;
    Some(KeepAwake(Some(tx)))
}
