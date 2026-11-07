//! macOS 구현.

mod capture;
pub mod ime;
mod input;
mod keymap;
pub mod login_agent;
pub mod mainloop;
mod vt;

use anyhow::{Result, bail};
use legato_core::codec::EncodedFrame;

pub use capture::{
    CaptureHandle, list_displays, pixel_buffer_to_i420, request_screen_capture, screen_capture_allowed, start_capture,
};
pub use input::{Injector, accessibility_trusted, request_accessibility};

use super::{RawFrame, RawImage, VideoEncoder};

/// 한/영 전환 (메인 스레드에서 실행).
pub fn toggle_ime() {
    mainloop::run_on_main(|| match ime::toggle_korean() {
        Some(id) => tracing::info!("입력 소스 전환: {id}"),
        None => tracing::warn!("입력 소스를 바꾸지 못함 (한국어 입력기가 없을 수 있음)"),
    });
}

/// VideoToolbox 하드웨어 인코더.
pub struct HardwareEncoder(vt::VtEncoder);

impl HardwareEncoder {
    pub fn new(width: usize, height: usize, bitrate: u32, fps: u32) -> Result<Self> {
        Ok(Self(vt::VtEncoder::new(width, height, bitrate, fps)?))
    }
}

impl VideoEncoder for HardwareEncoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>> {
        match &frame.image {
            RawImage::PixelBuffer(pb) => self.0.encode_pixel_buffer(pb.as_ptr(), frame.width, frame.height),
            RawImage::I420(_) => bail!("하드웨어 인코더는 픽셀 버퍼만 받음"),
        }
    }

    fn request_keyframe(&mut self) {
        self.0.request_keyframe();
    }

    fn name(&self) -> &'static str {
        "VideoToolbox (하드웨어)"
    }
}

/// 사용자 활동을 알려 잠든 디스플레이를 깨운다 (원격 접속 직후).
pub fn wake_display() {
    let _ = std::process::Command::new("/usr/bin/caffeinate").args(["-u", "-t", "3"]).spawn();
}

/// 호스트가 살아 있는 동안 시스템 유휴 잠자기를 막는다 (디스플레이는 꺼져도 됨).
/// 잠들면 원격 접속을 받을 수 없어서 필요하다. 반환된 프로세스는 호스트가 끝나면 같이 끝난다.
pub fn prevent_sleep() -> Option<std::process::Child> {
    std::process::Command::new("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).spawn().ok()
}

/// 화면이 잠겨 있는지 (로그인 세션의 잠금 화면). 알 수 없으면 None.
pub fn screen_locked() -> Option<bool> {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return None;
    }
    let dict: CFDictionary<CFString, core_foundation::base::CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(raw) };
    let locked = dict
        .find(CFString::from_static_string("CGSSessionScreenIsLocked"))
        .and_then(|v| v.downcast::<CFBoolean>())
        .map(bool::from)
        .unwrap_or(false);
    Some(locked)
}

/// 잠금 화면을 깨워 비밀번호 입력 칸이 보이게 한다: 사용자 활동을 알리고 Shift 를 한 번 누른다.
pub fn wake_for_unlock() {
    wake_display();
    use core_graphics::event::{CGEvent, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    if let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        for down in [true, false] {
            // kVK_Shift = 0x38: 글자를 입력하지 않고 화면만 깨운다.
            if let Ok(ev) = CGEvent::new_keyboard_event(src.clone(), 0x38, down) {
                ev.post(CGEventTapLocation::HID);
            }
        }
    }
}

/// 원격 세션 동안 디스플레이가 꺼지거나 잠기지 않게 막는다. drop 하면 풀린다.
pub struct KeepAwake(std::process::Child);

impl Drop for KeepAwake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn keep_awake() -> Option<KeepAwake> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-d", "-i", "-w", &std::process::id().to_string()])
        .spawn()
        .ok()
        .map(KeepAwake)
}
