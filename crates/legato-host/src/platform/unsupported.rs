//! 아직 구현하지 않은 OS 용 자리표시자.

use anyhow::{Result, bail};
use legato_core::protocol::{DisplayInfo, InputEvent};

use super::{CaptureOptions, FrameSink};

pub fn screen_capture_allowed() -> bool {
    false
}
pub fn request_screen_capture() -> bool {
    false
}
pub fn accessibility_trusted() -> bool {
    false
}
pub fn request_accessibility() -> bool {
    false
}
pub fn wake_display() {}
pub fn prevent_sleep() -> Option<std::process::Child> {
    None
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    bail!("이 OS 의 호스트 기능은 아직 구현되지 않음")
}

pub struct CaptureHandle {
    pub width: u32,
    pub height: u32,
}

pub fn start_capture(_opts: &CaptureOptions, _sink: FrameSink) -> Result<CaptureHandle> {
    bail!("이 OS 의 화면 캡처는 아직 구현되지 않음")
}

pub struct Injector;

impl Injector {
    pub fn new() -> Self {
        Self
    }
    pub fn handle(&mut self, _display_id: u32, _ev: &InputEvent) -> Result<()> {
        bail!("이 OS 의 입력 주입은 아직 구현되지 않음")
    }
}
