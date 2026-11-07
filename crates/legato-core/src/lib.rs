//! Legato 공통 코어: 프로토콜 정의, QUIC 전송, 인증, 코덱, 설정 파일 경로.

pub mod auth;
pub mod codec;
pub mod framing;
pub mod identity;
pub mod known_hosts;
pub mod paths;
pub mod protocol;
pub mod transport;

/// 기본 UDP 포트. 원격받는 쪽은 이 포트 하나만 열어두면 된다.
pub const DEFAULT_PORT: u16 = 47820;

/// QUIC ALPN 식별자. 프로토콜 버전이 바뀌면 함께 올린다.
pub const ALPN: &[u8] = b"legato/1";

/// 지금 시각을 유닉스 마이크로초로 돌려준다. 캡처 시각 기록과 지연 측정에 쓴다.
pub fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}
