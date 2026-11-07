//! 호스트와 뷰어가 주고받는 메시지 정의.
//!
//! 제어 채널: 뷰어가 여는 양방향 스트림 하나. 인증, 입력, 화면 선택 같은 신뢰성 필요한 메시지.
//! 영상 채널: 호스트가 여는 단방향 스트림 하나. 길이 접두 `VideoPacket` 이 이어진다.

use serde::{Deserialize, Serialize};

/// 프로토콜 버전. 호환이 깨지는 변경마다 올린다.
pub const PROTOCOL_VERSION: u32 = 2;

/// 뷰어 → 호스트 메시지.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    /// 접속 직후 첫 메시지.
    Hello { version: u32, client_name: String },
    /// 호스트 챌린지에 대한 응답. 입력한 비밀 하나로 슬롯마다 HMAC 을 계산해 같은 순서로 보낸다.
    AuthResponse { macs: Vec<[u8; 32]> },
    /// 이 디스플레이 영상을 보내 달라는 요청.
    StartStream { display_id: u32 },
    /// 입력 이벤트.
    Input(InputEvent),
    /// 디코더가 망가졌을 때 키프레임 요청.
    RequestKeyframe,
}

/// 호스트 → 뷰어 메시지.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    /// 인증 챌린지. 슬롯마다 salt 가 다르다 (표시 비밀번호, 고정 접속 코드).
    /// 슬롯 종류는 인증 전에는 알려주지 않는다.
    Challenge { slots: Vec<AuthSlot>, nonce: [u8; 32] },
    /// 인증 성공. 호스트 이름과 디스플레이 목록을 알려준다.
    /// `server_mac` 은 호스트도 비밀번호를 안다는 증명 (위조 호스트 차단).
    AuthOk {
        /// 맞은 슬롯 번호. `server_mac` 은 이 슬롯 키로 계산한다.
        slot: u32,
        /// 고정 접속 코드로 들어왔는지.
        via_access_code: bool,
        server_mac: [u8; 32],
        host_name: String,
        displays: Vec<DisplayInfo>,
    },
    /// 인증 실패. 이 메시지 뒤 연결은 닫힌다.
    AuthFailed { reason: String },
    /// 영상 스트림 시작. 해상도는 실제 인코딩 크기.
    StreamStarted {
        display_id: u32,
        width: u32,
        height: u32,
    },
    /// 사용자에게 보여줄 안내 (예: 호스트에 입력 권한이 없음). 연결은 유지된다.
    Notice { message: String },
    /// 복구 불가능한 오류. 이 메시지 뒤 연결은 닫힌다.
    Error { message: String },
}

/// 인증 슬롯 하나 (PBKDF2 매개변수).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AuthSlot {
    pub salt: [u8; 16],
    pub iterations: u32,
}

/// 챌린지에 담을 수 있는 최대 슬롯 수 (뷰어가 PBKDF2 를 과하게 돌지 않게).
pub const MAX_AUTH_SLOTS: usize = 4;

/// 원격 디스플레이 정보.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    /// 논리 해상도 (포인트 단위).
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// 영상 프레임 하나. `data` 는 Annex-B H.264 비트스트림.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoPacket {
    pub seq: u64,
    /// 호스트에서 캡처한 시각 (유닉스 마이크로초).
    pub capture_us: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// 마우스 버튼.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// 입력 이벤트. 좌표는 디스플레이 기준 0.0~1.0 정규화 값이라 해상도·배율과 무관하다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { x: f32, y: f32 },
    MouseButton { button: MouseButton, down: bool },
    /// 스크롤. `pixels` 가 true 면 픽셀 단위(트랙패드), false 면 줄 단위(휠).
    Scroll { dx: f32, dy: f32, pixels: bool },
    /// 키 입력. `code` 는 W3C `KeyboardEvent.code` 이름 (예: "KeyA", "ShiftLeft").
    Key { code: String, down: bool },
    /// 뷰어 창이 포커스를 잃을 때 눌려 있던 키를 모두 뗀다.
    ReleaseAll,
}
