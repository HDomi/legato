//! OS 별 화면 캡처·입력 주입. 공통 타입은 여기 두고 구현은 OS 모듈에 둔다.

use legato_core::codec::I420Frame;

/// 캡처 설정.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub display_id: u32,
    /// 인코딩 최대 폭 (픽셀). 원본이 더 크면 비율을 유지해 줄인다.
    pub max_width: u32,
    pub fps: u32,
}

/// 캡처된 원본 프레임.
pub struct RawFrame {
    pub frame: I420Frame,
    pub capture_us: u64,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::*;
