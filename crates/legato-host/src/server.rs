//! 접속 수락, 인증, 영상 송출, 입력 처리.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use legato_core::adaptive::{AdaptiveController, Sample};
use legato_core::auth::{AuthLimiter, PasswordRecord, channel_binding, limiter_key, match_slot, new_nonce};
use legato_core::clipboard::ClipboardSync;
use legato_core::framing::{read_msg, read_msg_limited, read_tag, write_msg, write_tag};
use legato_core::i18n::Lang;
use legato_core::protocol::{
    ClientMsg, EncodeParams, InputEvent, MAX_CLIPBOARD_BYTES, Os, PROTOCOL_VERSION, Quality, ServerMsg, UNI_CLIPBOARD,
    UNI_VIDEO, VideoPacket,
};
use tokio::sync::mpsc;

use crate::events::emit;
use crate::platform::{self, CaptureOptions, FrameSink, RawFrame};

/// 서버 전체가 공유하는 상태.
pub struct ServerState {
    /// 표시 비밀번호 (허용된 경우만).
    pub display: Option<PasswordRecord>,
    /// 고정 접속 코드 (설정된 경우만).
    pub access_code: Option<PasswordRecord>,
    /// 화질 상한 (명령줄 옵션). 뷰어가 고른 단계도 이 값을 넘지 않는다.
    pub limits: EncodeParams,
    pub host_name: String,
    limiter: Mutex<AuthLimiter>,
    /// 인증 진행 중인 주소(IPv6 는 /64). 같은 주소에서 동시에 여러 번 추측해 잠금을 피하지 못하게 한 번에 하나만 허용한다.
    pending: Mutex<HashSet<IpAddr>>,
    /// 지금 화면을 보고 있는 세션 (번호, 연결). 한 번에 하나만.
    current: Mutex<Option<(u64, quinn::Connection)>>,
    next_id: AtomicU64,
}

impl ServerState {
    pub fn new(display: Option<PasswordRecord>, access_code: Option<PasswordRecord>, limits: EncodeParams) -> Self {
        Self {
            display,
            access_code,
            limits,
            host_name: host_name(),
            limiter: Mutex::new(AuthLimiter::default()),
            pending: Mutex::new(HashSet::new()),
            current: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }
}

fn host_name() -> String {
    // PATH 에 끼워 넣은 가짜 프로그램이 실행되지 않게 전체 경로나 환경 변수를 쓴다.
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let name = std::process::Command::new("/bin/hostname").output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
    name.map(|s| sanitize_name(s.trim())).filter(|s| !s.is_empty()).unwrap_or_else(|| "Legato".into())
}

/// 인증을 마치지 않은 연결의 최대 개수. 넘으면 새 연결은 바로 닫는다.
const MAX_UNAUTHENTICATED: usize = 32;

/// 인증 진행 중인 주소 표시를 연결이 끝나거나 인증을 마치면 지운다.
struct PendingGuard<'a> {
    state: &'a ServerState,
    key: IpAddr,
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.state.pending.lock().expect("뮤텍스").remove(&self.key);
    }
}

/// 접속을 계속 받는다.
pub async fn run(endpoint: quinn::Endpoint, state: Arc<ServerState>) -> Result<()> {
    // 인증 전 연결 수를 세는 세마포어. 핸드셰이크부터 인증 끝까지 자리를 차지한다.
    let unauth = Arc::new(tokio::sync::Semaphore::new(MAX_UNAUTHENTICATED));
    while let Some(incoming) = endpoint.accept().await {
        let Ok(permit) = unauth.clone().try_acquire_owned() else {
            tracing::warn!("{} 인증 전 연결이 너무 많아 거절", incoming.remote_address());
            incoming.refuse();
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let peer = incoming.remote_address();
            // 핸드셰이크도 오래 걸리면 끊는다.
            let incoming = match tokio::time::timeout(Duration::from_secs(10), incoming).await {
                Ok(r) => r,
                Err(_) => {
                    tracing::debug!("{peer} 핸드셰이크 시간 초과");
                    return;
                }
            };
            let conn_permit = permit;
            match incoming {
                Ok(conn) => {
                    if let Err(e) = handle_connection(conn.clone(), peer, state, conn_permit).await {
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

/// 세션이 끝나면 자기 자리를 비운다. 이미 다른 세션이 넘겨받았으면 건드리지 않는다.
struct SessionGuard<'a> {
    state: &'a ServerState,
    id: u64,
}
impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        let mut cur = self.state.current.lock().expect("뮤텍스");
        if cur.as_ref().is_some_and(|(id, _)| *id == self.id) {
            *cur = None;
        }
    }
}

/// 인증 전에 받는 메시지(Hello, 인증 응답) 크기 상한.
const PRE_AUTH_MAX: usize = 4096;

/// 접속한 쪽이 보낸 이름을 로그·화면에 써도 되게 줄인다 (제어 문자 제거, 64자).
fn sanitize_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// 기존 세션을 닫을 때 쓰는 QUIC 종료 코드와 이유. 뷰어가 이 값으로 안내 문구를 고른다.
pub const CLOSE_REPLACED: u32 = 6;
/// 호스트가 꺼져서 닫을 때의 종료 코드.
pub const CLOSE_HOST_STOPPED: u32 = 7;

async fn handle_connection(
    conn: quinn::Connection,
    peer: SocketAddr,
    state: Arc<ServerState>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let ip = peer.ip();
    let key = limiter_key(ip);
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi())
        .await
        .context("제어 스트림 대기 시간 초과")??;

    let locked = state.limiter.lock().expect("뮤텍스").check(ip);
    if let Err(remaining) = locked {
        tracing::warn!("{peer} 잠김 상태에서 접속 시도 ({}초 남음)", remaining.as_secs());
        // 뷰어가 이유를 알 수 있게 메시지를 보내고 닫는다. 비밀번호 확인은 아예 하지 않는다.
        // 뷰어 언어를 알려고 Hello 만 읽는다 (비밀번호 확인은 하지 않는다).
        let lang = match tokio::time::timeout(
            Duration::from_secs(5),
            read_msg_limited::<_, ClientMsg>(&mut recv, PRE_AUTH_MAX),
        )
        .await
        {
            Ok(Ok(Some(ClientMsg::Hello { lang, .. }))) => lang,
            _ => Lang::Ko,
        };
        let secs = remaining.as_secs().max(1);
        let message = match lang {
            Lang::Ko => format!("비밀번호를 여러 번 틀려서 잠시 잠겼습니다. {secs}초 뒤에 다시 시도해 주세요."),
            Lang::En => format!("Too many wrong attempts. Please try again in {secs} seconds."),
        };
        write_msg(&mut send, &ServerMsg::Error { message }).await?;
        send.finish().ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(2u32.into(), b"locked");
        return Ok(());
    }

    // 같은 주소에서 이미 인증 중이면 거절한다 (병렬 추측 방지).
    if !state.pending.lock().expect("뮤텍스").insert(key) {
        tracing::warn!("{peer} 같은 주소에서 이미 인증 중이라 거절");
        conn.close(4u32.into(), b"busy");
        return Ok(());
    }
    let pending = PendingGuard { state: &state, key };

    // 1. Hello
    let hello: ClientMsg = tokio::time::timeout(Duration::from_secs(10), read_msg_limited(&mut recv, PRE_AUTH_MAX))
        .await
        .context("Hello 대기 시간 초과")??
        .context("Hello 전에 스트림 종료")?;
    let ClientMsg::Hello { version, client_name, client_os, lang: vl } = hello else {
        bail!("첫 메시지가 Hello 가 아님")
    };
    let client_name = sanitize_name(&client_name);
    if version != PROTOCOL_VERSION {
        write_msg(
            &mut send,
            &ServerMsg::Error {
                message: match vl {
                    Lang::Ko => format!(
                        "호스트와 뷰어의 버전이 맞지 않습니다 (프로토콜 호스트 {PROTOCOL_VERSION}, 뷰어 {version}). 두 기기 모두 최신 버전으로 업데이트해 주세요."
                    ),
                    Lang::En => format!(
                        "Host and viewer versions don't match (protocol host {PROTOCOL_VERSION}, viewer {version}). Please update both to the latest version."
                    ),
                }
            },
        )
        .await?;
        send.finish().ok();
        bail!("프로토콜 버전 불일치");
    }

    // 2. 챌린지-응답. 슬롯 순서: 표시 비밀번호(허용 시), 고정 접속 코드(설정 시).
    let mut records: Vec<&PasswordRecord> = Vec::new();
    let mut is_code: Vec<bool> = Vec::new();
    if let Some(r) = &state.display {
        records.push(r);
        is_code.push(false);
    }
    if let Some(r) = &state.access_code {
        records.push(r);
        is_code.push(true);
    }
    let nonce = new_nonce();
    let binding = channel_binding(&conn)?;
    let slots = records.iter().map(|r| r.slot()).collect();
    write_msg(&mut send, &ServerMsg::Challenge { slots, nonce }).await?;
    let resp: ClientMsg = tokio::time::timeout(Duration::from_secs(30), read_msg_limited(&mut recv, PRE_AUTH_MAX))
        .await
        .context("인증 응답 대기 시간 초과")??
        .context("인증 응답 전에 스트림 종료")?;
    let ClientMsg::AuthResponse { macs } = resp else { bail!("인증 응답이 아님") };
    let matched = match_slot(&records, &nonce, &binding, &macs);
    let Some(slot) = matched else {
        state.limiter.lock().expect("뮤텍스").record_failure(ip);
        // 실패를 기록했으니 다음 시도를 받아도 된다 (아래 대기 중에 온 시도가 거절되지 않게).
        drop(pending);
        tracing::warn!("{peer} ({client_name}) 비밀번호 틀림");
        emit("auth_failed", serde_json::json!({ "peer": peer.to_string() }));
        let reason = vl.pick("비밀번호 또는 접속 코드가 맞지 않습니다", "Wrong password or access code").into();
        write_msg(&mut send, &ServerMsg::AuthFailed { reason }).await?;
        send.finish().ok();
        // 메시지가 전달될 시간을 잠깐 준다.
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn.close(3u32.into(), b"auth");
        return Ok(());
    };
    state.limiter.lock().expect("뮤텍스").record_success(ip);
    // 인증을 마쳤으므로 인증 전 자리와 진행 중 표시를 내놓는다.
    drop(pending);
    drop(permit);
    let via_access_code = is_code[slot];

    // 인증을 통과한 새 접속이 기존 세션을 넘겨받는다 (네트워크가 끊겼다 다시 붙는 경우 바로 들어오게).
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    let previous = state.current.lock().expect("뮤텍스").replace((id, conn.clone()));
    if let Some((_, old)) = previous {
        tracing::info!("{peer} 가 기존 세션 ({}) 을 넘겨받음", old.remote_address());
        old.close(CLOSE_REPLACED.into(), b"replaced");
    }
    let _session = SessionGuard { state: &state, id };

    // ScreenCaptureKit 호출은 블로킹이고, 권한 확인 창이 떠 있으면 응답이 올 때까지 멈춘다.
    // tokio 작업 스레드를 막지 않게 블로킹 스레드에서 돌리고 시간 제한을 둔다.
    let displays = match blocking_with_timeout(platform::list_displays).await {
        Ok(d) => d,
        Err(e) => {
            // 화면 기록 권한이 없을 때 등. 뷰어가 이유를 볼 수 있게 보내고 닫는다.
            let message = match vl {
                Lang::Ko => format!("호스트 화면을 가져올 수 없습니다 (화면 기록 권한을 확인해 주세요): {e:#}"),
                Lang::En => format!("Can't capture the host screen (check Screen Recording permission): {e:#}"),
            };
            write_msg(&mut send, &ServerMsg::Error { message: message.clone() }).await?;
            send.finish().ok();
            tokio::time::sleep(Duration::from_millis(200)).await;
            conn.close(5u32.into(), b"capture");
            bail!(message);
        }
    };
    write_msg(
        &mut send,
        &ServerMsg::AuthOk {
            slot: slot as u32,
            via_access_code,
            server_mac: records[slot].server_proof(&nonce, &binding),
            host_name: state.host_name.clone(),
            host_os: Os::current(),
            displays: displays.clone(),
        },
    )
    .await?;
    let how = if via_access_code { "고정 접속 코드" } else { "표시 비밀번호" };
    tracing::info!("{peer} ({client_name}, {client_os:?}) 인증 성공 ({how})");
    emit(
        "session_started",
        serde_json::json!({ "peer": peer.to_string(), "client": client_name, "via_access_code": via_access_code }),
    );
    // 잠든 디스플레이를 깨운다 (사용자 활동으로 알림).
    platform::wake_display();

    // 제어 스트림 쓰기는 작업 하나가 맡는다 (클립보드 스레드·화질 조절 등 여러 곳에서 보낸다).
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if write_msg(&mut send, &m).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });
    // 읽기도 작업 하나로 빼서 select 에서 안전하게 기다린다 (read_msg 는 취소 안전하지 않음).
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Result<ClientMsg>>();
    let in_tx_uni = in_tx.clone();
    let reader = tokio::spawn(async move {
        loop {
            match read_msg::<_, ClientMsg>(&mut recv).await {
                Ok(Some(m)) => {
                    if in_tx.send(Ok(m)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = in_tx.send(Err(e));
                    break;
                }
            }
        }
    });

    if !platform::accessibility_trusted() {
        let _ = out_tx.send(ServerMsg::Notice {
            message: vl
                .pick(
                    "호스트에 입력 권한(손쉬운 사용)이 없어 마우스·키보드 조작이 되지 않습니다. 호스트에서 권한을 허용해 주세요.",
                    "The host has no input permission (Accessibility), so mouse and keyboard won't work. Please allow it on the host.",
                )
                .into(),
        });
    }

    // 클립보드: 호스트에서 바뀌면 뷰어로 보낸다. 큰 텍스트가 제어 메시지를 막지 않게 별도 스트림으로.
    let clipboard = {
        let (conn, rt) = (conn.clone(), tokio::runtime::Handle::current());
        ClipboardSync::start(move |text| {
            tracing::debug!("호스트 클립보드 보냄 ({} 바이트)", text.len());
            let conn = conn.clone();
            rt.spawn(async move {
                if let Err(e) = send_uni(&conn, UNI_CLIPBOARD, -2, &ServerMsg::Clipboard { text }).await {
                    tracing::debug!("클립보드 보내기 실패: {e:#}");
                }
            });
        })
    };
    // 뷰어가 여는 단방향 스트림 (클립보드). 한 스트림에 메시지 하나.
    let uni_reader = {
        let (conn, in_tx) = (conn.clone(), in_tx_uni);
        tokio::spawn(async move {
            while let Ok(mut uni) = conn.accept_uni().await {
                let in_tx = in_tx.clone();
                tokio::spawn(async move {
                    if read_tag(&mut uni).await.ok() != Some(UNI_CLIPBOARD) {
                        return;
                    }
                    if let Ok(Some(m @ ClientMsg::Clipboard { .. })) =
                        read_msg_limited::<_, ClientMsg>(&mut uni, MAX_CLIPBOARD_BYTES + 64).await
                    {
                        let _ = in_tx.send(Ok(m));
                    }
                });
            }
        })
    };

    // 원격 세션 동안 화면이 꺼지거나 잠기지 않게 막는다.
    let _keep_awake = platform::keep_awake();
    // 잠금 상태: 접속 직후 한 번 알리고, 이후 바뀔 때마다 알린다. 잠겨 있으면 화면을 깨워 비밀번호 칸을 띄운다.
    let mut last_locked = platform::screen_locked();
    if let Some(locked) = last_locked {
        let _ = out_tx.send(ServerMsg::LockState { locked });
        emit("lock_state", serde_json::json!({ "locked": locked }));
        if locked {
            platform::wake_for_unlock();
        }
    }

    // 3. 세션 루프
    let mut pipelines: HashMap<u32, Pipeline> = HashMap::new();
    let mut injector = platform::Injector::new();
    let mut warned_input = false;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut old_restarts = 0u32;
    let result: Result<()> = async {
        loop {
            tokio::select! {
                msg = in_rx.recv() => {
                    let Some(msg) = msg else { break };
                    match msg? {
                        ClientMsg::StartStream { display_id, quality } => {
                            pipelines.remove(&display_id); // 같은 화면의 이전 캡처를 먼저 멈춘다.
                            let p = start_pipeline(&conn, &state, display_id, quality, None).await;
                            match p {
                                Ok(p) => {
                                    let _ = out_tx.send(p.started_msg());
                                    pipelines.insert(display_id, p);
                                }
                                Err(e) => {
                                    let message = match vl {
                                        Lang::Ko => format!("화면 캡처를 시작할 수 없습니다: {e:#}"),
                                        Lang::En => format!("Couldn't start screen capture: {e:#}"),
                                    };
                                    let _ = out_tx.send(ServerMsg::Error { message: message.clone() });
                                    tokio::time::sleep(Duration::from_millis(200)).await;
                                    bail!(message);
                                }
                            }
                        }
                        ClientMsg::StopStream { display_id } => {
                            pipelines.remove(&display_id);
                        }
                        ClientMsg::Input { display_id, event } => {
                            if let Err(e) = injector.handle(display_id, &event)
                                && !warned_input
                            {
                                tracing::warn!("입력 주입 실패: {e:#}");
                                warned_input = true;
                            }
                        }
                        ClientMsg::RequestKeyframe { display_id } => {
                            if let Some(p) = pipelines.get(&display_id) {
                                p.keyframe.store(true, Ordering::SeqCst);
                                p.stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        ClientMsg::Clipboard { text } => {
                            tracing::debug!("뷰어 클립보드 받음 ({} 바이트)", text.len());
                            if text.len() <= MAX_CLIPBOARD_BYTES
                                && let Some(c) = &clipboard
                            {
                                c.set(text);
                            }
                        }
                        ClientMsg::WakeScreen => platform::wake_for_unlock(),
                        ClientMsg::FrameAck { display_id, stream, seq } => {
                            if let Some(p) = pipelines.get(&display_id)
                                && p.stream == stream
                            {
                                p.latency.ack(seq);
                            }
                        }
                        ClientMsg::Hello { .. } | ClientMsg::AuthResponse { .. } => bail!("세션 중 잘못된 메시지"),
                    }
                }
                _ = ticker.tick() => {
                    // 잠금 상태가 바뀌었으면 알린다.
                    let locked = platform::screen_locked();
                    if locked.is_some() && locked != last_locked {
                        last_locked = locked;
                        let locked = locked.unwrap_or(false);
                        tracing::info!("화면 잠금 상태: {locked}");
                        let _ = out_tx.send(ServerMsg::LockState { locked });
                        emit("lock_state", serde_json::json!({ "locked": locked }));
                    }
                    // 지연 예산 계산에 쓰는 왕복 시간을 갱신한다.
                    let rtt = conn.rtt().as_millis().min(u32::MAX as u128) as u32;
                    for p in pipelines.values() {
                        p.latency.rtt_ms.store(rtt, Ordering::Relaxed);
                    }
                    // 자동 화질: 디스플레이마다 1초 측정값으로 단계를 정한다.
                    let mut changes = Vec::new();
                    let mut failed = Vec::new();
                    for (id, p) in pipelines.iter_mut() {
                        if p.failed.load(Ordering::SeqCst) {
                            failed.push(*id);
                        } else if let Some(level) = p.adapt() {
                            changes.push((*id, level));
                        }
                    }
                    // 인코더가 죽은 파이프라인은 같은 설정으로 다시 시작한다 (화면이 굳지 않게).
                    for display_id in failed {
                        let mut old = pipelines.remove(&display_id).expect("위에서 찾음");
                        let (quality, controller) = (old.quality, old.controller.take());
                        let level = controller.as_ref().map(|c| c.level()).unwrap_or(quality);
                        drop(old);
                        old_restarts += 1;
                        if old_restarts > 5 {
                            let message = vl
                                .pick(
                                    "화면 인코딩이 계속 실패해 연결을 끊었습니다",
                                    "Disconnected because video encoding kept failing",
                                )
                                .to_string();
                            let _ = out_tx.send(ServerMsg::Error { message });
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            bail!("화면 인코딩이 계속 실패해 연결을 끊음");
                        }
                        let message = vl.pick("화면 인코딩 오류가 있어 영상을 다시 시작했습니다", "Video restarted after an encoding error").into();
                        let _ = out_tx.send(ServerMsg::Notice { message });
                        let auto = (quality == Quality::Auto).then_some((controller, level));
                        match start_pipeline(&conn, &state, display_id, quality, auto).await {
                            Ok(p) => {
                                let _ = out_tx.send(p.started_msg());
                                pipelines.insert(display_id, p);
                            }
                            Err(e) => tracing::warn!("인코딩 오류 뒤 캡처 재시작 실패: {e:#}"),
                        }
                    }
                    for (display_id, level) in changes {
                        let mut old = pipelines.remove(&display_id).expect("위에서 찾음");
                        tracing::info!("디스플레이 {display_id} 자동 화질: {}", level.label(Lang::Ko));
                        let controller = old.controller.take();
                        drop(old); // 이전 캡처를 먼저 멈춘다.
                        match start_pipeline(&conn, &state, display_id, Quality::Auto, Some((controller, level))).await {
                            Ok(p) => {
                                let _ = out_tx.send(p.started_msg());
                                let _ = out_tx.send(ServerMsg::QualityChanged { display_id, quality: level });
                                pipelines.insert(display_id, p);
                            }
                            Err(e) => tracing::warn!("화질 변경 후 캡처 재시작 실패: {e:#}"),
                        }
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    let _ = injector.handle(0, &InputEvent::ReleaseAll);
    drop(pipelines);
    drop(clipboard);
    drop(out_tx);
    reader.abort();
    uni_reader.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
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
        Err(_) => bail!(
            "{}",
            legato_core::t!(
                "호스트가 응답하지 않습니다. 호스트에 화면 기록 권한 확인 창이 떠 있는지 확인해 주세요",
                "The host isn't responding. Check whether a Screen Recording permission prompt is open on the host"
            )
        ),
    }
}

/// 화질 단계와 상한을 합친 실제 인코딩 매개변수.
fn effective_params(level: Quality, limits: EncodeParams) -> EncodeParams {
    let p = level.params();
    EncodeParams {
        max_width: p.max_width.min(limits.max_width),
        fps: p.fps.min(limits.fps),
        bitrate: p.bitrate.min(limits.bitrate),
    }
}

/// 파이프라인을 시작한다. `auto` 가 있으면 자동 조절 중인 조절기와 단계를 이어받는다.
async fn start_pipeline(
    conn: &quinn::Connection,
    state: &ServerState,
    display_id: u32,
    quality: Quality,
    auto: Option<(Option<AdaptiveController>, Quality)>,
) -> Result<Pipeline> {
    let (controller, level) = match (quality, auto) {
        (Quality::Auto, Some((c, level))) => (Some(c.unwrap_or_default()), level),
        (Quality::Auto, None) => {
            let c = AdaptiveController::default();
            let level = c.level();
            (Some(c), level)
        }
        (fixed, _) => (None, fixed),
    };
    let params = effective_params(level, state.limits);
    let c = conn.clone();
    let stream = NEXT_STREAM.fetch_add(1, Ordering::Relaxed);
    let mut p = blocking_with_timeout(move || Pipeline::start(&c, display_id, params, stream)).await?;
    p.quality = quality;
    p.controller = controller;
    Ok(p)
}

/// 파이프라인 세대 번호 (프로세스 전체에서 증가).
static NEXT_STREAM: AtomicU32 = AtomicU32::new(1);

/// 보낸 뒤 아직 뷰어가 처리했다고 알려오지 않은 프레임들. 이걸로 화면에 쌓인 지연을 잰다.
///
/// 네트워크나 뷰어가 밀리면 확인 응답이 늦어지고, 이 목록의 가장 오래된 프레임 나이가 늘어난다.
/// 나이가 "왕복 시간 + 여유" 를 넘으면 새 프레임을 인코딩하지 않고 버린다. 그러면 대기열에 낡은
/// 프레임이 쌓이지 않아 화면 지연이 일정 한도를 넘지 않는다 (대신 순간 fps 가 내려간다).
#[derive(Default)]
pub struct Latency {
    inflight: Mutex<VecDeque<(u64, Instant)>>,
    pub rtt_ms: AtomicU32,
}

/// 왕복 시간 위에 허용하는 지연 여유.
const LATENCY_SLACK: Duration = Duration::from_millis(100);
/// 확인 응답 없이 보낼 수 있는 최대 프레임 수 (확인 응답이 아예 안 올 때의 안전장치).
const MAX_INFLIGHT_FRAMES: usize = 90;

impl Latency {
    fn sent(&self, seq: u64) {
        self.inflight.lock().expect("뮤텍스").push_back((seq, Instant::now()));
    }

    fn ack(&self, seq: u64) {
        let mut q = self.inflight.lock().expect("뮤텍스");
        while q.front().is_some_and(|(s, _)| *s <= seq) {
            q.pop_front();
        }
    }

    /// 지금 프레임을 더 보내면 지연이 한도를 넘는지.
    fn congested(&self) -> bool {
        // 진단용: 지연 제한을 끄고 비교할 때.
        if std::env::var_os("LEGATO_DEBUG_NO_LATENCY_LIMIT").is_some() {
            return false;
        }
        let q = self.inflight.lock().expect("뮤텍스");
        let budget = Duration::from_millis(self.rtt_ms.load(Ordering::Relaxed) as u64) + LATENCY_SLACK;
        q.len() >= MAX_INFLIGHT_FRAMES || q.front().is_some_and(|(_, t)| t.elapsed() > budget)
    }
}

/// 단방향 스트림 하나에 종류 바이트와 메시지 하나를 보내고 닫는다.
async fn send_uni<T: serde::Serialize>(conn: &quinn::Connection, tag: u8, priority: i32, msg: &T) -> Result<()> {
    let mut uni = conn.open_uni().await?;
    let _ = uni.set_priority(priority);
    write_tag(&mut uni, tag).await?;
    write_msg(&mut uni, msg).await?;
    uni.finish()?;
    Ok(())
}

/// 1초 측정값 묶음.
#[derive(Default)]
struct PipeStats {
    sent: AtomicU32,
    max_wait_ms: AtomicU32,
    keyframe_requests: AtomicU32,
}

/// 캡처 → 인코딩 → 전송 파이프라인 (디스플레이 하나). drop 하면 모두 멈춘다.
struct Pipeline {
    display_id: u32,
    /// 파이프라인 세대 (뷰어 확인 응답과 맞춰 본다).
    stream: u32,
    latency: Arc<Latency>,
    quality: Quality,
    controller: Option<AdaptiveController>,
    _capture: platform::CaptureHandle,
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    /// 인코더 스레드가 오류로 끝났는지.
    failed: Arc<AtomicBool>,
    stats: Arc<PipeStats>,
    dropped: Arc<AtomicU32>,
    width: u32,
    height: u32,
    encoder: &'static str,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Pipeline {
    fn started_msg(&self) -> ServerMsg {
        ServerMsg::StreamStarted {
            display_id: self.display_id,
            width: self.width,
            height: self.height,
            encoder: self.encoder.to_string(),
        }
    }

    /// 자동 화질이면 지난 1초 측정값을 조절기에 넣고, 단계가 바뀌면 새 단계를 돌려준다.
    fn adapt(&mut self) -> Option<Quality> {
        let sample = Sample {
            dropped: self.dropped.swap(0, Ordering::Relaxed),
            sent: self.stats.sent.swap(0, Ordering::Relaxed),
            max_send_wait_ms: self.stats.max_wait_ms.swap(0, Ordering::Relaxed),
            keyframe_requests: self.stats.keyframe_requests.swap(0, Ordering::Relaxed),
        };
        if self.quality != Quality::Auto {
            return None;
        }
        self.controller.as_mut()?.push(sample)
    }

    fn start(conn: &quinn::Connection, display_id: u32, params: EncodeParams, stream: u32) -> Result<Self> {
        let (raw_tx, raw_rx) = sync_channel(1);
        let sink = FrameSink::new(raw_tx);
        let dropped = sink.dropped.clone();
        let opts = CaptureOptions { display_id, max_width: params.max_width, fps: params.fps };
        let capture = platform::start_capture(&opts, sink)?;
        let mut enc =
            platform::create_encoder(capture.width as usize, capture.height as usize, params.bitrate, params.fps)?;
        let encoder = enc.name();
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(PipeStats::default());
        let failed = Arc::new(AtomicBool::new(false));
        let latency = Arc::new(Latency::default());
        latency.rtt_ms.store(conn.rtt().as_millis().min(u32::MAX as u128) as u32, Ordering::Relaxed);
        // 전송 대기열은 1개: 쌓인 지연은 확인 응답 기반 판단으로 막는다.
        let (pkt_tx, mut pkt_rx) = mpsc::channel::<VideoPacket>(1);

        // 인코더 스레드: 블로킹 작업이라 tokio 밖에서 돈다.
        {
            let stop = stop.clone();
            let keyframe = keyframe.clone();
            let stats = stats.clone();
            let failed = failed.clone();
            let latency = latency.clone();
            let dropped = dropped.clone();
            std::thread::Builder::new().name(format!("legato-encoder-{display_id}")).spawn(move || {
                let mut seq = 0u64;
                // 마지막 프레임. 화면이 멈춰 새 프레임이 안 와도 키프레임 요청에 답할 수 있게 들고 있는다.
                let mut last: Option<RawFrame> = None;
                while !stop.load(Ordering::SeqCst) {
                    let raw = match raw_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => {
                            if keyframe.load(Ordering::SeqCst)
                                && let Some(prev) = last.take()
                            {
                                prev
                            } else {
                                continue;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    // 뷰어가 아직 처리 못 한 프레임이 지연 한도를 넘었으면 이번 프레임은 버린다 (낡은 화면이 쌓이지 않게).
                    if latency.congested() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        last = Some(raw);
                        continue;
                    }
                    if keyframe.swap(false, Ordering::SeqCst) {
                        enc.request_keyframe();
                    }
                    match enc.encode(&raw) {
                        Ok(Some(e)) => {
                            let pkt = VideoPacket {
                                display_id,
                                stream,
                                seq,
                                capture_us: raw.capture_us,
                                keyframe: e.keyframe,
                                width: raw.width as u32,
                                height: raw.height as u32,
                                data: e.data,
                            };
                            seq += 1;
                            last = Some(raw);
                            // 네트워크가 밀리면 여기서 기다린다. 그동안 새 캡처 프레임은 버려진다.
                            let t = Instant::now();
                            if pkt_tx.blocking_send(pkt).is_err() {
                                break;
                            }
                            let waited = t.elapsed().as_millis() as u32;
                            stats.sent.fetch_add(1, Ordering::Relaxed);
                            stats.max_wait_ms.fetch_max(waited, Ordering::Relaxed);
                        }
                        Ok(None) => last = Some(raw),
                        Err(e) => {
                            tracing::error!("인코딩 실패: {e:#}");
                            failed.store(true, Ordering::SeqCst);
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
            let latency = latency.clone();
            let rt = tokio::runtime::Handle::current();
            rt.spawn(async move {
                let mut uni = match conn.open_uni().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("영상 스트림 열기 실패: {e}");
                        return;
                    }
                };
                // 제어 메시지(우선순위 0)가 영상보다 먼저 나가게 한다.
                let _ = uni.set_priority(-1);
                if write_tag(&mut uni, UNI_VIDEO).await.is_err() {
                    return;
                }
                while let Some(pkt) = pkt_rx.recv().await {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let seq = pkt.seq;
                    if let Err(e) = write_msg(&mut uni, &pkt).await {
                        tracing::debug!("영상 전송 중단: {e:#}");
                        break;
                    }
                    latency.sent(seq);
                }
                let _ = uni.finish();
            });
        }

        Ok(Self {
            display_id,
            stream,
            latency,
            quality: Quality::Balanced,
            controller: None,
            width: capture.width,
            height: capture.height,
            _capture: capture,
            stop,
            keyframe,
            failed,
            stats,
            dropped,
            encoder,
        })
    }
}
