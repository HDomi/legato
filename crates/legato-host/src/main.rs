//! Legato 호스트 (원격받는 쪽).
//!
//! 사용 예:
//!   legato-host serve                     # 접속 대기
//!   legato-host set-password --generate   # 표시 비밀번호 새로 만들기
//!   legato-host set-access-code           # 고정 접속 코드 설정 (표준 입력)
//!   legato-host info --json

mod config;
mod events;
mod platform;
mod server;

use std::io::BufRead;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use legato_core::auth::{MIN_ACCESS_CODE_LEN, PasswordRecord, generate_password};
use legato_core::identity::short_fingerprint;

use crate::config::{HostConfig, load_identity};

#[derive(Parser)]
#[command(name = "legato-host", version, about = "Legato 호스트: 이 컴퓨터 화면을 원격으로 열어준다")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 접속을 기다린다.
    Serve {
        /// UDP 포트. 생략하면 설정 파일 값 (기본 47820).
        #[arg(long)]
        port: Option<u16>,
        /// 수신 주소.
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// 인코딩 최대 폭 상한 (픽셀). 뷰어가 고른 화질도 이 값을 넘지 않는다.
        #[arg(long, default_value_t = 3840)]
        max_width: u32,
        /// 초당 프레임 상한.
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// 비트레이트 상한 (bps).
        #[arg(long, default_value_t = 20_000_000)]
        bitrate: u32,
        /// 호스팅 중에도 시스템 잠자기를 허용한다 (기본은 막음).
        #[arg(long)]
        allow_sleep: bool,
    },
    /// 표시 비밀번호를 바꾼다. `--generate` 가 없으면 표준 입력 첫 줄을 쓴다.
    SetPassword {
        #[arg(long)]
        generate: bool,
        #[arg(long)]
        json: bool,
    },
    /// 고정 접속 코드를 설정한다 (표준 입력 첫 줄, 10자 이상). 해시만 저장하고 다시 보여주지 않는다.
    SetAccessCode,
    /// 고정 접속 코드를 지운다.
    ClearAccessCode,
    /// 표시 비밀번호 동작을 바꾼다.
    Configure {
        /// 켤 때마다 표시 비밀번호를 새로 만들지.
        #[arg(long)]
        rotate_display_password: Option<bool>,
        /// 표시 비밀번호로 접속을 허용할지. 끄면 고정 접속 코드로만 접속된다.
        #[arg(long)]
        allow_display_password: Option<bool>,
    },
    /// 지문, 포트, 권한 상태를 보여준다.
    Info {
        #[arg(long)]
        json: bool,
    },
    /// 화면 기록·손쉬운 사용 권한을 요청한다 (macOS).
    RequestPermissions,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { port, bind, max_width, fps, bitrate, allow_sleep } => {
            serve(port, &bind, max_width, fps, bitrate, allow_sleep)
        }
        Cmd::SetPassword { generate, json } => set_password(generate, json),
        Cmd::SetAccessCode => set_access_code(),
        Cmd::ClearAccessCode => {
            let mut cfg = HostConfig::load()?;
            cfg.access_code = None;
            cfg.save()?;
            println!("{}", serde_json::json!({ "ok": true }));
            Ok(())
        }
        Cmd::Configure { rotate_display_password, allow_display_password } => {
            let mut cfg = HostConfig::load()?;
            if let Some(v) = rotate_display_password {
                cfg.rotate_display_password = v;
            }
            if let Some(v) = allow_display_password {
                if !v && cfg.access_code.is_none() {
                    bail!("고정 접속 코드가 없으면 표시 비밀번호를 끌 수 없어 (아무도 접속 못 하게 됨)");
                }
                cfg.allow_display_password = v;
            }
            cfg.save()?;
            println!("{}", serde_json::json!({ "ok": true }));
            Ok(())
        }
        Cmd::Info { json } => info(json),
        Cmd::RequestPermissions => {
            let screen = platform::request_screen_capture();
            let ax = platform::request_accessibility();
            println!("{}", serde_json::json!({ "screen_capture": screen, "accessibility": ax }));
            Ok(())
        }
    }
}

/// 표준 입력 첫 줄을 읽는다 (줄바꿈 제외).
fn read_stdin_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).context("표준 입력 읽기 실패")?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn set_password(generate: bool, json: bool) -> Result<()> {
    let password = if generate {
        generate_password()
    } else {
        let p = read_stdin_line()?;
        if p.chars().count() < 8 {
            bail!("비밀번호는 8자 이상이어야 해");
        }
        p
    };
    let mut cfg = HostConfig::load()?;
    cfg.display_password = Some(password.clone());
    cfg.save()?;
    if json {
        println!("{}", serde_json::json!({ "password": password }));
    } else {
        println!("표시 비밀번호: {password}");
    }
    Ok(())
}

fn set_access_code() -> Result<()> {
    let code = read_stdin_line()?;
    if code.chars().count() < MIN_ACCESS_CODE_LEN {
        bail!("고정 접속 코드는 {MIN_ACCESS_CODE_LEN}자 이상이어야 해");
    }
    let mut cfg = HostConfig::load()?;
    if cfg.display_password.as_deref() == Some(code.as_str()) {
        bail!("고정 접속 코드는 표시 비밀번호와 달라야 해");
    }
    cfg.access_code = Some(PasswordRecord::new(&code));
    cfg.save()?;
    println!("{}", serde_json::json!({ "ok": true }));
    Ok(())
}

/// 이 컴퓨터의 사설·공인 IPv4 주소 목록 (루프백 제외).
fn local_addresses() -> Vec<String> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
        .map(|i| i.ip().to_string())
        .collect()
}

fn info(json: bool) -> Result<()> {
    let cfg = HostConfig::load()?;
    let id = load_identity()?;
    let fp = id.fingerprint();
    let v = serde_json::json!({
        "port": cfg.port,
        "fingerprint": fp,
        "short_fingerprint": short_fingerprint(&fp),
        "display_password": if cfg.allow_display_password { cfg.display_password.clone() } else { None },
        "allow_display_password": cfg.allow_display_password,
        "rotate_display_password": cfg.rotate_display_password,
        "has_access_code": cfg.access_code.is_some(),
        "screen_capture": platform::screen_capture_allowed(),
        "accessibility": platform::accessibility_trusted(),
        "addresses": local_addresses(),
        "config_dir": legato_core::paths::config_dir()?.display().to_string(),
    });
    if json {
        println!("{v}");
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}

fn serve(port: Option<u16>, bind: &str, max_width: u32, fps: u32, bitrate: u32, allow_sleep: bool) -> Result<()> {
    let mut cfg = HostConfig::load()?;
    if !cfg.allow_display_password && cfg.access_code.is_none() {
        // 설정 파일을 직접 고친 경우. 아무도 못 들어오는 상태로는 열지 않는다.
        bail!("표시 비밀번호가 꺼져 있고 고정 접속 코드도 없어서 접속할 방법이 없어");
    }
    // 표시 비밀번호: 없거나 매번 새로 만드는 설정이면 새로 만든다.
    if cfg.allow_display_password && (cfg.display_password.is_none() || cfg.rotate_display_password) {
        cfg.display_password = Some(generate_password());
        cfg.save()?;
    }
    let display = if cfg.allow_display_password {
        let pw = cfg.display_password.clone().expect("위에서 채움");
        eprintln!("표시 비밀번호: {pw}");
        events::emit("display_password", serde_json::json!({ "password": pw }));
        Some(PasswordRecord::new(&pw))
    } else {
        None
    };
    let port = port.unwrap_or(cfg.port);
    let id = load_identity()?;
    let fp = id.fingerprint();
    if !platform::screen_capture_allowed() {
        tracing::warn!("화면 기록 권한이 없어. 접속하면 캡처가 실패할 거야 (legato-host request-permissions)");
    }

    let _sleep_guard = if allow_sleep { None } else { platform::prevent_sleep() };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let addr: SocketAddr = format!("{bind}:{port}").parse().context("수신 주소 형식 오류")?;
        let endpoint = legato_core::transport::server_endpoint(&id, addr)?;
        tracing::info!("{addr} 에서 대기 중. 지문 {}", short_fingerprint(&fp));
        events::emit(
            "listening",
            serde_json::json!({ "port": port, "fingerprint": fp, "addresses": local_addresses() }),
        );
        let limits = legato_core::protocol::EncodeParams { max_width, fps, bitrate };
        let state = Arc::new(server::ServerState::new(display, cfg.access_code.clone(), limits));
        tokio::select! {
            r = server::run(endpoint.clone(), state) => r,
            _ = tokio::signal::ctrl_c() => {
                endpoint.close(0u32.into(), b"shutdown");
                Ok(())
            }
        }
    })
}
