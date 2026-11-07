//! 영상 수신과 디코딩. 네트워크 작업은 tokio 에서, 디코딩은 전용 스레드에서 돈다.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use legato_core::codec::{H264Decoder, I420Frame};
use legato_core::framing::read_msg;
use legato_core::protocol::{ClientMsg, VideoPacket};
use tokio::sync::mpsc::UnboundedSender;

/// 디코딩 대기열이 이보다 길면 따라잡기 모드: 키프레임이 올 때까지 버리고 키프레임을 요청한다.
const MAX_BACKLOG: usize = 15;

/// 호스트가 여는 단방향 영상 스트림을 계속 받아 디코딩 스레드로 넘긴다.
pub fn spawn_video_reader(conn: quinn::Connection, tx: mpsc::Sender<VideoPacket>, backlog: Arc<AtomicUsize>) {
    tokio::spawn(async move {
        while let Ok(mut uni) = conn.accept_uni().await {
            let tx = tx.clone();
            let backlog = backlog.clone();
            tokio::spawn(async move {
                loop {
                    match read_msg::<_, VideoPacket>(&mut uni).await {
                        Ok(Some(pkt)) => {
                            backlog.fetch_add(1, Ordering::SeqCst);
                            if tx.send(pkt).is_err() {
                                break;
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

/// 디코딩 스레드를 띄운다. 프레임이 나올 때마다 `on_frame(프레임, 캡처시각)` 을 부른다.
pub fn spawn_decoder(
    rx: mpsc::Receiver<VideoPacket>,
    backlog: Arc<AtomicUsize>,
    ctl: UnboundedSender<ClientMsg>,
    mut on_frame: impl FnMut(I420Frame, u64) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("legato-decoder".into())
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
                let queued = backlog.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                if queued > MAX_BACKLOG && !waiting_key {
                    tracing::warn!("디코딩이 밀림 (대기 {queued}), 키프레임부터 다시 시작");
                    waiting_key = true;
                    requested = false;
                }
                if waiting_key {
                    if !pkt.keyframe {
                        if !requested {
                            let _ = ctl.send(ClientMsg::RequestKeyframe);
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
                        tracing::warn!("디코딩 오류, 키프레임 요청: {e:#}");
                        waiting_key = true;
                        requested = false;
                    }
                }
            }
        })
        .expect("디코더 스레드 생성");
}
