//! H.264 인코더·디코더 (OpenH264 소프트웨어 구현).
//! 하드웨어 코덱(VideoToolbox, Media Foundation)은 이후 단계에서 같은 인터페이스 뒤에 붙인다.

use anyhow::{Context, Result, bail};
use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVSlices, YUVSource};

/// 촘촘하게 붙은 I420 (Y, U, V 평면) 프레임. 가로·세로는 짝수.
#[derive(Clone)]
pub struct I420Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420Frame {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            y: vec![0; width * height],
            u: vec![128; width * height / 4],
            v: vec![128; width * height / 4],
        }
    }

    /// NV12 (Y 평면 + UV 교차 평면) 을 I420 으로 옮긴다. 행 패딩(stride)을 고려한다.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_from_nv12(&mut self, y_src: &[u8], y_stride: usize, uv_src: &[u8], uv_stride: usize) {
        let (w, h) = (self.width, self.height);
        for row in 0..h {
            let s = row * y_stride;
            self.y[row * w..row * w + w].copy_from_slice(&y_src[s..s + w]);
        }
        let cw = w / 2;
        for row in 0..h / 2 {
            let src = &uv_src[row * uv_stride..row * uv_stride + w];
            let ud = &mut self.u[row * cw..row * cw + cw];
            for (i, d) in ud.iter_mut().enumerate() {
                *d = src[i * 2];
            }
            let vd = &mut self.v[row * cw..row * cw + cw];
            for (i, d) in vd.iter_mut().enumerate() {
                *d = src[i * 2 + 1];
            }
        }
    }

    /// BGRA(32비트) 화면을 이 프레임 크기로 줄이면서 I420(BT.709 제한 범위)으로 옮긴다.
    /// 줄일 때는 원본 영역 평균(박스 필터)을 써서 글자가 깨지지 않게 한다. Windows 캡처용.
    pub fn fill_from_bgra_scaled(&mut self, src: &[u8], src_w: usize, src_h: usize, stride: usize) {
        let (w, h) = (self.width, self.height);
        // 출력 픽셀 하나가 덮는 원본 범위 (고정소수 16비트).
        let sx = ((src_w as u64) << 16) / w as u64;
        let sy = ((src_h as u64) << 16) / h as u64;
        let mut rgb_row = vec![[0i32; 3]; w];
        let mut prev_row = vec![[0i32; 3]; w];
        for oy in 0..h {
            let y0 = ((oy as u64 * sy) >> 16) as usize;
            let y1 = ((((oy + 1) as u64 * sy) >> 16) as usize).clamp(y0 + 1, src_h);
            for (ox, out) in rgb_row.iter_mut().enumerate() {
                let x0 = ((ox as u64 * sx) >> 16) as usize;
                let x1 = ((((ox + 1) as u64 * sx) >> 16) as usize).clamp(x0 + 1, src_w);
                let (mut r, mut g, mut b, mut n) = (0i32, 0i32, 0i32, 0i32);
                for yy in y0..y1 {
                    let row = &src[yy * stride..];
                    for xx in x0..x1 {
                        let p = &row[xx * 4..xx * 4 + 4];
                        b += p[0] as i32;
                        g += p[1] as i32;
                        r += p[2] as i32;
                        n += 1;
                    }
                }
                *out = [r / n, g / n, b / n];
            }
            // Y: BT.709 제한 범위 (정수 근사, 계수 ×256).
            for (ox, &[r, g, b]) in rgb_row.iter().enumerate() {
                let yv = ((47 * r + 157 * g + 16 * b + 128) >> 8) + 16;
                self.y[oy * w + ox] = yv.clamp(16, 235) as u8;
            }
            // U, V: 홀수 줄에서 위아래 두 줄 · 좌우 두 칸 평균으로.
            if oy % 2 == 1 {
                let cw = w / 2;
                for cx in 0..cw {
                    let mut acc = [0i32; 3];
                    for p in [prev_row[cx * 2], prev_row[cx * 2 + 1], rgb_row[cx * 2], rgb_row[cx * 2 + 1]] {
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                    }
                    let (r, g, b) = (acc[0] / 4, acc[1] / 4, acc[2] / 4);
                    let u = ((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128;
                    let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
                    let ci = (oy / 2) * cw + cx;
                    self.u[ci] = u.clamp(16, 240) as u8;
                    self.v[ci] = v.clamp(16, 240) as u8;
                }
            }
            std::mem::swap(&mut prev_row, &mut rgb_row);
        }
    }

    /// BT.709 제한 범위로 RGB 로 바꾼다. 디버그 덤프·테스트용이라 느려도 된다.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 3];
        for yy in 0..self.height {
            for xx in 0..self.width {
                let y = self.y[yy * self.width + xx] as f32 - 16.0;
                let ci = (yy / 2) * (self.width / 2) + xx / 2;
                let u = self.u[ci] as f32 - 128.0;
                let v = self.v[ci] as f32 - 128.0;
                let r = 1.1644 * y + 1.7927 * v;
                let g = 1.1644 * y - 0.2132 * u - 0.5329 * v;
                let b = 1.1644 * y + 2.1124 * u;
                let o = (yy * self.width + xx) * 3;
                out[o] = r.clamp(0.0, 255.0) as u8;
                out[o + 1] = g.clamp(0.0, 255.0) as u8;
                out[o + 2] = b.clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    fn as_source(&self) -> YUVSlices<'_> {
        YUVSlices::new(
            (&self.y, &self.u, &self.v),
            (self.width, self.height),
            (self.width, self.width / 2, self.width / 2),
        )
    }
}

/// 인코딩 결과 한 프레임.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub struct H264Encoder {
    inner: Encoder,
    force_key: bool,
}

impl H264Encoder {
    /// 화면 공유용 설정으로 인코더를 만든다. 해상도는 첫 프레임에서 정해진다.
    pub fn new(bitrate_bps: u32, max_fps: f32) -> Result<Self> {
        let cfg = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(max_fps))
            // 유실 대비 안전망: 10초마다 한 번은 키프레임.
            .intra_frame_period(IntraFramePeriod::from_num_frames((max_fps as u32) * 10))
            // 비트레이트 제어가 동작하려면 프레임 건너뛰기를 허용해야 한다 (건너뛴 프레임은 Skip 으로 온다).
            .skip_frames(true)
            // 화면 콘텐츠 모드에서 지원하지 않는 기능이라 명시적으로 끈다 (경고 로그 방지).
            .adaptive_quantization(false)
            .background_detection(false)
            .num_threads(0)
            .vui(VuiConfig::bt709());
        let inner = Encoder::with_api_config(OpenH264API::from_source(), cfg).context("H.264 인코더 생성 실패")?;
        Ok(Self { inner, force_key: true })
    }

    /// 다음 프레임을 키프레임으로 만든다.
    pub fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    /// 한 프레임을 인코딩한다. 인코더가 프레임을 건너뛰면 `None`.
    pub fn encode(&mut self, frame: &I420Frame) -> Result<Option<EncodedFrame>> {
        if self.force_key {
            self.inner.force_intra_frame();
            self.force_key = false;
        }
        let bs = self.inner.encode(&frame.as_source()).context("H.264 인코딩 실패")?;
        let keyframe = match bs.frame_type() {
            FrameType::IDR | FrameType::I => true,
            FrameType::P | FrameType::IPMixed => false,
            FrameType::Skip => return Ok(None),
            FrameType::Invalid => bail!("인코더가 잘못된 프레임을 냄"),
        };
        Ok(Some(EncodedFrame { data: bs.to_vec(), keyframe }))
    }
}

pub struct H264Decoder {
    inner: Decoder,
}

impl H264Decoder {
    pub fn new() -> Result<Self> {
        let inner = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
            .context("H.264 디코더 생성 실패")?;
        Ok(Self { inner })
    }

    /// Annex-B 패킷 하나를 디코딩한다. 아직 낼 그림이 없으면 `None`.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<I420Frame>> {
        let Some(yuv) = self.inner.decode(packet).context("H.264 디코딩 실패")? else {
            return Ok(None);
        };
        let (w, h) = yuv.dimensions();
        let (sy, su, sv) = yuv.strides();
        let mut f = I420Frame::new(w, h);
        for row in 0..h {
            f.y[row * w..row * w + w].copy_from_slice(&yuv.y()[row * sy..row * sy + w]);
        }
        let cw = w / 2;
        for row in 0..h / 2 {
            f.u[row * cw..row * cw + cw].copy_from_slice(&yuv.u()[row * su..row * su + cw]);
            f.v[row * cw..row * cw + cw].copy_from_slice(&yuv.v()[row * sv..row * sv + cw]);
        }
        Ok(Some(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 인코딩_디코딩_왕복() {
        let (w, h) = (320, 240);
        let mut enc = H264Encoder::new(2_000_000, 30.0).unwrap();
        let mut dec = H264Decoder::new().unwrap();
        let mut decoded = 0;
        for i in 0..10 {
            let mut f = I420Frame::new(w, h);
            // 움직이는 그라데이션.
            for (idx, p) in f.y.iter_mut().enumerate() {
                *p = ((idx % w + i * 8) % 220 + 16) as u8;
            }
            if let Some(e) = enc.encode(&f).unwrap() {
                if i == 0 {
                    assert!(e.keyframe, "첫 프레임은 키프레임이어야 함");
                }
                if let Some(out) = dec.decode(&e.data).unwrap() {
                    assert_eq!((out.width, out.height), (w, h));
                    decoded += 1;
                }
            }
        }
        assert!(decoded >= 9, "디코딩된 프레임 수 {decoded}");
    }

    #[test]
    fn bgra_축소_변환() {
        // 8x4 BGRA: 왼쪽 절반 빨강, 오른쪽 절반 파랑 → 4x2 로 줄여도 색 블록(2x2)이 반쪽 안에 있어 섞이지 않는다.
        let (sw, sh) = (8usize, 4usize);
        let mut src = vec![0u8; sw * sh * 4];
        for y in 0..sh {
            for x in 0..sw {
                let p = &mut src[(y * sw + x) * 4..(y * sw + x) * 4 + 4];
                if x < 4 {
                    p.copy_from_slice(&[0, 0, 255, 255]);
                } else {
                    p.copy_from_slice(&[255, 0, 0, 255]);
                }
            }
        }
        let mut f = I420Frame::new(4, 2);
        f.fill_from_bgra_scaled(&src, sw, sh, sw * 4);
        let rgb = f.to_rgb();
        let (l, r) = (&rgb[0..3], &rgb[9..12]);
        assert!(l[0] > 200 && l[2] < 60, "왼쪽은 빨강: {l:?}");
        assert!(r[2] > 200 && r[0] < 60, "오른쪽은 파랑: {r:?}");
        // 흰색은 Y 235 근처.
        let white = vec![255u8; 8 * 8 * 4];
        let mut f = I420Frame::new(4, 4);
        f.fill_from_bgra_scaled(&white, 8, 8, 32);
        assert!(f.y.iter().all(|&v| v >= 233), "{:?}", f.y);
    }

    #[test]
    fn nv12_변환() {
        let mut f = I420Frame::new(4, 2);
        let y: Vec<u8> = (0..16).collect(); // stride 8
        let uv = vec![1, 2, 3, 4, 0, 0, 0, 0]; // stride 8, 한 줄
        f.fill_from_nv12(&y, 8, &uv, 8);
        assert_eq!(f.y, vec![0, 1, 2, 3, 8, 9, 10, 11]);
        assert_eq!(f.u, vec![1, 3]);
        assert_eq!(f.v, vec![2, 4]);
    }
}
