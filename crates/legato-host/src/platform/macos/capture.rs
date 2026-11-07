//! ScreenCaptureKit 화면 캡처. NV12(420v) 로 받아 I420 으로 옮긴 뒤 인코더 스레드로 넘긴다.

use anyhow::{Context, Result, anyhow};
use core_graphics::display::CGDisplay;
use legato_core::codec::I420Frame;
use legato_core::protocol::DisplayInfo;
use screencapturekit::cm::{CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::cv::{CVPixelBuffer, CVPixelBufferLockFlags};
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::colors::color_matrix;

use crate::platform::{CaptureOptions, FrameSink, RawFrame, RawImage};

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

/// H.264 인코딩 최대 크기. OpenH264 인코더·디코더 한도(3840x2160)에 맞춘다 (뷰어가 OpenH264 로 디코딩).
const MAX_ENCODE_W: u64 = 3840;
const MAX_ENCODE_H: u64 = 2160;

/// 실제 픽셀 해상도에 최대 폭과 H.264 한도를 적용한 인코딩 크기 (짝수, 비율 유지).
fn encode_size(display_id: u32, max_width: u32) -> (u32, u32) {
    let d = CGDisplay::new(display_id);
    let (pw, ph) = match d.display_mode() {
        Some(m) => (m.pixel_width(), m.pixel_height()),
        None => (d.pixels_wide(), d.pixels_high()),
    };
    fit_size(pw, ph, max_width as u64)
}

/// 비율을 유지하면서 폭·높이 한도 안으로 줄인다.
fn fit_size(pw: u64, ph: u64, max_width: u64) -> (u32, u32) {
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

struct Handler {
    sink: FrameSink,
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
        if pb.plane_count() < 2 || pb.width() < self.width || pb.height() < self.height {
            return;
        }
        // 변환·인코딩은 인코더 스레드에서 한다. 여기서는 버퍼만 넘겨 캡처 콜백을 가볍게 유지한다.
        // 인코더가 바쁘면 이 프레임은 버린다 (지연이 쌓이지 않게).
        self.sink.push(RawFrame { image: RawImage::PixelBuffer(pb), width: self.width, height: self.height, capture_us });
    }
}

/// NV12 CVPixelBuffer 를 I420 으로 옮긴다 (소프트웨어 인코더용). `scratch` 를 재사용한다.
pub fn pixel_buffer_to_i420<'a>(
    pb: &CVPixelBuffer,
    width: usize,
    height: usize,
    scratch: &'a mut Option<I420Frame>,
) -> Result<&'a I420Frame> {
    let guard = pb.lock(CVPixelBufferLockFlags::READ_ONLY).map_err(|e| anyhow!("픽셀 버퍼 잠금 실패 ({e})"))?;
    let (Some(y), Some(uv)) = (unsafe { guard.plane_data(0) }, unsafe { guard.plane_data(1) }) else {
        return Err(anyhow!("픽셀 버퍼 평면을 읽을 수 없음"));
    };
    if scratch.as_ref().is_none_or(|f| f.width != width || f.height != height) {
        *scratch = Some(I420Frame::new(width, height));
    }
    let f = scratch.as_mut().expect("위에서 만듦");
    f.fill_from_nv12(y, guard.bytes_per_row_of_plane(0), uv, guard.bytes_per_row_of_plane(1));
    Ok(f)
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

pub fn start_capture(opts: &CaptureOptions, sink: FrameSink) -> Result<CaptureHandle> {
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
        // 채널(1) + 인코더가 들고 있는 것(1) + 여유. 너무 작으면 SCK 가 프레임을 못 낸다.
        .with_queue_depth(6);

    let mut stream = SCStream::new(&filter, &config).map_err(|e| anyhow!("캡처 스트림 생성 실패: {e:?}"))?;
    stream
        .add_output_handler(Handler { sink, width: width as usize, height: height as usize }, SCStreamOutputType::Screen)
        .map_err(|e| anyhow!("캡처 핸들러 등록 실패: {e:?}"))?;
    stream.start_capture().map_err(|e| anyhow!("캡처 시작 실패 (화면 기록 권한 확인): {e:?}"))?;
    Ok(CaptureHandle { stream, width, height })
}
