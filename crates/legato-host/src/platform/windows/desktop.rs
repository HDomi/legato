//! 입력 데스크톱 따라가기. Windows 는 UAC 확인 창·잠금 화면을 별도 "보안 데스크톱" 에 띄운다.
//! SYSTEM 권한으로 실행 중이면 지금 입력을 받는 데스크톱을 열어 이 스레드를 그쪽으로 옮길 수 있다.

use std::cell::Cell;

use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, OpenInputDesktop, SetThreadDesktop,
};

thread_local! {
    /// 이 스레드가 지금 붙어 있는 (우리가 연) 데스크톱 핸들.
    static CURRENT: Cell<Option<HDESK>> = const { Cell::new(None) };
}

/// 입력 데스크톱을 열어 현재 스레드에 붙인다. 실패하면 아무것도 바꾸지 않는다 (일반 사용자 권한 등).
pub fn follow_input_desktop() {
    unsafe {
        // GENERIC_ALL = 0x10000000
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0x1000_0000)) else {
            return;
        };
        // 스레드에 창이 없으면 데스크톱을 옮길 수 있다.
        if SetThreadDesktop(desk).is_ok() {
            // 이전에 열어 둔 핸들은 이제 안 쓰므로 닫는다.
            if let Some(old) = CURRENT.with(|c| c.replace(Some(desk))) {
                let _ = CloseDesktop(old);
            }
        } else {
            let _ = CloseDesktop(desk);
        }
    }
}
