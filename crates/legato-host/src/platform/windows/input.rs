//! SendInput 으로 마우스·키보드 입력을 주입한다.
//! 키는 스캔 코드로 보내 자판 배열과 무관하게 같은 물리 키가 눌린다.
//! 관리자 권한 창(UIPI)·보안 데스크톱에 입력하려면 SYSTEM 서비스 헬퍼로 실행해야 한다.

use anyhow::Result;
use legato_core::protocol::{InputEvent, MouseButton};
use windows::Win32::Foundation::RECT;
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::desktop::follow_input_desktop;
use super::displays;
use super::keymap::to_scancode;

/// 휠 한 칸 (WHEEL_DELTA).
const WHEEL_DELTA: f32 = 120.0;
/// 트랙패드 픽셀 스크롤을 휠 단위로 바꿀 때 한 칸에 해당하는 픽셀 수.
const PIXELS_PER_NOTCH: f32 = 40.0;

pub struct Injector {
    pressed_keys: Vec<(u16, bool)>,
    buttons: [bool; 5],
    /// 픽셀 스크롤의 소수점 누적 (작은 움직임이 사라지지 않게).
    wheel_acc: (f32, f32),
}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn key(scan: u16, extended: bool, down: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: KEYBD_EVENT_FLAGS(flags.0), time: 0, dwExtraInfo: 0 },
        },
    }
}

impl Injector {
    pub fn new() -> Self {
        Self { pressed_keys: Vec::new(), buttons: [false; 5], wheel_acc: (0.0, 0.0) }
    }

    pub fn handle(&mut self, display_id: u32, ev: &InputEvent) -> Result<()> {
        // 보안 데스크톱(UAC·잠금 화면)으로 바뀌었으면 이 스레드도 따라간다 (SYSTEM 일 때만 됨).
        follow_input_desktop();
        match ev {
            InputEvent::MouseMove { x, y } => self.mouse_move(display_id, *x, *y),
            InputEvent::MouseButton { button, down } => self.mouse_button(*button, *down),
            InputEvent::Scroll { dx, dy, pixels } => self.scroll(*dx, *dy, *pixels),
            InputEvent::Key { code, down } => self.key(code, *down),
            InputEvent::ReleaseAll => self.release_all(),
            InputEvent::SecureAttention => {
                // SYSTEM 서비스에서만 동작한다 (일반 권한이면 무시됨).
                unsafe { SendSAS(false) };
                Ok(())
            }
        }
    }

    fn mouse_move(&mut self, display_id: u32, x: f32, y: f32) -> Result<()> {
        let r: RECT = displays::find(display_id)?.rect;
        let px = r.left as f32 + x.clamp(0.0, 1.0) * ((r.right - r.left - 1) as f32);
        let py = r.top as f32 + y.clamp(0.0, 1.0) * ((r.bottom - r.top - 1) as f32);
        // 절대 좌표는 가상 데스크톱 전체를 0~65535 로 본다.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            )
        };
        let ax = ((px - vx as f32) * 65535.0 / (vw - 1) as f32).round() as i32;
        let ay = ((py - vy as f32) * 65535.0 / (vh - 1) as f32).round() as i32;
        send(&[mouse(ax, ay, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)]);
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let (idx, flags, data) = match (button, down) {
            (MouseButton::Left, true) => (0, MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (0, MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (1, MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (1, MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (2, MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (2, MOUSEEVENTF_MIDDLEUP, 0),
            // XBUTTON1 = 뒤로, XBUTTON2 = 앞으로.
            (MouseButton::Back, true) => (3, MOUSEEVENTF_XDOWN, 1),
            (MouseButton::Back, false) => (3, MOUSEEVENTF_XUP, 1),
            (MouseButton::Forward, true) => (4, MOUSEEVENTF_XDOWN, 2),
            (MouseButton::Forward, false) => (4, MOUSEEVENTF_XUP, 2),
        };
        if self.buttons[idx] == down {
            return Ok(());
        }
        self.buttons[idx] = down;
        send(&[mouse(0, 0, data, flags)]);
        Ok(())
    }

    fn scroll(&mut self, dx: f32, dy: f32, pixels: bool) -> Result<()> {
        let (wx, wy) = if pixels {
            self.wheel_acc.0 += dx * WHEEL_DELTA / PIXELS_PER_NOTCH;
            self.wheel_acc.1 += dy * WHEEL_DELTA / PIXELS_PER_NOTCH;
            let (wx, wy) = (self.wheel_acc.0.trunc(), self.wheel_acc.1.trunc());
            self.wheel_acc.0 -= wx;
            self.wheel_acc.1 -= wy;
            (wx as i32, wy as i32)
        } else {
            ((dx * WHEEL_DELTA) as i32, (dy * WHEEL_DELTA) as i32)
        };
        let mut inputs = Vec::new();
        if wy != 0 {
            inputs.push(mouse(0, 0, wy, MOUSEEVENTF_WHEEL));
        }
        if wx != 0 {
            // 뷰어 쪽 양수 dx 는 "내용이 오른쪽으로" = 왼쪽 스크롤. Windows 가로 휠은 양수가 오른쪽.
            inputs.push(mouse(0, 0, -wx, MOUSEEVENTF_HWHEEL));
        }
        if !inputs.is_empty() {
            send(&inputs);
        }
        Ok(())
    }

    fn key(&mut self, code: &str, down: bool) -> Result<()> {
        let Some((scan, ext)) = to_scancode(code) else {
            tracing::debug!("매핑 없는 키: {code}");
            return Ok(());
        };
        if down {
            if !self.pressed_keys.contains(&(scan, ext)) {
                self.pressed_keys.push((scan, ext));
            }
        } else {
            self.pressed_keys.retain(|k| *k != (scan, ext));
        }
        send(&[key(scan, ext, down)]);
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        let keys: Vec<INPUT> = std::mem::take(&mut self.pressed_keys).into_iter().map(|(s, e)| key(s, e, false)).collect();
        if !keys.is_empty() {
            send(&keys);
        }
        for (i, b) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
            .into_iter()
            .enumerate()
        {
            if self.buttons[i] {
                self.mouse_button(b, false)?;
            }
        }
        Ok(())
    }
}
