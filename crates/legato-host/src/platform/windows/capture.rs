//! DXGI Desktop Duplication 화면 캡처.
//!
//! 보안 데스크톱(UAC 확인 창, 잠금·로그인 화면)으로 바뀌면 duplication 이 끊기므로(ACCESS_LOST)
//! 지금 입력을 받는 데스크톱으로 스레드를 옮긴 뒤 다시 만든다. 보안 데스크톱은 SYSTEM 권한
//! 서비스가 띄운 헬퍼에서만 열린다 (일반 사용자 권한이면 열기에 실패하고 기존 화면을 계속 잡는다).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use legato_core::codec::I420Frame;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGIResource,
};
use windows::core::Interface;

use super::desktop::follow_input_desktop;
use super::displays;
use crate::platform::{CaptureOptions, FrameSink, RawFrame, RawImage, fit_size};

/// 캡처가 살아 있는 동안 들고 있는 핸들. drop 하면 캡처 스레드가 멈춘다.
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub width: u32,
    pub height: u32,
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Dup {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
}

fn open_duplication(display_id: u32) -> Result<Dup> {
    let out = displays::find(display_id)?;
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            &out.adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .context("D3D11 장치 생성 실패")?;
    let device = device.context("D3D11 장치 없음")?;
    let context = context.context("D3D11 컨텍스트 없음")?;
    let output1: IDXGIOutput1 = out.output.cast().context("IDXGIOutput1 아님")?;
    let dup = unsafe { output1.DuplicateOutput(&device) }.context("화면 복제 시작 실패")?;
    Ok(Dup { device, context, dup, staging: None })
}

pub fn start_capture(opts: &CaptureOptions, sink: FrameSink) -> Result<CaptureHandle> {
    let out = displays::find(opts.display_id)?;
    let (sw, sh) = ((out.rect.right - out.rect.left) as u64, (out.rect.bottom - out.rect.top) as u64);
    let (width, height) = fit_size(sw, sh, opts.max_width as u64);
    // 시작할 때 한 번 열어 보고 실패하면 바로 알린다 (권한·드라이버 문제).
    follow_input_desktop();
    drop(open_duplication(opts.display_id)?);

    let stop = Arc::new(AtomicBool::new(false));
    let (display_id, fps) = (opts.display_id, opts.fps.max(1));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name(format!("legato-capture-{display_id}"))
        .spawn(move || capture_loop(display_id, fps, width as usize, height as usize, sink, stop2))?;
    Ok(CaptureHandle { stop, thread: Some(thread), width, height })
}

fn capture_loop(display_id: u32, fps: u32, width: usize, height: usize, sink: FrameSink, stop: Arc<AtomicBool>) {
    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut last_sent = Instant::now() - interval;
    let mut frame = I420Frame::new(width, height);
    while !stop.load(Ordering::SeqCst) {
        // 데스크톱이 바뀌었으면(UAC·잠금 화면) 따라간 뒤 duplication 을 다시 만든다.
        follow_input_desktop();
        let mut d = match open_duplication(display_id) {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!("화면 복제 다시 열기 실패: {e:#}");
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        };
        while !stop.load(Ordering::SeqCst) {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match unsafe { d.dup.AcquireNextFrame(100, &mut info, &mut res) } {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => break,
                Err(e) => {
                    tracing::debug!("프레임 받기 실패: {e}");
                    break;
                }
            }
            // 화면이 안 바뀌고 마우스만 움직였거나, 초당 프레임 상한보다 빠르면 건너뛴다.
            let due = last_sent.elapsed() >= interval;
            if info.LastPresentTime == 0 || !due {
                let _ = unsafe { d.dup.ReleaseFrame() };
                continue;
            }
            let capture_us = legato_core::now_micros();
            let ok = res
                .and_then(|r| r.cast::<ID3D11Texture2D>().ok())
                .is_some_and(|tex| copy_frame(&mut d, &tex, &mut frame));
            let _ = unsafe { d.dup.ReleaseFrame() };
            if ok {
                last_sent = Instant::now();
                sink.push(RawFrame { image: RawImage::I420(frame.clone()), width, height, capture_us });
            }
        }
    }
}

/// GPU 텍스처를 CPU 로 읽어 줄이면서 I420 으로 옮긴다.
fn copy_frame(d: &mut Dup, tex: &ID3D11Texture2D, frame: &mut I420Frame) -> bool {
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        let need_new = match &d.staging {
            None => true,
            Some(s) => {
                let mut sd = D3D11_TEXTURE2D_DESC::default();
                s.GetDesc(&mut sd);
                sd.Width != desc.Width || sd.Height != desc.Height || sd.Format != desc.Format
            }
        };
        if need_new {
            let mut sd = desc;
            sd.Usage = D3D11_USAGE_STAGING;
            sd.BindFlags = 0;
            sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            sd.MiscFlags = 0;
            sd.MipLevels = 1;
            sd.ArraySize = 1;
            let mut staging = None;
            if d.device.CreateTexture2D(&sd, None, Some(&mut staging)).is_err() {
                return false;
            }
            d.staging = staging;
        }
        let Some(staging) = d.staging.clone() else {
            return false;
        };
        d.context.CopyResource(&staging, tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if d.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).is_err() {
            return false;
        }
        let (sw, sh, stride) = (desc.Width as usize, desc.Height as usize, mapped.RowPitch as usize);
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, stride * sh);
        frame.fill_from_bgra_scaled(src, sw, sh, stride);
        d.context.Unmap(&staging, 0);
        true
    }
}
