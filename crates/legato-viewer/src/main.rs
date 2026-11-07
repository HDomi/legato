//! Legato 뷰어 (접속하는 쪽).
//!
//! 사용 예:
//!   LEGATO_PASSWORD=xxxx-xxxx-xxxx legato-viewer 192.168.0.10
//!   legato-viewer 192.168.0.10:47820 --windowed --display 2 --quality high
//!   legato-viewer 127.0.0.1 --headless --frames 120 --dump frame.png   # 창 없이 수신 검증

mod app;
mod keys;
mod menu;
mod net;
mod render;
mod stream;

use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use legato_core::clipboard::ClipboardSync;
use legato_core::codec::I420Frame;
use legato_core::framing::{read_msg, write_msg, write_tag};
use legato_core::keymap::should_swap_cmd_ctrl;
use legato_core::protocol::{ClientMsg, InputEvent, Os, Quality, ServerMsg, UNI_CLIPBOARD};
use legato_core::t;
use winit::event_loop::EventLoop;

use crate::app::{App, AppConfig, UserEvent};
use crate::stream::Routes;

#[derive(Clone, Copy, ValueEnum)]
enum QualityArg {
    Auto,
    High,
    Balanced,
    Low,
}

impl From<QualityArg> for Quality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Auto => Quality::Auto,
            QualityArg::High => Quality::High,
            QualityArg::Balanced => Quality::Balanced,
            QualityArg::Low => Quality::Low,
        }
    }
}

#[derive(Parser)]
#[command(name = "legato-viewer", version, about = "Legato 뷰어: 원격 컴퓨터 화면을 열고 조작한다")]
struct Cli {
    /// 호스트 주소 (IP 또는 IP:포트).
    host: String,
    /// 볼 디스플레이 ID (여러 번 쓸 수 있음). 생략하면 원격 디스플레이 전부.
    #[arg(long)]
    display: Vec<u32>,
    /// 화질. auto 는 네트워크 상태를 보고 호스트가 조절한다.
    #[arg(long, value_enum, default_value = "auto")]
    quality: QualityArg,
    /// 전체화면 대신 일반 창으로 연다 (macOS 기본은 전체화면 Space).
    #[arg(long)]
    windowed: bool,
    /// OS 가 달라도 Cmd↔Ctrl 을 바꾸지 않는다.
    #[arg(long)]
    no_swap_cmd_ctrl: bool,
    /// 클립보드를 공유하지 않는다.
    #[arg(long)]
    no_clipboard: bool,
    /// Caps Lock 을 원격 한/영 전환 키로 쓸지 (기본: macOS 는 켬, 그 밖은 끔).
    #[arg(long)]
    caps_hangul: Option<bool>,
    /// 호스트 인증서 지문이 바뀌었어도 새 지문을 받아들인다.
    #[arg(long)]
    accept_new_fingerprint: bool,
    /// 창 없이 영상만 받아 통계를 낸다 (검증용, 첫 디스플레이 하나).
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
    /// 진단용: headless 에서 이 키(W3C 이름)를 한 번 눌렀다 뗀다.
    #[arg(long, hide = true)]
    probe_key: Option<String>,
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
        quinn::ConnectionError::ApplicationClosed(c) if c.reason.as_ref() == b"replaced" => t!(
            "다른 곳에서 이 호스트에 접속해 이 연결은 끊어졌습니다",
            "Another viewer connected to this host, so this connection was closed"
        )
        .into(),
        quinn::ConnectionError::TimedOut => {
            t!("호스트 응답이 끊겼습니다. 네트워크를 확인해 주세요", "The host stopped responding. Check your network")
                .into()
        }
        other => format!("{}: {other}", t!("연결이 끊어졌습니다", "Disconnected")),
    }
}

fn resolve(host: &str) -> Result<(SocketAddr, String)> {
    let with_port = if host.parse::<SocketAddr>().is_ok()
        || host.rsplit_once(':').is_some_and(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok())
    {
        host.to_string()
    } else if host.contains(':') {
        format!("[{host}]:{}", legato_core::DEFAULT_PORT) // 포트 없는 IPv6
    } else {
        format!("{host}:{}", legato_core::DEFAULT_PORT)
    };
    let addr = with_port
        .to_socket_addrs()
        .with_context(|| format!("{}: {host}", t!("주소를 해석하지 못했습니다", "Couldn't resolve the address")))?
        .next()
        .with_context(|| format!("{}: {host}", t!("주소를 찾지 못했습니다", "Address not found")))?;
    Ok((addr, with_port))
}

fn password() -> Result<String> {
    if let Ok(p) = std::env::var("LEGATO_PASSWORD") {
        // 다른 곳(자식 프로세스 등)으로 새지 않게 바로 지운다. 아직 스레드가 하나뿐일 때 부른다.
        unsafe { std::env::remove_var("LEGATO_PASSWORD") };
        return Ok(p);
    }
    rpassword::prompt_password("비밀번호 또는 접속 코드: ").context("비밀번호 입력 실패")
}

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,quinn=warn,wgpu=warn".into()),
        )
        .init();
    if let Err(e) = run(Cli::parse()) {
        eprintln!("{}: {e:#}", t!("오류", "Error"));
        // 앱이 언어와 무관하게 알아보도록 지문 변경은 코드로도 알린다.
        let code = if e.downcast_ref::<net::FingerprintChanged>().is_some() { "fingerprint_changed" } else { "error" };
        emit("closed", serde_json::json!({ "error": format!("{e:#}"), "code": code }));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let (addr, host_key) = resolve(&cli.host)?;
    let password = password()?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    let session = rt.block_on(net::connect(addr, &host_key, &password, cli.accept_new_fingerprint))?;
    let mut open: Vec<u32> = if cli.display.is_empty() {
        // 주 디스플레이를 맨 앞에 (첫 로컬 모니터에 오게).
        let mut v: Vec<_> = session.displays.iter().map(|d| (!d.primary, d.id)).collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    } else {
        cli.display.clone()
    };
    for id in &open {
        if !session.displays.iter().any(|d| d.id == *id) {
            bail!(
                "{} {id} ({:?})",
                t!("호스트에 이 디스플레이가 없습니다:", "The host has no display"),
                session.displays.iter().map(|d| d.id).collect::<Vec<_>>()
            );
        }
    }
    if open.is_empty() {
        bail!("{}", t!("호스트에 디스플레이가 없습니다", "The host has no displays"));
    }
    if cli.headless {
        open.truncate(1);
    }
    let swap = !cli.no_swap_cmd_ctrl && should_swap_cmd_ctrl(Os::current(), session.host_os);
    tracing::info!(
        "{} ({:?}) 에 접속. 디스플레이 {:?}, Cmd↔Ctrl 교환 {}",
        session.host_name,
        session.host_os,
        open,
        if swap { "켬" } else { "끔" }
    );
    emit(
        "connected",
        serde_json::json!({ "host": session.host_name, "displays": session.displays, "via_access_code": session.via_access_code }),
    );

    let net::Session { endpoint, conn, mut send, mut recv, host_name, displays, .. } = session;

    // 제어 메시지 송신 작업.
    let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<ClientMsg>();
    rt.spawn(async move {
        while let Some(msg) = ctl_rx.recv().await {
            if write_msg(&mut send, &msg).await.is_err() {
                break;
            }
        }
    });

    // 클립보드 공유: 뷰어에서 바뀌면 호스트로 보낸다. 입력이 담긴 제어 스트림을 막지 않게 별도 스트림으로.
    let clipboard_on = Arc::new(AtomicBool::new(!cli.no_clipboard));
    let clipboard = if cli.headless {
        None
    } else {
        let (conn, on, handle) = (conn.clone(), clipboard_on.clone(), rt.handle().clone());
        ClipboardSync::start(move |text| {
            if !on.load(Ordering::SeqCst) {
                return;
            }
            let conn = conn.clone();
            handle.spawn(async move {
                let r: Result<()> = async {
                    let mut uni = conn.open_uni().await?;
                    let _ = uni.set_priority(-2);
                    write_tag(&mut uni, UNI_CLIPBOARD).await?;
                    write_msg(&mut uni, &ClientMsg::Clipboard { text }).await?;
                    uni.finish()?;
                    Ok(())
                }
                .await;
                if let Err(e) = r {
                    tracing::debug!("클립보드 보내기 실패: {e:#}");
                }
            });
        })
    };

    // 영상·클립보드 수신 → 디스플레이별 디코딩 스레드 / 로컬 클립보드.
    let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
    {
        let _guard = rt.enter();
        let clip = clipboard.as_ref().map(|c| c.sender());
        let on = clipboard_on.clone();
        let on_clipboard: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text| {
            if on.load(Ordering::SeqCst)
                && let Some(c) = &clip
            {
                c.set(text);
            }
        });
        stream::spawn_video_reader(conn.clone(), routes.clone(), on_clipboard);
    }

    if cli.headless {
        let display_id = open[0];
        if let Some((x, y)) = cli.probe_mouse {
            ctl_tx.send(ClientMsg::Input { display_id, event: InputEvent::MouseMove { x, y } }).ok();
        }
        if let Some(code) = &cli.probe_key {
            for down in [true, false] {
                ctl_tx.send(ClientMsg::Input { display_id, event: InputEvent::Key { code: code.clone(), down } }).ok();
            }
        }
        let r = run_headless(&rt, conn, recv, routes, ctl_tx, display_id, cli.quality.into(), cli.frames, cli.dump);
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
                        Ok(Some(ServerMsg::StreamStarted { display_id, width, height, encoder })) => {
                            tracing::info!("디스플레이 {display_id} 영상 시작 {width}x{height} ({encoder})");
                        }
                        Ok(Some(ServerMsg::LockState { locked })) => {
                            emit("lock_state", serde_json::json!({ "locked": locked }));
                            let _ = proxy.send_event(UserEvent::Locked(locked));
                        }
                        Ok(Some(ServerMsg::QualityChanged { display_id, quality })) => {
                            let _ = proxy.send_event(UserEvent::QualityChanged(display_id, quality));
                        }
                        Ok(Some(ServerMsg::Notice { message })) => {
                            eprintln!("{}: {message}", t!("알림", "Notice"));
                            emit("notice", serde_json::json!({ "message": message }));
                            let _ = proxy.send_event(UserEvent::Notice(message));
                        }
                        Ok(Some(ServerMsg::Error { message })) => break message,
                        Ok(Some(other)) => tracing::debug!("무시한 메시지: {other:?}"),
                        Ok(None) => break t!("호스트가 연결을 닫았습니다", "The host closed the connection").to_string(),
                        Err(e) => break match conn.close_reason() {
                            Some(reason) => describe_close(&reason),
                            None => format!("{}: {e:#}", t!("연결이 끊어졌습니다", "Disconnected")),
                        },
                    },
                    e = conn.closed() => break describe_close(&e),
                }
            };
            let _ = proxy.send_event(UserEvent::Closed(reason));
        });
    }

    let fullscreen = cfg!(target_os = "macos") && !cli.windowed;
    let cfg = AppConfig {
        host_name: host_name.clone(),
        fullscreen,
        displays,
        open,
        quality: cli.quality.into(),
        swap,
        clipboard_on,
        caps_hangul: cli.caps_hangul.unwrap_or(cfg!(target_os = "macos")),
    };
    // 1초마다 왕복 시간을 창에 알려 메뉴에 지연·fps 를 보여준다.
    {
        let (proxy, conn) = (proxy.clone(), conn.clone());
        rt.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                if proxy.send_event(UserEvent::Stats(conn.rtt().as_millis() as u32)).is_err() {
                    break;
                }
            }
        });
    }
    let mut app = App::new(cfg, ctl_tx, routes, proxy);
    event_loop.run_app(&mut app)?;

    drop(clipboard);
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
    routes: Routes,
    ctl_tx: tokio::sync::mpsc::UnboundedSender<ClientMsg>,
    display_id: u32,
    quality: Quality,
    frames: u32,
    dump: Option<std::path::PathBuf>,
) -> Result<()> {
    rt.spawn(async move {
        while let Ok(Some(msg)) = read_msg::<_, ServerMsg>(&mut recv).await {
            match msg {
                ServerMsg::StreamStarted { width, height, encoder, .. } => {
                    tracing::info!("영상 시작 {width}x{height} ({encoder})")
                }
                ServerMsg::Notice { message } => eprintln!("{}: {message}", t!("알림", "Notice")),
                ServerMsg::Error { message } => {
                    eprintln!("{}: {message}", t!("오류", "Error"));
                    break;
                }
                other => tracing::info!("메시지: {other:?}"),
            }
        }
    });
    let (done_tx, done_rx) = std::sync::mpsc::channel::<(I420Frame, u64, u64)>();
    let input = stream::spawn_decoder(display_id, ctl_tx.clone(), move |frame, capture_us| {
        let _ = done_tx.send((frame, capture_us, legato_core::now_micros()));
    });
    routes.lock().expect("뮤텍스").insert(display_id, input);
    ctl_tx.send(ClientMsg::StartStream { display_id, quality }).ok();

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
        eprintln!("{}: {}", t!("알림", "Notice"), describe_close(&reason));
    }
    conn.close(0u32.into(), b"bye");
    let Some(last) = last else {
        bail!("{}", t!("30초 안에 받은 화면이 없습니다", "No frames received within 30 seconds"))
    };
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
        "rtt_ms": conn.rtt().as_secs_f64() * 1000.0,
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
