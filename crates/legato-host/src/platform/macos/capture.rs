//! ScreenCaptureKit 화면 캡처. NV12(420v) 로 받아 I420 으로 옮긴 뒤 인코더 스레드로 넘긴다.

use std::sync::mpsc::SyncSender;

use anyhow::{Context, Result, anyhow};
use core_graphics::display::CGDisplay;
use legato_core::codec::I420Frame;
use legato_core::protocol::DisplayInfo;
use screencapturekit::cm::{CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::colors::color_matrix;

use crate::platform::{CaptureOptions, RawFrame};

/// 화면 기록 권한이 있는지 확인한다 (없어도 묻지 않음).
pub fn screen_capture_allowed() -> bool {
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 화면 기록 권한을 요청한다. 처음 한 번은 시스템 설정 창이 뜬다.
pub fn request_screen_capture() -> bool {
    unsafe extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    let content = SCShareableContent::get().map_err(|e| anyhow!("화면 목록 조회 실패 (화면 기록 권한 확인): {e:?}"))?;
    let main = CGDisplay::main().id;
    Ok(content
        .displays()
        .iter()
        .enumerate()
        .map(|(i, d)| DisplayInfo {
            id: d.display_id(),
            name: format!("디스플레이 {}", i + 1),
            width: d.width(),
            height: d.height(),
            primary: d.display_id() == main,
        })
        .collect())
}

/// 실제 픽셀 해상도에서 최대 폭 제한을 적용한 인코딩 크기 (짝수).
fn encode_size(display_id: u32, max_width: u32) -> (u32, u32) {
    let d = CGDisplay::new(display_id);
    let (pw, ph) = match d.display_mode() {
        Some(m) => (m.pixel_width() as u32, m.pixel_height() as u32),
        None => (d.pixels_wide() as u32, d.pixels_high() as u32),
    };
    let (w, h) = if pw > max_width {
        (max_width, (ph as u64 * max_width as u64 / pw as u64) as u32)
    } else {
        (pw, ph)
    };
    (w & !1, h & !1)
}

struct Handler {
    tx: SyncSender<RawFrame>,
    width: usize,
    height: usize,
}

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }
        // 화면 변화가 없으면 Idle 프레임이 오는데 픽셀이 없다. 건너뛴다.
        if !matches!(sample.frame_status(), Some(SCFrameStatus::Complete) | None) {
            return;
        }
        let capture_us = legato_core::now_micros();
        let Some(pb) = sample.pixel_buffer() else { return };
        let Ok(guard) = pb.lock(CVPixelBufferLockFlags::READ_ONLY) else { return };
        if guard.plane_count() < 2 || guard.width() < self.width || guard.height() < self.height {
            return;
        }
        let (Some(y), Some(uv)) = (unsafe { guard.plane_data(0) }, unsafe { guard.plane_data(1) }) else {
            return;
        };
        let mut frame = I420Frame::new(self.width, self.height);
        frame.fill_from_nv12(y, guard.bytes_per_row_of_plane(0), uv, guard.bytes_per_row_of_plane(1));
        drop(guard);
        // 인코더가 바쁘면 이 프레임은 버린다 (지연이 쌓이지 않게).
        let _ = self.tx.try_send(RawFrame { frame, capture_us });
    }
}

/// 캡처가 살아 있는 동안 들고 있는 핸들. drop 하면 캡처가 멈춘다.
pub struct CaptureHandle {
    stream: SCStream,
    pub width: u32,
    pub height: u32,
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

pub fn start_capture(opts: &CaptureOptions, tx: SyncSender<RawFrame>) -> Result<CaptureHandle> {
    let content = SCShareableContent::get().map_err(|e| anyhow!("화면 목록 조회 실패 (화면 기록 권한 확인): {e:?}"))?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|d| d.display_id() == opts.display_id)
        .or_else(|| displays.first())
        .context("캡처할 디스플레이가 없음")?;
    let (width, height) = encode_size(display.display_id(), opts.max_width);

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build()
        .map_err(|e| anyhow!("캡처 필터 생성 실패: {e:?}"))?;
    let config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_color_matrix(color_matrix::ITU_R_709_2)
        .map_err(|_| anyhow!("색 행렬 설정 실패"))?
        .with_shows_cursor(true)
        .with_minimum_frame_interval(&CMTime::new(1, opts.fps as i32))
        .with_queue_depth(4);

    let mut stream = SCStream::new(&filter, &config).map_err(|e| anyhow!("캡처 스트림 생성 실패: {e:?}"))?;
    stream
        .add_output_handler(Handler { tx, width: width as usize, height: height as usize }, SCStreamOutputType::Screen)
        .map_err(|e| anyhow!("캡처 핸들러 등록 실패: {e:?}"))?;
    stream.start_capture().map_err(|e| anyhow!("캡처 시작 실패 (화면 기록 권한 확인): {e:?}"))?;
    Ok(CaptureHandle { stream, width, height })
}
