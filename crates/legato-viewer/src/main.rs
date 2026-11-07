//! Legato 뷰어 (접속하는 쪽).
//!
//! 사용 예:
//!   LEGATO_PASSWORD=xxxx-xxxx-xxxx legato-viewer 192.168.0.10
//!   legato-viewer 192.168.0.10:47820 --windowed
//!   legato-viewer 127.0.0.1 --headless --frames 120 --dump frame.png   # 창 없이 수신 검증

mod app;
mod keys;
mod net;
mod render;
mod stream;

use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use legato_core::codec::I420Frame;
use legato_core::framing::{read_msg, write_msg};
use legato_core::protocol::{ClientMsg, ServerMsg};
use winit::event_loop::EventLoop;

use crate::app::{App, UserEvent};

#[derive(Parser)]
#[command(name = "legato-viewer", version, about = "Legato 뷰어: 원격 컴퓨터 화면을 열고 조작한다")]
struct Cli {
    /// 호스트 주소 (IP 또는 IP:포트).
    host: String,
    /// 볼 디스플레이 ID. 생략하면 주 디스플레이.
    #[arg(long)]
    display: Option<u32>,
    /// 전체화면 대신 일반 창으로 연다 (macOS 기본은 전체화면 Space).
    #[arg(long)]
    windowed: bool,
    /// 호스트 인증서 지문이 바뀌었어도 새 지문을 받아들인다.
    #[arg(long)]
    accept_new_fingerprint: bool,
    /// 창 없이 영상만 받아 통계를 낸다 (검증용).
    #[arg(long)]
    headless: bool,
    /// headless 에서 받을 프레임 수.
    #[arg(long, default_value_t = 120)]
    frames: u32,
    /// headless 에서 마지막 프레임을 PNG 로 저장할 경로.
    #[arg(long)]
    dump: Option<std::path::PathBuf>,
    /// 진단용: headless 에서 원격 마우스를 이 정규화 좌표(x,y)로 옮긴다 (입력 주입 확인).
    #[arg(long, hide = true, value_parser = parse_point)]
    probe_mouse: Option<(f32, f32)>,
}

fn parse_point(s: &str) -> Result<(f32, f32), String> {
    let (x, y) = s.split_once(',').ok_or("x,y 형식")?;
    Ok((x.trim().parse().map_err(|_| "x 숫자 아님")?, y.trim().parse().map_err(|_| "y 숫자 아님")?))
}

/// 표준 출력 JSON 이벤트 (Tauri 앱이 읽는다).
fn emit(event: &str, data: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::json!({ "event": event, "data": data }));
    let _ = out.flush();
}

/// QUIC 종료 이유를 사람이 읽을 문장으로 바꾼다.
fn describe_close(e: &quinn::ConnectionError) -> String {
    match e {
        quinn::ConnectionError::ApplicationClosed(c) if c.reason.as_ref() == b"replaced" => {
            "다른 곳에서 이 호스트에 접속해서 이 연결은 끊겼어".into()
        }
        quinn::ConnectionError::TimedOut => "호스트 응답이 끊겼어 (네트워크 확인)".into(),
        other => format!("연결 끊김: {other}"),
    }
}

fn resolve(host: &str) -> Result<(SocketAddr, String)> {
    let with_port = if host.parse::<SocketAddr>().is_ok() || host.rsplit_once(':').is_some_and(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok()) {
        host.to_string()
    } else if host.contains(':') {
        format!("[{host}]:{}", legato_core::DEFAULT_PORT) // 포트 없는 IPv6
    } else {
        format!("{host}:{}", legato_core::DEFAULT_PORT)
    };
    let addr = with_port
        .to_socket_addrs()
        .with_context(|| format!("주소 해석 실패: {host}"))?
        .next()
        .with_context(|| format!("주소 없음: {host}"))?;
    Ok((addr, with_port))
}

fn password() -> Result<String> {
    if let Ok(p) = std::env::var("LEGATO_PASSWORD") {
        return Ok(p);
    }
    rpassword::prompt_password("비밀번호 또는 접속 코드: ").context("비밀번호 입력 실패")
}

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn,wgpu=warn".into()),
        )
        .init();
    if let Err(e) = run(Cli::parse()) {
        eprintln!("오류: {e:#}");
        emit("closed", serde_json::json!({ "error": format!("{e:#}") }));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let (addr, host_key) = resolve(&cli.host)?;
    let password = password()?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    let session = rt.block_on(net::connect(addr, &host_key, &password, cli.accept_new_fingerprint))?;
    let target = match cli.display {
        Some(id) => session.displays.iter().find(|d| d.id == id).cloned().with_context(|| format!("디스플레이 {id} 없음"))?,
        None => session
            .displays
            .iter()
            .find(|d| d.primary)
            .or(session.displays.first())
            .cloned()
            .context("호스트에 디스플레이가 없음")?,
    };
    tracing::info!("{} 에 접속. 디스플레이 {} ({}x{})", session.host_name, target.id, target.width, target.height);
    emit(
        "connected",
        serde_json::json!({ "host": session.host_name, "displays": session.displays, "via_access_code": session.via_access_code }),
    );

    let net::Session { endpoint, conn, mut send, mut recv, host_name, .. } = session;

    // 제어 메시지 송신 작업.
    let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMsg>();
    rt.spawn(async move {
        while let Some(msg) = ctl_rx.recv().await {
            if write_msg(&mut send, &msg).await.is_err() {
                break;
            }
        }
    });
    ctl_tx.send(ClientMsg::StartStream { display_id: target.id }).ok();

    // 영상 수신 → 디코딩 스레드.
    let backlog = Arc::new(AtomicUsize::new(0));
    let (pkt_tx, pkt_rx) = std::sync::mpsc::channel();
    {
        let _guard = rt.enter();
        stream::spawn_video_reader(conn.clone(), pkt_tx, backlog.clone());
    }

    if let Some((x, y)) = cli.probe_mouse {
        ctl_tx.send(ClientMsg::Input(legato_core::protocol::InputEvent::MouseMove { x, y })).ok();
    }

    if cli.headless {
        let r = run_headless(&rt, conn, recv, pkt_rx, backlog, ctl_tx, cli.frames, cli.dump);
        // 종료 신호가 호스트에 전달될 때까지 잠깐 기다린다 (안 그러면 호스트가 시간 초과까지 세션을 붙잡음).
        rt.block_on(async { tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await.ok() });
        return r;
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

    // 제어 메시지 수신 작업.
    {
        let proxy = proxy.clone();
        let conn = conn.clone();
        rt.spawn(async move {
            let reason = loop {
                tokio::select! {
                    msg = read_msg::<_, ServerMsg>(&mut recv) => match msg {
                        Ok(Some(ServerMsg::StreamStarted { width, height, .. })) => {
                            tracing::info!("영상 시작 {width}x{height}");
                        }
                        Ok(Some(ServerMsg::Notice { message })) => {
                            eprintln!("알림: {message}");
                            emit("notice", serde_json::json!({ "message": message }));
                            let _ = proxy.send_event(UserEvent::Notice(message));
                        }
                        Ok(Some(ServerMsg::Error { message })) => break message,
                        Ok(Some(other)) => tracing::debug!("무시한 메시지: {other:?}"),
                        Ok(None) => break "호스트가 연결을 닫았어".to_string(),
                        Err(e) => break match conn.close_reason() {
                            Some(reason) => describe_close(&reason),
                            None => format!("연결 끊김: {e:#}"),
                        },
                    },
                    e = conn.closed() => break describe_close(&e),
                }
            };
            let _ = proxy.send_event(UserEvent::Closed(reason));
        });
    }

    let latest: Arc<Mutex<Option<I420Frame>>> = Arc::new(Mutex::new(None));
    {
        let latest = latest.clone();
        let proxy = proxy.clone();
        stream::spawn_decoder(pkt_rx, backlog, ctl_tx.clone(), move |frame, _capture_us| {
            *latest.lock().expect("뮤텍스") = Some(frame);
            let _ = proxy.send_event(UserEvent::NewFrame);
        });
    }

    let fullscreen = cfg!(target_os = "macos") && !cli.windowed;
    let mut app = App::new(format!("Legato — {host_name}"), fullscreen, (target.width, target.height), latest, ctl_tx);
    event_loop.run_app(&mut app)?;

    conn.close(0u32.into(), b"bye");
    rt.block_on(async { tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await.ok() });
    match app.exit_message.take() {
        Some(msg) => {
            emit("closed", serde_json::json!({ "error": msg }));
            bail!("{msg}")
        }
        None => {
            emit("closed", serde_json::json!({}));
            Ok(())
        }
    }
}

/// 창 없이 프레임을 받아 지연·FPS 를 재고 마지막 프레임을 PNG 로 남긴다.
#[allow(clippy::too_many_arguments)]
fn run_headless(
    rt: &tokio::runtime::Runtime,
    conn: quinn::Connection,
    mut recv: quinn::RecvStream,
    pkt_rx: std::sync::mpsc::Receiver<legato_core::protocol::VideoPacket>,
    backlog: Arc<AtomicUsize>,
    ctl_tx: tokio::sync::mpsc::UnboundedSender<ClientMsg>,
    frames: u32,
    dump: Option<std::path::PathBuf>,
) -> Result<()> {
    rt.spawn(async move {
        while let Ok(Some(msg)) = read_msg::<_, ServerMsg>(&mut recv).await {
            match msg {
                ServerMsg::StreamStarted { width, height, .. } => tracing::info!("영상 시작 {width}x{height}"),
                ServerMsg::Notice { message } => eprintln!("알림: {message}"),
                ServerMsg::Error { message } => {
                    eprintln!("오류: {message}");
                    break;
                }
                other => tracing::info!("메시지: {other:?}"),
            }
        }
    });
    let (done_tx, done_rx) = std::sync::mpsc::channel::<(I420Frame, u64, u64)>();
    stream::spawn_decoder(pkt_rx, backlog, ctl_tx, move |frame, capture_us| {
        let _ = done_tx.send((frame, capture_us, legato_core::now_micros()));
    });

    let start = Instant::now();
    let mut lat: Vec<f64> = Vec::new();
    let mut last: Option<I420Frame> = None;
    let mut first_at: Option<Instant> = None;
    while lat.len() < frames as usize {
        let remaining = Duration::from_secs(30).saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match done_rx.recv_timeout(remaining) {
            Ok((f, cap, now)) => {
                first_at.get_or_insert_with(Instant::now);
                lat.push(now.saturating_sub(cap) as f64 / 1000.0);
                last = Some(f);
            }
            Err(_) => break,
        }
    }
    // 호스트 쪽에서 먼저 끊었으면 이유를 알려준다.
    if let Some(reason) = conn.close_reason() {
        eprintln!("알림: {}", describe_close(&reason));
    }
    conn.close(0u32.into(), b"bye");
    let Some(last) = last else { bail!("30초 안에 디코딩된 프레임이 없음") };
    let elapsed = first_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    let mut sorted = lat.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("NaN 없음"));
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    let stats = serde_json::json!({
        "frames": lat.len(),
        "width": last.width,
        "height": last.height,
        "fps": if elapsed > 0.0 { (lat.len() - 1) as f64 / elapsed } else { 0.0 },
        "latency_ms_p50": pct(0.5),
        "latency_ms_p95": pct(0.95),
        "latency_ms_max": pct(1.0),
    });
    println!("{stats}");
    if let Some(path) = dump {
        let file = std::fs::File::create(&path).with_context(|| format!("{} 만들기 실패", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), last.width as u32, last.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&last.to_rgb())?;
        eprintln!("마지막 프레임 저장: {}", path.display());
    }
    Ok(())
}
