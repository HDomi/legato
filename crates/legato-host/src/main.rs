//! Legato 호스트 (원격받는 쪽).
//!
//! 사용 예:
//!   legato-host serve            # 접속 대기
//!   legato-host set-password --generate
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
use legato_core::auth::{PasswordRecord, generate_password};
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
        /// 인코딩 최대 폭 (픽셀).
        #[arg(long, default_value_t = 1920)]
        max_width: u32,
        /// 최대 초당 프레임.
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// 목표 비트레이트 (bps).
        #[arg(long, default_value_t = 8_000_000)]
        bitrate: u32,
    },
    /// 접속 비밀번호를 바꾼다. `--generate` 가 없으면 표준 입력 첫 줄을 비밀번호로 쓴다.
    SetPassword {
        #[arg(long)]
        generate: bool,
        #[arg(long)]
        json: bool,
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
        Cmd::Serve { port, bind, max_width, fps, bitrate } => serve(port, &bind, max_width, fps, bitrate),
        Cmd::SetPassword { generate, json } => set_password(generate, json),
        Cmd::Info { json } => info(json),
        Cmd::RequestPermissions => {
            let screen = platform::request_screen_capture();
            let ax = platform::request_accessibility();
            println!("{}", serde_json::json!({ "screen_capture": screen, "accessibility": ax }));
            Ok(())
        }
    }
}

fn set_password(generate: bool, json: bool) -> Result<()> {
    let password = if generate {
        generate_password()
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).context("표준 입력 읽기 실패")?;
        let p = line.trim_end_matches(['\r', '\n']).to_string();
        if p.chars().count() < 8 {
            bail!("비밀번호는 8자 이상이어야 해");
        }
        p
    };
    let mut cfg = HostConfig::load()?;
    cfg.password = Some(PasswordRecord::new(&password));
    cfg.save()?;
    if json {
        println!("{}", serde_json::json!({ "password": if generate { Some(&password) } else { None } }));
    } else if generate {
        println!("새 비밀번호: {password}");
    } else {
        println!("비밀번호를 바꿨어");
    }
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
        "has_password": cfg.password.is_some(),
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

fn serve(port: Option<u16>, bind: &str, max_width: u32, fps: u32, bitrate: u32) -> Result<()> {
    let mut cfg = HostConfig::load()?;
    if cfg.password.is_none() {
        // 비밀번호 없이 열리는 일은 없게 한다. 처음이면 만들어서 한 번 보여준다.
        let pw = generate_password();
        cfg.password = Some(PasswordRecord::new(&pw));
        cfg.save()?;
        eprintln!("비밀번호가 없어서 새로 만들었어: {pw}");
        events::emit("password_generated", serde_json::json!({ "password": pw }));
    }
    let port = port.unwrap_or(cfg.port);
    let id = load_identity()?;
    let fp = id.fingerprint();
    if !platform::screen_capture_allowed() {
        tracing::warn!("화면 기록 권한이 없어. 접속하면 캡처가 실패할 거야 (legato-host request-permissions)");
    }

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let addr: SocketAddr = format!("{bind}:{port}").parse().context("수신 주소 형식 오류")?;
        let endpoint = legato_core::transport::server_endpoint(&id, addr)?;
        tracing::info!("{addr} 에서 대기 중. 지문 {}", short_fingerprint(&fp));
        events::emit(
            "listening",
            serde_json::json!({ "port": port, "fingerprint": fp, "addresses": local_addresses() }),
        );
        let state = Arc::new(server::ServerState::new(cfg.password.clone().expect("위에서 채움"), max_width, fps, bitrate));
        tokio::select! {
            r = server::run(endpoint.clone(), state) => r,
            _ = tokio::signal::ctrl_c() => {
                endpoint.close(0u32.into(), b"shutdown");
                Ok(())
            }
        }
    })
}
