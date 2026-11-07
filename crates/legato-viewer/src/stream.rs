//! 영상 수신과 디코딩. 네트워크 작업은 tokio 에서, 디코딩은 디스플레이마다 전용 스레드에서 돈다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use legato_core::codec::{H264Decoder, I420Frame};
use legato_core::framing::read_msg;
use legato_core::protocol::{ClientMsg, VideoPacket};
use tokio::sync::mpsc::UnboundedSender;

/// 디코딩 대기열이 이보다 길면 따라잡기 모드: 키프레임이 올 때까지 버리고 키프레임을 요청한다.
const MAX_BACKLOG: usize = 15;

/// 디스플레이 하나의 디코딩 입구.
#[derive(Clone)]
pub struct DecodeInput {
    pub tx: mpsc::Sender<VideoPacket>,
    pub backlog: Arc<AtomicUsize>,
}

/// 디스플레이 번호 → 디코딩 입구. 창을 열고 닫을 때 바뀐다.
pub type Routes = Arc<Mutex<HashMap<u32, DecodeInput>>>;

/// 호스트가 여는 단방향 영상 스트림을 계속 받아 디스플레이별 디코딩 스레드로 넘긴다.
pub fn spawn_video_reader(conn: quinn::Connection, routes: Routes) {
    tokio::spawn(async move {
        while let Ok(mut uni) = conn.accept_uni().await {
            let routes = routes.clone();
            tokio::spawn(async move {
                loop {
                    match read_msg::<_, VideoPacket>(&mut uni).await {
                        Ok(Some(pkt)) => {
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
pub fn spawn_decoder(
    display_id: u32,
    ctl: UnboundedSender<ClientMsg>,
    mut on_frame: impl FnMut(I420Frame, u64) + Send + 'static,
) -> DecodeInput {
    let (tx, rx) = mpsc::channel::<VideoPacket>();
    let backlog = Arc::new(AtomicUsize::new(0));
    let bl = backlog.clone();
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
            // 키프레임을 기다리는 중인지 (첫 프레임, 디코딩 오류, 대기열 폭주 후).
            let mut waiting_key = true;
            let mut requested = false;
            while let Ok(pkt) = rx.recv() {
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
                        continue;
                    }
                    waiting_key = false;
                }
                match dec.decode(&pkt.data) {
                    Ok(Some(frame)) => on_frame(frame, pkt.capture_us),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("디스플레이 {display_id} 디코딩 오류, 키프레임 요청: {e:#}");
                        waiting_key = true;
                        requested = false;
                    }
                }
            }
        })
        .expect("디코더 스레드 생성");
    DecodeInput { tx, backlog }
}
