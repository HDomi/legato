//! macOS 구현.

mod capture;
mod input;
mod keymap;
pub mod login_agent;
mod vt;

use anyhow::{Result, bail};
use legato_core::codec::EncodedFrame;

pub use capture::{CaptureHandle, list_displays, pixel_buffer_to_i420, request_screen_capture, screen_capture_allowed, start_capture};
pub use input::{Injector, accessibility_trusted, request_accessibility};

use super::{RawFrame, RawImage, VideoEncoder};

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
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .spawn()
        .ok()
}
