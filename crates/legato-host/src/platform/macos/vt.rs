//! VideoToolbox 하드웨어 H.264 인코더.
//!
//! 캡처한 CVPixelBuffer(NV12)를 복사 없이 바로 넣는다. 뷰어의 OpenH264 디코더가 읽을 수 있게
//! Constrained Baseline 프로필로 만들고, 출력(AVCC 길이 접두)을 Annex-B(시작 코드)로 바꾼다.
//! 키프레임 앞에는 SPS/PPS 를 붙인다.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Mutex;

use anyhow::{Result, bail};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use legato_core::codec::EncodedFrame;

type OSStatus = i32;
type SessionRef = *mut c_void;
type SampleBufferRef = *mut c_void;
type OutputCallback = extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, SampleBufferRef);

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CM_TIME_VALID: u32 = 1;
const CM_TIME_INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };
/// 'avc1'
const CODEC_H264: u32 = 0x6176_6331;

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCompressionSessionCreate(
        allocator: *const c_void,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: *const c_void,
        output_callback: Option<OutputCallback>,
        output_callback_ref_con: *mut c_void,
        session_out: *mut SessionRef,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: SessionRef, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: SessionRef) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: SessionRef,
        image_buffer: *mut c_void,
        pts: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: SessionRef, complete_until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: SessionRef);

    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel: CFStringRef;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFStringRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetDataBuffer(sb: SampleBufferRef) -> *mut c_void;
    fn CMBlockBufferGetDataLength(bb: *mut c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(bb: *mut c_void, offset: usize, len: usize, dest: *mut c_void) -> OSStatus;
    fn CMSampleBufferGetFormatDescription(sb: SampleBufferRef) -> *mut c_void;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        fd: *mut c_void,
        index: usize,
        out_ptr: *mut *const u8,
        out_size: *mut usize,
        out_count: *mut usize,
        out_nal_header_len: *mut i32,
    ) -> OSStatus;
    fn CMSampleBufferGetSampleAttachmentsArray(sb: SampleBufferRef, create_if_necessary: u8) -> *const c_void;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> u8;
    static kCMSampleAttachmentKey_NotSync: CFStringRef;
}

/// 콜백이 결과를 쌓는 곳. 세션보다 오래 살아야 해서 Box 로 고정한다.
struct Output {
    frames: Mutex<VecDeque<Result<EncodedFrame, OSStatus>>>,
}

pub struct VtEncoder {
    session: SessionRef,
    output: Box<Output>,
    width: usize,
    height: usize,
    frame_no: i64,
    fps: i32,
    force_key: bool,
}

// 세션은 인코더 스레드 하나에서만 쓴다.
unsafe impl Send for VtEncoder {}

extern "C" fn on_output(refcon: *mut c_void, _src: *mut c_void, status: OSStatus, _flags: u32, sb: SampleBufferRef) {
    let out = unsafe { &*(refcon as *const Output) };
    let result = if status != 0 || sb.is_null() { Err(status) } else { unsafe { convert(sb) } };
    out.frames.lock().expect("뮤텍스").push_back(result);
}

/// AVCC 샘플을 Annex-B 로 바꾼다. 키프레임이면 SPS/PPS 를 앞에 붙인다.
unsafe fn convert(sb: SampleBufferRef) -> Result<EncodedFrame, OSStatus> {
    unsafe {
        let keyframe = {
            let arr = CMSampleBufferGetSampleAttachmentsArray(sb, 0);
            if arr.is_null() || CFArrayGetCount(arr) == 0 {
                true
            } else {
                let dict = CFArrayGetValueAtIndex(arr, 0);
                let v = CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync as *const c_void);
                v.is_null() || CFBooleanGetValue(v) == 0
            }
        };
        let mut data = Vec::new();
        if keyframe {
            let fd = CMSampleBufferGetFormatDescription(sb);
            let (mut count, mut idx) = (0usize, 0usize);
            loop {
                let (mut p, mut len, mut hdr) = (std::ptr::null::<u8>(), 0usize, 0i32);
                let st =
                    CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd, idx, &mut p, &mut len, &mut count, &mut hdr);
                if st != 0 {
                    return Err(st);
                }
                data.extend_from_slice(&[0, 0, 0, 1]);
                data.extend_from_slice(std::slice::from_raw_parts(p, len));
                idx += 1;
                if idx >= count {
                    break;
                }
            }
        }
        let bb = CMSampleBufferGetDataBuffer(sb);
        let len = CMBlockBufferGetDataLength(bb);
        let mut avcc = vec![0u8; len];
        let st = CMBlockBufferCopyDataBytes(bb, 0, len, avcc.as_mut_ptr() as *mut c_void);
        if st != 0 {
            return Err(st);
        }
        // 4바이트 길이 접두 NAL 들을 시작 코드로 바꾼다.
        let mut i = 0;
        while i + 4 <= avcc.len() {
            let n = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
            i += 4;
            if i + n > avcc.len() {
                break;
            }
            data.extend_from_slice(&[0, 0, 0, 1]);
            data.extend_from_slice(&avcc[i..i + n]);
            i += n;
        }
        Ok(EncodedFrame { data, keyframe })
    }
}

fn set(session: SessionRef, key: CFStringRef, value: &CFType) -> OSStatus {
    unsafe { VTSessionSetProperty(session, key, value.as_CFTypeRef()) }
}

impl VtEncoder {
    pub fn new(width: usize, height: usize, bitrate: u32, fps: u32) -> Result<Self> {
        let output = Box::new(Output { frames: Mutex::new(VecDeque::new()) });
        let refcon = &*output as *const Output as *mut c_void;
        let mut session: SessionRef = std::ptr::null_mut();
        // 저지연 비트레이트 제어를 먼저 시도하고, 안 되면 기본 설정으로.
        let low_latency = unsafe {
            CFDictionary::from_CFType_pairs(&[(
                CFString::wrap_under_get_rule(kVTVideoEncoderSpecification_EnableLowLatencyRateControl).as_CFType(),
                CFBoolean::true_value().as_CFType(),
            )])
        };
        let mut st = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                width as i32,
                height as i32,
                CODEC_H264,
                low_latency.as_concrete_TypeRef(),
                std::ptr::null(),
                std::ptr::null(),
                Some(on_output),
                refcon,
                &mut session,
            )
        };
        if st != 0 {
            st = unsafe {
                VTCompressionSessionCreate(
                    std::ptr::null(),
                    width as i32,
                    height as i32,
                    CODEC_H264,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    Some(on_output),
                    refcon,
                    &mut session,
                )
            };
        }
        if st != 0 || session.is_null() {
            bail!("VideoToolbox 세션 생성 실패 (OSStatus {st})");
        }
        let enc = Self { session, output, width, height, frame_no: 0, fps: fps.max(1) as i32, force_key: true };
        unsafe {
            let t = CFBoolean::true_value().as_CFType();
            let f = CFBoolean::false_value().as_CFType();
            set(session, kVTCompressionPropertyKey_RealTime, &t);
            set(session, kVTCompressionPropertyKey_AllowFrameReordering, &f);
            let profile = CFString::wrap_under_get_rule(kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel).as_CFType();
            let st = set(session, kVTCompressionPropertyKey_ProfileLevel, &profile);
            if st != 0 {
                // 뷰어 디코더(OpenH264)는 Baseline 만 확실히 읽는다. 프로필을 못 맞추면 쓰지 않는다.
                bail!("Constrained Baseline 프로필 설정 실패 (OSStatus {st})");
            }
            set(session, kVTCompressionPropertyKey_AverageBitRate, &CFNumber::from(bitrate as i64).as_CFType());
            set(session, kVTCompressionPropertyKey_ExpectedFrameRate, &CFNumber::from(fps as i64).as_CFType());
            // 유실 대비 안전망: 10초마다 키프레임.
            set(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, &CFNumber::from((fps * 10) as i64).as_CFType());
            let st = VTCompressionSessionPrepareToEncodeFrames(session);
            if st != 0 {
                bail!("VideoToolbox 준비 실패 (OSStatus {st})");
            }
        }
        Ok(enc)
    }

    pub fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    /// CVPixelBuffer(크기가 세션과 같아야 함)를 인코딩한다.
    pub fn encode_pixel_buffer(
        &mut self,
        pixel_buffer: *mut c_void,
        width: usize,
        height: usize,
    ) -> Result<Option<EncodedFrame>> {
        if width != self.width || height != self.height {
            bail!("프레임 크기가 인코더와 다름 ({width}x{height}, 인코더 {}x{})", self.width, self.height);
        }
        let pts = CMTime { value: self.frame_no, timescale: self.fps, flags: CM_TIME_VALID, epoch: 0 };
        let dur = CMTime { value: 1, timescale: self.fps, flags: CM_TIME_VALID, epoch: 0 };
        self.frame_no += 1;
        let props = if self.force_key {
            self.force_key = false;
            Some(unsafe {
                CFDictionary::from_CFType_pairs(&[(
                    CFString::wrap_under_get_rule(kVTEncodeFrameOptionKey_ForceKeyFrame).as_CFType(),
                    CFBoolean::true_value().as_CFType(),
                )])
            })
        } else {
            None
        };
        let st = unsafe {
            VTCompressionSessionEncodeFrame(
                self.session,
                pixel_buffer,
                pts,
                dur,
                props.as_ref().map_or(std::ptr::null(), |d| d.as_concrete_TypeRef()),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if st != 0 {
            bail!("VideoToolbox 인코딩 실패 (OSStatus {st})");
        }
        // 지연을 줄이려고 프레임마다 바로 꺼낸다.
        unsafe { VTCompressionSessionCompleteFrames(self.session, CM_TIME_INVALID) };
        let mut q = self.output.frames.lock().expect("뮤텍스");
        let mut last: Option<EncodedFrame> = None;
        while let Some(r) = q.pop_front() {
            match r {
                Ok(f) => {
                    // 여러 개가 나왔으면 이어 붙인다 (보통 하나).
                    last = Some(match last {
                        None => f,
                        Some(mut prev) => {
                            let EncodedFrame { data, keyframe } = f;
                            prev.data.extend_from_slice(&data);
                            prev.keyframe |= keyframe;
                            prev
                        }
                    });
                }
                Err(st) => bail!("VideoToolbox 출력 오류 (OSStatus {st})"),
            }
        }
        Ok(last)
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, CM_TIME_INVALID);
            VTCompressionSessionInvalidate(self.session);
            core_foundation::base::CFRelease(self.session as CFTypeRef);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legato_core::codec::H264Decoder;
    use screencapturekit::cv::{CVPixelBuffer, CVPixelBufferLockFlags};

    /// VideoToolbox 로 만든 스트림을 OpenH264 가 읽을 수 있는지 (뷰어 호환성) 확인한다.
    #[test]
    fn 하드웨어_인코딩을_openh264_가_디코딩() {
        let (w, h) = (640usize, 360usize);
        let mut enc = VtEncoder::new(w, h, 2_000_000, 30).expect("VideoToolbox 사용 가능해야 함");
        let mut dec = H264Decoder::new().unwrap();
        let mut decoded = 0;
        for i in 0..20 {
            // '420v' NV12 버퍼를 만들어 움직이는 무늬를 그린다.
            let pb = CVPixelBuffer::create(w, h, 0x3432_3076).expect("픽셀 버퍼 생성");
            {
                let mut g = pb.lock(CVPixelBufferLockFlags::from_bits(0)).unwrap();
                let (ys, uvs) = (g.bytes_per_row_of_plane(0), g.bytes_per_row_of_plane(1));
                let (yh, uvh) = (g.height_of_plane(0), g.height_of_plane(1));
                let y = unsafe { std::slice::from_raw_parts_mut(g.base_address_of_plane_mut(0).unwrap(), ys * yh) };
                for row in 0..h {
                    for col in 0..w {
                        y[row * ys + col] = ((col + row + i * 7) % 200 + 16) as u8;
                    }
                }
                let uv = unsafe { std::slice::from_raw_parts_mut(g.base_address_of_plane_mut(1).unwrap(), uvs * uvh) };
                uv.fill(128);
            }
            let out = enc.encode_pixel_buffer(pb.as_ptr(), w, h).unwrap();
            if let Some(f) = out {
                if i == 0 {
                    assert!(f.keyframe, "첫 프레임은 키프레임");
                }
                if let Some(img) = dec.decode(&f.data).unwrap() {
                    assert_eq!((img.width, img.height), (w, h));
                    decoded += 1;
                }
            }
        }
        assert!(decoded >= 15, "디코딩된 프레임 {decoded}");
    }
}
