//! OS 별 화면 캡처·입력 주입·인코더. 공통 타입은 여기 두고 구현은 OS 모듈에 둔다.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use anyhow::Result;
use legato_core::codec::{EncodedFrame, H264Encoder, I420Frame};

/// 캡처 설정. 일부 필드는 OS 구현에서만 읽어서, 구현이 없는 OS 에서는 미사용 경고를 끈다.
#[derive(Debug, Clone)]
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub struct CaptureOptions {
    pub display_id: u32,
    /// 인코딩 최대 폭 (픽셀). 원본이 더 크면 비율을 유지해 줄인다.
    pub max_width: u32,
    pub fps: u32,
}

/// 캡처된 원본 이미지. OS 가 준 버퍼를 그대로 들고 있다가 인코더가 필요한 형식으로 쓴다.
pub enum RawImage {
    /// 이미 I420 으로 옮긴 이미지 (Windows 캡처).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    I420(I420Frame),
    /// macOS: ScreenCaptureKit 이 준 NV12 CVPixelBuffer (하드웨어 인코더에 복사 없이 넣는다).
    #[cfg(target_os = "macos")]
    PixelBuffer(screencapturekit::cv::CVPixelBuffer),
}

/// 캡처된 프레임 하나.
pub struct RawFrame {
    pub image: RawImage,
    pub width: usize,
    pub height: usize,
    pub capture_us: u64,
}

/// 캡처 → 인코더 전달 통로. 인코더가 바쁘면 프레임을 버리고 개수를 센다 (자동 화질 조절 신호).
#[derive(Clone)]
pub struct FrameSink {
    tx: SyncSender<RawFrame>,
    pub dropped: Arc<AtomicU32>,
}

impl FrameSink {
    pub fn new(tx: SyncSender<RawFrame>) -> Self {
        Self { tx, dropped: Arc::new(AtomicU32::new(0)) }
    }

    pub fn push(&self, frame: RawFrame) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(frame) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 인코더 공통 인터페이스.
pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>>;
    fn request_keyframe(&mut self);
    fn name(&self) -> &'static str;
}

/// OpenH264 소프트웨어 인코더. 하드웨어 인코더를 못 쓸 때의 대비책.
pub struct SoftwareEncoder {
    inner: H264Encoder,
    scratch: Option<I420Frame>,
}

impl SoftwareEncoder {
    pub fn new(bitrate: u32, fps: u32) -> Result<Self> {
        Ok(Self { inner: H264Encoder::new(bitrate, fps as f32)?, scratch: None })
    }
}

impl VideoEncoder for SoftwareEncoder {
    fn encode(&mut self, frame: &RawFrame) -> Result<Option<EncodedFrame>> {
        match &frame.image {
            RawImage::I420(f) => self.inner.encode(f),
            #[cfg(target_os = "macos")]
            RawImage::PixelBuffer(pb) => {
                let f = macos::pixel_buffer_to_i420(pb, frame.width, frame.height, &mut self.scratch)?;
                self.inner.encode(f)
            }
        }
    }

    fn request_keyframe(&mut self) {
        self.inner.request_keyframe();
    }

    fn name(&self) -> &'static str {
        "OpenH264 (소프트웨어)"
    }
}

/// H.264 인코딩 최대 크기. OpenH264 인코더·디코더 한도(3840x2160)에 맞춘다 (뷰어가 OpenH264 로 디코딩).
const MAX_ENCODE_W: u64 = 3840;
const MAX_ENCODE_H: u64 = 2160;

/// 비율을 유지하면서 폭·높이 한도 안으로 줄인다.
pub fn fit_size(pw: u64, ph: u64, max_width: u64) -> (u32, u32) {
    let (pw, ph) = (pw.max(2), ph.max(2));
    let max_w = max_width.min(MAX_ENCODE_W);
    // 가로·세로 중 더 많이 줄여야 하는 쪽 비율을 쓴다 (분수 비교로 반올림 오차 없이).
    let (num, den) = if pw * MAX_ENCODE_H > ph * max_w {
        if pw > max_w { (max_w, pw) } else { (1, 1) }
    } else if ph > MAX_ENCODE_H {
        (MAX_ENCODE_H, ph)
    } else {
        (1, 1)
    };
    let (w, h) = (pw * num / den, ph * num / den);
    ((w as u32) & !1, (h as u32) & !1)
}

#[cfg(test)]
mod tests {
    use super::fit_size;

    #[test]
    fn 인코딩_크기_한도() {
        assert_eq!(fit_size(3420, 2224, 3840), (3320, 2160), "높이 한도로 줄임");
        assert_eq!(fit_size(3420, 2224, 1920), (1920, 1248));
        assert_eq!(fit_size(5120, 2880, 3840), (3840, 2160));
        assert_eq!(fit_size(1710, 1112, 3840), (1710, 1112));
        assert_eq!(fit_size(1080, 1920, 3840), (1080, 1920), "세로 모니터는 그대로");
        assert_eq!(fit_size(2160, 3840, 3840), (1214, 2160), "세로로 긴 화면은 높이 한도");
    }
}

/// 이 OS 에서 쓸 수 있는 가장 좋은 인코더를 만든다. 하드웨어가 안 되면 소프트웨어로.
pub fn create_encoder(width: usize, height: usize, bitrate: u32, fps: u32) -> Result<Box<dyn VideoEncoder>> {
    #[cfg(target_os = "macos")]
    {
        if std::env::var_os("LEGATO_SOFTWARE_ENCODER").is_none() {
            match macos::HardwareEncoder::new(width, height, bitrate, fps) {
                Ok(e) => return Ok(Box::new(e)),
                Err(e) => tracing::warn!("하드웨어 인코더를 못 써서 소프트웨어로 바꿈: {e:#}"),
            }
        }
    }
    let _ = (width, height);
    Ok(Box::new(SoftwareEncoder::new(bitrate, fps)?))
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    CaptureHandle, Injector, accessibility_trusted, list_displays, prevent_sleep, request_accessibility,
    request_screen_capture, screen_capture_allowed, start_capture, wake_display,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::service;
#[cfg(windows)]
pub use self::windows::{
    CaptureHandle, Injector, accessibility_trusted, init_process, list_displays, prevent_sleep, request_accessibility,
    request_screen_capture, screen_capture_allowed, start_capture, wake_display,
};

#[cfg(not(any(target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "macos", windows)))]
pub use unsupported::*;

/// OS 별 시작 준비 (Windows: DPI 인식). 다른 OS 는 할 일 없음.
#[cfg(not(windows))]
pub fn init_process() {}
