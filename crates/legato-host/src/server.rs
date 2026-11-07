//! 접속 수락, 인증, 영상 송출, 입력 처리.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use legato_core::auth::{AuthLimiter, PasswordRecord, channel_binding, new_nonce};
use legato_core::codec::H264Encoder;
use legato_core::framing::{read_msg, write_msg};
use legato_core::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg, VideoPacket};
use tokio::sync::mpsc;

use crate::events::emit;
use crate::platform::{self, CaptureOptions};

/// 서버 전체가 공유하는 상태.
pub struct ServerState {
    pub password: PasswordRecord,
    pub max_width: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub host_name: String,
    limiter: Mutex<AuthLimiter>,
    busy: AtomicBool,
}

impl ServerState {
    pub fn new(password: PasswordRecord, max_width: u32, fps: u32, bitrate: u32) -> Self {
        Self {
            password,
            max_width,
            fps,
            bitrate,
            host_name: host_name(),
            limiter: Mutex::new(AuthLimiter::default()),
            busy: AtomicBool::new(false),
        }
    }
}

fn host_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Legato 호스트".into())
}

/// 접속을 계속 받는다.
pub async fn run(endpoint: quinn::Endpoint, state: Arc<ServerState>) -> Result<()> {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            let peer = incoming.remote_address();
            match incoming.await {
                Ok(conn) => {
                    if let Err(e) = handle_connection(conn.clone(), peer, state).await {
                        tracing::warn!("{peer} 세션 종료: {e:#}");
                        conn.close(1u32.into(), b"error");
                    }
                }
                Err(e) => tracing::debug!("{peer} 핸드셰이크 실패: {e}"),
            }
        });
    }
    Ok(())
}

/// 세션이 끝나면 busy 를 푼다.
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn handle_connection(conn: quinn::Connection, peer: SocketAddr, state: Arc<ServerState>) -> Result<()> {
    let ip = peer.ip();
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi())
        .await
        .context("제어 스트림 대기 시간 초과")??;

    let locked = state.limiter.lock().expect("뮤텍스").check(ip);
    if let Err(remaining) = locked {
        tracing::warn!("{peer} 잠김 상태에서 접속 시도 ({}초 남음)", remaining.as_secs());
        // 뷰어가 이유를 알 수 있게 메시지를 보내고 닫는다. 비밀번호 확인은 아예 하지 않는다.
        let _ = tokio::time::timeout(Duration::from_secs(5), read_msg::<_, ClientMsg>(&mut recv)).await;
        write_msg(&mut send, &ServerMsg::Error {
            message: format!("비밀번호를 너무 많이 틀려서 잠겼어. {}초 뒤에 다시 시도해", remaining.as_secs().max(1)),
        })
        .await?;
        send.finish().ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(2u32.into(), b"locked");
        return Ok(());
    }

    // 1. Hello
    let hello: ClientMsg = tokio::time::timeout(Duration::from_secs(10), read_msg(&mut recv))
        .await
        .context("Hello 대기 시간 초과")??
        .context("Hello 전에 스트림 종료")?;
    let ClientMsg::Hello { version, client_name } = hello else { bail!("첫 메시지가 Hello 가 아님") };
    if version != PROTOCOL_VERSION {
        write_msg(&mut send, &ServerMsg::Error { message: format!("프로토콜 버전 불일치 (호스트 {PROTOCOL_VERSION}, 뷰어 {version})") }).await?;
        send.finish().ok();
        bail!("프로토콜 버전 불일치");
    }

    // 2. 챌린지-응답
    let nonce = new_nonce();
    let binding = channel_binding(&conn)?;
    write_msg(&mut send, &ServerMsg::Challenge { salt: state.password.salt, iterations: state.password.iterations, nonce }).await?;
    let resp: ClientMsg = tokio::time::timeout(Duration::from_secs(30), read_msg(&mut recv))
        .await
        .context("인증 응답 대기 시간 초과")??
        .context("인증 응답 전에 스트림 종료")?;
    let ClientMsg::AuthResponse { mac } = resp else { bail!("인증 응답이 아님") };
    if !state.password.verify(&nonce, &binding, &mac) {
        state.limiter.lock().expect("뮤텍스").record_failure(ip);
        tracing::warn!("{peer} ({client_name}) 비밀번호 틀림");
        emit("auth_failed", serde_json::json!({ "peer": peer.to_string() }));
        write_msg(&mut send, &ServerMsg::AuthFailed { reason: "비밀번호가 틀렸어".into() }).await?;
        send.finish().ok();
        // 메시지가 전달될 시간을 잠깐 준다.
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(3u32.into(), b"auth");
        return Ok(());
    }
    state.limiter.lock().expect("뮤텍스").record_success(ip);

    if state.busy.swap(true, Ordering::SeqCst) {
        write_msg(&mut send, &ServerMsg::Error { message: "다른 뷰어가 이미 접속 중이야".into() }).await?;
        send.finish().ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(4u32.into(), b"busy");
        return Ok(());
    }
    let _busy = BusyGuard(&state.busy);

    // ScreenCaptureKit 호출은 블로킹이고, 권한 확인 창이 떠 있으면 응답이 올 때까지 멈춘다.
    // tokio 작업 스레드를 막지 않게 블로킹 스레드에서 돌리고 시간 제한을 둔다.
    let displays = match blocking_with_timeout(platform::list_displays).await {
        Ok(d) => d,
        Err(e) => {
            // 화면 기록 권한이 없을 때 등. 뷰어가 이유를 볼 수 있게 보내고 닫는다.
            let message = format!("호스트 화면을 가져올 수 없어: {e:#}");
            write_msg(&mut send, &ServerMsg::Error { message: message.clone() }).await?;
            send.finish().ok();
            tokio::time::sleep(Duration::from_millis(200)).await;
            conn.close(5u32.into(), b"capture");
            bail!(message);
        }
    };
    write_msg(&mut send, &ServerMsg::AuthOk {
        server_mac: state.password.server_proof(&nonce, &binding),
        host_name: state.host_name.clone(),
        displays: displays.clone(),
    }).await?;
    tracing::info!("{peer} ({client_name}) 인증 성공");
    emit("session_started", serde_json::json!({ "peer": peer.to_string(), "client": client_name }));

    if !platform::accessibility_trusted() {
        write_msg(&mut send, &ServerMsg::Notice {
            message: "호스트에 손쉬운 사용 권한이 없어서 마우스·키보드 조작이 안 돼. 호스트 Mac 에서 권한을 허용해줘".into(),
        })
        .await?;
    }

    // 3. 세션 루프
    let mut pipeline: Option<Pipeline> = None;
    let mut injector: Option<platform::Injector> = None;
    let mut warned_input = false;
    let result: Result<()> = async {
        loop {
            let Some(msg) = read_msg::<_, ClientMsg>(&mut recv).await? else { break };
            match msg {
                ClientMsg::StartStream { display_id } => {
                    pipeline = None; // 이전 캡처를 먼저 멈춘다.
                    let opts = CaptureOptions { display_id, max_width: state.max_width, fps: state.fps };
                    let (c, b) = (conn.clone(), state.bitrate);
                    let p = match blocking_with_timeout(move || Pipeline::start(&c, opts, b)).await {
                        Ok(p) => p,
                        Err(e) => {
                            let message = format!("화면 캡처를 시작할 수 없어: {e:#}");
                            write_msg(&mut send, &ServerMsg::Error { message: message.clone() }).await?;
                            send.finish().ok();
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            bail!(message);
                        }
                    };
                    write_msg(&mut send, &ServerMsg::StreamStarted { display_id, width: p.width, height: p.height }).await?;
                    injector = Some(platform::Injector::new(display_id));
                    pipeline = Some(p);
                }
                ClientMsg::Input(ev) => {
                    if let Some(inj) = injector.as_mut()
                        && let Err(e) = inj.handle(&ev)
                        && !warned_input
                    {
                        tracing::warn!("입력 주입 실패: {e:#}");
                        warned_input = true;
                    }
                }
                ClientMsg::RequestKeyframe => {
                    if let Some(p) = &pipeline {
                        p.keyframe.store(true, Ordering::SeqCst);
                    }
                }
                ClientMsg::Hello { .. } | ClientMsg::AuthResponse { .. } => bail!("세션 중 잘못된 메시지"),
            }
        }
        Ok(())
    }
    .await;
    if let Some(inj) = injector.as_mut() {
        let _ = inj.handle(&legato_core::protocol::InputEvent::ReleaseAll);
    }
    drop(pipeline);
    tracing::info!("{peer} 세션 끝");
    emit("session_ended", serde_json::json!({ "peer": peer.to_string() }));
    // 뷰어가 정상 종료(코드 0)로 닫은 경우는 오류가 아니다.
    match (&result, conn.close_reason()) {
        (Err(_), Some(quinn::ConnectionError::ApplicationClosed(c))) if c.error_code == 0u32.into() => Ok(()),
        _ => result,
    }
}

/// 블로킹 작업을 별도 스레드에서 돌리고 15초 안에 안 끝나면 실패로 본다.
/// (멈춘 스레드는 남지만 세션은 정리되어 다음 접속을 받을 수 있다.)
async fn blocking_with_timeout<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(15), tokio::task::spawn_blocking(f)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => bail!("작업 스레드 실패: {e}"),
        Err(_) => bail!("응답 없음. 호스트 Mac 에 화면 기록 권한 확인 창이 떠 있는지 봐줘"),
    }
}

/// 캡처 → 인코딩 → 전송 파이프라인. drop 하면 모두 멈춘다.
struct Pipeline {
    _capture: platform::CaptureHandle,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    width: u32,
    height: u32,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Pipeline {
    fn start(conn: &quinn::Connection, opts: CaptureOptions, bitrate: u32) -> Result<Self> {
        let (raw_tx, raw_rx) = sync_channel(1);
        let capture = platform::start_capture(&opts, raw_tx)?;
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));
        let (pkt_tx, mut pkt_rx) = mpsc::channel::<VideoPacket>(2);

        // 인코더 스레드: 블로킹 작업이라 tokio 밖에서 돈다.
        {
            let stop = stop.clone();
            let keyframe = keyframe.clone();
            let fps = opts.fps as f32;
            std::thread::Builder::new().name("legato-encoder".into()).spawn(move || {
                let mut enc = match H264Encoder::new(bitrate, fps) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("{e:#}");
                        return;
                    }
                };
                let mut seq = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    let raw = match raw_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    if keyframe.swap(false, Ordering::SeqCst) {
                        enc.request_keyframe();
                    }
                    match enc.encode(&raw.frame) {
                        Ok(Some(e)) => {
                            let pkt = VideoPacket {
                                seq,
                                capture_us: raw.capture_us,
                                keyframe: e.keyframe,
                                width: raw.frame.width as u32,
                                height: raw.frame.height as u32,
                                data: e.data,
                            };
                            seq += 1;
                            // 네트워크가 밀리면 여기서 기다린다. 그동안 새 캡처 프레임은 버려진다.
                            if pkt_tx.blocking_send(pkt).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::error!("{e:#}");
                            break;
                        }
                    }
                }
            })?;
        }

        // 전송 작업: 새 단방향 스트림에 패킷을 이어 쓴다.
        {
            let conn = conn.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                let mut uni = match conn.open_uni().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("영상 스트림 열기 실패: {e}");
                        return;
                    }
                };
                while let Some(pkt) = pkt_rx.recv().await {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = write_msg(&mut uni, &pkt).await {
                        tracing::debug!("영상 전송 중단: {e:#}");
                        break;
                    }
                }
                let _ = uni.finish();
            });
        }

        Ok(Self { width: capture.width, height: capture.height, _capture: capture, stop, keyframe })
    }
}
