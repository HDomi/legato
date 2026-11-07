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
use legato_core::t;

use crate::config::{HostConfig, load_identity};

#[derive(Parser)]
#[command(name = "legato-host", version, about = "Legato 호스트: 이 컴퓨터 화면을 원격으로 열어준다")]
struct Cli {
    /// 로그를 표준 에러 대신 이 파일에 덧붙인다 (서비스 모드용).
    #[arg(long, global = true)]
    log_file: Option<std::path::PathBuf>,
    /// 설정 폴더 (LEGATO_CONFIG_DIR 와 같음). 서비스는 설치한 사용자의 설정 폴더를 쓴다.
    #[arg(long, global = true)]
    config_dir: Option<std::path::PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// 서비스를 설치하고 시작한다 (관리자 권한). 로그인 화면 · UAC 확인 창까지 원격으로 다룰 수 있다.
    Install,
    /// 서비스를 멈추고 지운다 (관리자 권한).
    Uninstall,
    /// 설치·실행 상태.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// 서비스 관리자가 실행하는 진입점 (직접 쓰지 않는다).
    Run,
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
        /// 설정 파일을 바꾸지 않는다 (macOS 로그인 화면 에이전트용: root 로 돌아도 사용자 파일 소유권을 지키게).
        /// 표시 비밀번호는 메모리에서만 만든다.
        #[arg(long)]
        readonly_config: bool,
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
    /// Windows 서비스 모드 관리.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// 진단용: 한/영 입력 소스를 한 번 바꾸고 결과를 출력한다 (macOS).
    #[command(hide = true)]
    ToggleIme,
    /// macOS 로그인 화면 에이전트 관리 (로그아웃 상태에서도 접속).
    LoginAgent {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

fn main() -> Result<()> {
    platform::init_process();
    let cli = Cli::parse();
    if let Some(dir) = &cli.config_dir {
        // 아직 스레드가 하나뿐일 때 설정한다 (서비스 헬퍼도 이 값을 물려받는다).
        unsafe { std::env::set_var("LEGATO_CONFIG_DIR", dir) };
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into());
    match &cli.log_file {
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("로그 파일 열기 실패: {}", path.display()))?;
            tracing_subscriber::fmt()
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .with_env_filter(filter)
                .init();
        }
        None => tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(filter).init(),
    }
    match cli.cmd {
        Cmd::Serve { port, bind, max_width, fps, bitrate, allow_sleep, readonly_config } => {
            serve(port, &bind, max_width, fps, bitrate, allow_sleep, readonly_config)
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
                    bail!(
                        "{}",
                        t!(
                            "고정 접속 코드가 없으면 표시 비밀번호를 끌 수 없습니다 (아무도 접속할 수 없게 됩니다)",
                            "You can't turn off the display password without an access code (no one could connect)"
                        )
                    );
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
        Cmd::Service { action } => service_cmd(action),
        Cmd::LoginAgent { action } => login_agent_cmd(action),
        Cmd::ToggleIme => {
            #[cfg(target_os = "macos")]
            println!("{:?}", platform::macos_ime_toggle());
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
fn login_agent_cmd(action: ServiceCmd) -> Result<()> {
    use platform::login_agent;
    match action {
        ServiceCmd::Install => login_agent::install(&legato_core::paths::config_dir()?)?,
        ServiceCmd::Uninstall => login_agent::uninstall()?,
        ServiceCmd::Status { .. } => {
            println!("{}", serde_json::json!({ "installed": login_agent::status(), "supported": true }));
            return Ok(());
        }
        ServiceCmd::Run => bail!(
            "{}",
            t!("로그인 화면 에이전트는 launchd 가 실행합니다", "The login-window agent is started by launchd")
        ),
    }
    println!("{}", serde_json::json!({ "ok": true }));
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn login_agent_cmd(action: ServiceCmd) -> Result<()> {
    match action {
        ServiceCmd::Status { .. } => {
            println!("{}", serde_json::json!({ "installed": false, "supported": false }));
            Ok(())
        }
        _ => bail!(
            "{}",
            t!(
                "로그인 화면 에이전트는 macOS 전용입니다 (Windows 는 서비스 모드)",
                "The login-window agent is macOS only (use service mode on Windows)"
            )
        ),
    }
}

#[cfg(windows)]
fn service_cmd(action: ServiceCmd) -> Result<()> {
    use platform::service;
    match action {
        ServiceCmd::Install => {
            let dir = legato_core::paths::config_dir()?;
            let port = HostConfig::load()?.port;
            service::install(&dir, port)?;
            println!("{}", serde_json::json!({ "ok": true }));
        }
        ServiceCmd::Uninstall => {
            service::uninstall()?;
            println!("{}", serde_json::json!({ "ok": true }));
        }
        ServiceCmd::Status { json } => {
            let (installed, running) = service::status();
            let v = serde_json::json!({ "installed": installed, "running": running });
            if json {
                println!("{v}")
            } else {
                println!("{}: {installed}, {}: {running}", t!("설치됨", "installed"), t!("실행 중", "running"))
            }
        }
        ServiceCmd::Run => service::run_dispatcher()?,
    }
    Ok(())
}

#[cfg(not(windows))]
fn service_cmd(action: ServiceCmd) -> Result<()> {
    match action {
        // 다른 OS 에서는 항상 "없음" (앱이 같은 명령으로 물어본다).
        ServiceCmd::Status { .. } => {
            println!("{}", serde_json::json!({ "installed": false, "running": false, "supported": false }));
            Ok(())
        }
        _ => bail!("{}", t!("서비스 모드는 Windows 전용입니다", "Service mode is Windows only")),
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
            bail!("{}", t!("비밀번호는 8자 이상이어야 합니다", "The password must be at least 8 characters"));
        }
        p
    };
    let mut cfg = HostConfig::load()?;
    cfg.display_password = Some(password.clone());
    cfg.save()?;
    if json {
        println!("{}", serde_json::json!({ "password": password }));
    } else {
        println!("{}: {password}", t!("표시 비밀번호", "Display password"));
    }
    Ok(())
}

fn set_access_code() -> Result<()> {
    let code = read_stdin_line()?;
    if code.chars().count() < MIN_ACCESS_CODE_LEN {
        bail!(
            "{}",
            match legato_core::i18n::lang() {
                legato_core::i18n::Lang::Ko => format!("고정 접속 코드는 {MIN_ACCESS_CODE_LEN}자 이상이어야 합니다"),
                legato_core::i18n::Lang::En =>
                    format!("The access code must be at least {MIN_ACCESS_CODE_LEN} characters"),
            }
        );
    }
    let mut cfg = HostConfig::load()?;
    if cfg.display_password.as_deref() == Some(code.as_str()) {
        bail!(
            "{}",
            t!(
                "고정 접속 코드는 표시 비밀번호와 달라야 합니다",
                "The access code must differ from the display password"
            )
        );
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

#[allow(clippy::too_many_arguments)]
fn serve(
    port: Option<u16>,
    bind: &str,
    max_width: u32,
    fps: u32,
    bitrate: u32,
    allow_sleep: bool,
    readonly: bool,
) -> Result<()> {
    let mut cfg = HostConfig::load()?;
    if !cfg.allow_display_password && cfg.access_code.is_none() {
        // 설정 파일을 직접 고친 경우. 아무도 못 들어오는 상태로는 열지 않는다.
        bail!(
            "{}",
            t!(
                "표시 비밀번호가 꺼져 있고 고정 접속 코드도 없어 접속할 방법이 없습니다",
                "The display password is off and there is no access code, so no one can connect"
            )
        );
    }
    // 표시 비밀번호: 없거나 매번 새로 만드는 설정이면 새로 만든다. 읽기 전용이면 저장하지 않는다.
    if cfg.allow_display_password && (cfg.display_password.is_none() || cfg.rotate_display_password) {
        cfg.display_password = Some(generate_password());
        if !readonly {
            cfg.save()?;
        }
    }
    let display = if cfg.allow_display_password {
        let pw = cfg.display_password.clone().expect("위에서 채움");
        eprintln!("{}: {pw}", t!("표시 비밀번호", "Display password"));
        events::emit("display_password", serde_json::json!({ "password": pw }));
        Some(PasswordRecord::new(&pw))
    } else {
        None
    };
    let port = port.unwrap_or(cfg.port);
    let id = if readonly {
        // 읽기 전용: 신원 파일이 없으면 만들지 않고 멈춘다 (root 소유 파일이 생기지 않게).
        let path = legato_core::paths::config_dir()?.join("identity.json");
        if !path.exists() {
            bail!(
                "{} ({})",
                t!(
                    "신원 파일이 없습니다. 먼저 앱에서 호스트를 한 번 켜 주세요",
                    "No identity file. Turn the host on once in the app first"
                ),
                path.display()
            );
        }
        load_identity()?
    } else {
        load_identity()?
    };
    let fp = id.fingerprint();
    if !platform::screen_capture_allowed() {
        tracing::warn!("화면 기록 권한이 없어. 접속하면 캡처가 실패할 거야 (legato-host request-permissions)");
    }

    let _sleep_guard = if allow_sleep { None } else { platform::prevent_sleep() };

    let bind = bind.to_string();
    platform::run_server(move || {
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
    })
}
