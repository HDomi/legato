//! CGEvent 로 마우스·키보드 입력을 주입한다. "손쉬운 사용" 권한이 있어야 동작한다.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use legato_core::protocol::{InputEvent, MouseButton};

use super::keymap::{modifier_flag, to_mac_keycode};

/// 더블클릭으로 인정하는 간격과 거리.
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DIST: f64 = 4.0;

pub struct Injector {
    display_id: u32,
    pos: CGPoint,
    flags: u64,
    /// 눌린 마우스 버튼 (드래그 이벤트 종류 결정용).
    buttons: [bool; 5],
    /// 눌린 일반 키 (ReleaseAll 처리용).
    pressed_keys: Vec<u16>,
    last_click: Option<(MouseButton, Instant, CGPoint, i64)>,
}

/// 손쉬운 사용 권한이 있는지 확인한다.
pub fn accessibility_trusted() -> bool {
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

impl Injector {
    pub fn new() -> Self {
        let display_id = CGDisplay::main().id;
        let b = CGDisplay::new(display_id).bounds();
        Self {
            display_id,
            pos: CGPoint::new(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0),
            flags: 0,
            buttons: [false; 5],
            pressed_keys: Vec::new(),
            last_click: None,
        }
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| anyhow!("CGEventSource 생성 실패"))
    }

    fn post(&self, ev: CGEvent) {
        ev.set_flags(CGEventFlags::from_bits_truncate(self.flags));
        ev.post(CGEventTapLocation::HID);
    }

    /// 입력 하나를 주입한다. 마우스 좌표는 `display_id` 화면 기준 0~1 값.
    pub fn handle(&mut self, display_id: u32, ev: &InputEvent) -> Result<()> {
        if display_id != 0 {
            self.display_id = display_id;
        }
        match ev {
            InputEvent::MouseMove { x, y } => self.mouse_move(*x, *y),
            InputEvent::MouseButton { button, down } => self.mouse_button(*button, *down),
            InputEvent::Scroll { dx, dy, pixels } => self.scroll(*dx, *dy, *pixels),
            InputEvent::Key { code, down } => self.key(code, *down),
            InputEvent::ReleaseAll => self.release_all(),
            // Ctrl+Alt+Del 은 Windows 전용. macOS 에서는 할 일이 없다.
            InputEvent::SecureAttention => Ok(()),
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> Result<()> {
        // 디스플레이 영역 안으로 자르고 전역 좌표(포인트)로 바꾼다.
        let b = CGDisplay::new(self.display_id).bounds();
        let x = (x.clamp(0.0, 1.0) as f64) * (b.size.width - 1.0) + b.origin.x;
        let y = (y.clamp(0.0, 1.0) as f64) * (b.size.height - 1.0) + b.origin.y;
        self.pos = CGPoint::new(x, y);
        let (ty, btn) = if self.buttons[0] {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if self.buttons[1] {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else if self.buttons[2..].iter().any(|b| *b) {
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        };
        let ev = CGEvent::new_mouse_event(Self::source()?, ty, self.pos, btn)
            .map_err(|_| anyhow!("마우스 이벤트 생성 실패"))?;
        self.post(ev);
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let idx = match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
            MouseButton::Back => 3,
            MouseButton::Forward => 4,
        };
        if self.buttons[idx] == down {
            return Ok(());
        }
        self.buttons[idx] = down;
        let (ty, btn) = match (button, down) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
            (_, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
            (_, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
        };
        // 클릭 횟수: 가까운 위치에서 빠르게 누르면 더블·트리플 클릭으로 센다.
        let clicks = if down {
            let now = Instant::now();
            let n = match self.last_click {
                Some((b, t, p, n))
                    if b == button
                        && now.duration_since(t) < DOUBLE_CLICK_TIME
                        && (p.x - self.pos.x).abs() < DOUBLE_CLICK_DIST
                        && (p.y - self.pos.y).abs() < DOUBLE_CLICK_DIST =>
                {
                    n + 1
                }
                _ => 1,
            };
            self.last_click = Some((button, now, self.pos, n));
            n
        } else {
            self.last_click.map(|(_, _, _, n)| n).unwrap_or(1)
        };
        let ev = CGEvent::new_mouse_event(Self::source()?, ty, self.pos, btn)
            .map_err(|_| anyhow!("마우스 이벤트 생성 실패"))?;
        ev.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, clicks);
        if idx >= 2 {
            // 가운데·앞·뒤 버튼은 버튼 번호 필드로 구분한다.
            ev.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, idx as i64);
        }
        self.post(ev);
        Ok(())
    }

    fn scroll(&mut self, dx: f32, dy: f32, pixels: bool) -> Result<()> {
        let unit = if pixels { ScrollEventUnit::PIXEL } else { ScrollEventUnit::LINE };
        let ev = CGEvent::new_scroll_event(Self::source()?, unit, 2, dy.round() as i32, dx.round() as i32, 0)
            .map_err(|_| anyhow!("스크롤 이벤트 생성 실패"))?;
        if pixels {
            ev.set_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS, 1);
        }
        self.post(ev);
        Ok(())
    }

    fn key(&mut self, code: &str, down: bool) -> Result<()> {
        // 한/영 키: macOS 는 키 하나로 입력 소스를 바꾸지 않으므로 직접 전환한다 (누를 때 한 번).
        if code == "Lang1" {
            if down {
                super::toggle_ime();
            }
            return Ok(());
        }
        let Some(kc) = to_mac_keycode(code) else {
            tracing::debug!("매핑 없는 키: {code}");
            return Ok(());
        };
        if let Some(bit) = modifier_flag(code) {
            if code == "CapsLock" {
                if down {
                    self.flags ^= bit;
                }
            } else if down {
                self.flags |= bit;
            } else {
                self.flags &= !bit;
            }
        } else if down {
            if !self.pressed_keys.contains(&kc) {
                self.pressed_keys.push(kc);
            }
        } else {
            self.pressed_keys.retain(|k| *k != kc);
        }
        let ev = CGEvent::new_keyboard_event(Self::source()?, kc, down).map_err(|_| anyhow!("키 이벤트 생성 실패"))?;
        if modifier_flag(code).is_some() {
            ev.set_type(CGEventType::FlagsChanged);
        }
        self.post(ev);
        Ok(())
    }

    fn release_all(&mut self) -> Result<()> {
        for kc in std::mem::take(&mut self.pressed_keys) {
            let ev =
                CGEvent::new_keyboard_event(Self::source()?, kc, false).map_err(|_| anyhow!("키 이벤트 생성 실패"))?;
            self.post(ev);
        }
        for (idx, b) in
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
                .into_iter()
                .enumerate()
        {
            if self.buttons[idx] {
                self.mouse_button(b, false)?;
            }
        }
        // CapsLock 은 토글 상태라 그대로 두고 나머지 수정자만 뗀다.
        let caps = self.flags & 0x0001_0000;
        if self.flags & !caps != 0 {
            self.flags = caps;
            let ev = CGEvent::new_keyboard_event(Self::source()?, 0x38, false)
                .map_err(|_| anyhow!("키 이벤트 생성 실패"))?;
            ev.set_type(CGEventType::FlagsChanged);
            self.post(ev);
        }
        Ok(())
    }
}

/// 손쉬운 사용 권한을 요청한다. 권한이 없으면 시스템 설정으로 안내하는 창이 뜬다.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringRef};
    unsafe extern "C" {
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    }
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef())
    }
}
