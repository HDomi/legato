//! 영상 수신과 디코딩. 네트워크 작업은 tokio 에서, 디코딩은 디스플레이마다 전용 스레드에서 돈다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use legato_core::codec::{H264Decoder, I420Frame};
use legato_core::framing::{read_msg, read_msg_limited, read_tag};
use legato_core::protocol::{ClientMsg, MAX_CLIPBOARD_BYTES, ServerMsg, UNI_CLIPBOARD, UNI_VIDEO, VideoPacket};
use tokio::sync::mpsc::UnboundedSender;

/// 디코딩 대기열이 이보다 길면 따라잡기 모드: 키프레임이 올 때까지 버리고 키프레임을 요청한다.
const MAX_BACKLOG: usize = 15;

/// 디스플레이 하나의 디코딩 입구.
#[derive(Clone)]
pub struct DecodeInput {
    pub tx: mpsc::Sender<VideoPacket>,
    pub backlog: Arc<AtomicUsize>,
    pub stats: Arc<DecodeStats>,
}

/// 디스플레이 번호 → 디코딩 입구. 창을 열고 닫을 때 바뀐다.
pub type Routes = Arc<Mutex<HashMap<u32, DecodeInput>>>;

/// 받아들이는 최대 화면 크기. 이보다 크면 GPU 텍스처를 만들 수 없거나 악의적인 값이다.
const MAX_DIM: u32 = 8192;

/// 디스플레이 하나의 1초 통계 (메뉴 표시용).
#[derive(Default)]
pub struct DecodeStats {
    pub frames: AtomicUsize,
    /// 지난 구간 가장 오래 걸린 디코딩 (마이크로초).
    pub max_decode_us: AtomicUsize,
}

/// 호스트가 여는 단방향 스트림을 계속 받는다. 첫 바이트로 종류를 가린다.
/// - 영상: 디스플레이별 디코딩 스레드로 넘긴다.
/// - 클립보드: `on_clipboard` 로 넘긴다 (크기 제한 확인 후).
pub fn spawn_video_reader(conn: quinn::Connection, routes: Routes, on_clipboard: Arc<dyn Fn(String) + Send + Sync>) {
    tokio::spawn(async move {
        while let Ok(mut uni) = conn.accept_uni().await {
            let routes = routes.clone();
            let on_clipboard = on_clipboard.clone();
            tokio::spawn(async move {
                match read_tag(&mut uni).await {
                    Ok(UNI_VIDEO) => {}
                    Ok(UNI_CLIPBOARD) => {
                        if let Ok(Some(ServerMsg::Clipboard { text })) =
                            read_msg_limited::<_, ServerMsg>(&mut uni, MAX_CLIPBOARD_BYTES + 64).await
                            && text.len() <= MAX_CLIPBOARD_BYTES
                        {
                            on_clipboard(text);
                        }
                        return;
                    }
                    _ => return,
                }
                loop {
                    match read_msg::<_, VideoPacket>(&mut uni).await {
                        Ok(Some(pkt)) => {
                            if pkt.width > MAX_DIM || pkt.height > MAX_DIM {
                                tracing::warn!("너무 큰 프레임 ({}x{}) 무시", pkt.width, pkt.height);
                                continue;
                            }
                            let input = routes.lock().expect("뮤텍스").get(&pkt.display_id).cloned();
                            // 닫은 디스플레이의 늦은 패킷은 버린다.
                            if let Some(input) = input {
                                input.backlog.fetch_add(1, Ordering::SeqCst);
                                let _ = input.tx.send(pkt);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!("영상 스트림 읽기 끝: {e:#}");
                            break;
                        }
                    }
                }
            });
        }
    });
}

/// 디스플레이 하나의 디코딩 스레드를 띄우고 입구를 돌려준다.
/// 프레임이 나올 때마다 `on_frame(프레임, 캡처시각)` 을 부른다. 입구가 모두 drop 되면 스레드가 끝난다.
/// 패킷을 처리할 때마다(디코딩·버림 모두) 확인 응답을 보내 호스트가 쌓인 지연을 알 수 있게 한다.
pub fn spawn_decoder(
    display_id: u32,
    ctl: UnboundedSender<ClientMsg>,
    mut on_frame: impl FnMut(I420Frame, u64) + Send + 'static,
) -> DecodeInput {
    let (tx, rx) = mpsc::channel::<VideoPacket>();
    let backlog = Arc::new(AtomicUsize::new(0));
    let stats = Arc::new(DecodeStats::default());
    let (bl, st) = (backlog.clone(), stats.clone());
    std::thread::Builder::new()
        .name(format!("legato-decoder-{display_id}"))
        .spawn(move || {
            let mut dec = match H264Decoder::new() {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("{e:#}");
                    return;
                }
            };
            // 진단용: 느린 뷰어를 흉내 낸다 (지연 제한 동작 확인).
            let slow = std::env::var("LEGATO_DEBUG_DECODE_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok());
            // 키프레임을 기다리는 중인지 (첫 프레임, 디코딩 오류, 대기열 폭주 후).
            let mut waiting_key = true;
            let mut requested = false;
            while let Ok(pkt) = rx.recv() {
                let ack = ClientMsg::FrameAck { display_id, stream: pkt.stream, seq: pkt.seq };
                let queued = bl.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                if queued > MAX_BACKLOG && !waiting_key {
                    tracing::warn!("디스플레이 {display_id} 디코딩이 밀림 (대기 {queued}), 키프레임부터 다시 시작");
                    waiting_key = true;
                    requested = false;
                }
                if waiting_key {
                    if !pkt.keyframe {
                        if !requested {
                            let _ = ctl.send(ClientMsg::RequestKeyframe { display_id });
                            requested = true;
                        }
                        let _ = ctl.send(ack);
                        continue;
                    }
                    waiting_key = false;
                }
                let t = std::time::Instant::now();
                if let Some(ms) = slow {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
                match dec.decode(&pkt.data) {
                    Ok(Some(frame)) if frame.width as u32 <= MAX_DIM && frame.height as u32 <= MAX_DIM => {
                        st.max_decode_us.fetch_max(t.elapsed().as_micros() as usize, Ordering::Relaxed);
                        st.frames.fetch_add(1, Ordering::Relaxed);
                        on_frame(frame, pkt.capture_us)
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("디스플레이 {display_id} 디코딩 오류, 키프레임 요청: {e:#}");
                        waiting_key = true;
                        requested = false;
                    }
                }
                let _ = ctl.send(ack);
            }
        })
        .expect("디코더 스레드 생성");
    DecodeInput { tx, backlog, stats }
}
