//! Legato 데스크톱 앱. 주소록·설정 UI 를 보여주고, 실제 원격 기능은
//! 같은 폴더의 `legato-host` / `legato-viewer` 프로세스를 띄워서 처리한다.
//! 뷰어를 별도 프로세스로 두는 이유: 뷰어 창의 이벤트 루프가 Tauri 와 메인 스레드를 두고 충돌하지 않게,
//! 그리고 세션 하나가 죽어도 앱은 멀쩡하게.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

mod keychain;
mod update;

/// 이 컴퓨터(호스트)에 접속해 있는 원격 세션 수. 업데이트 보류 판단용.
static HOST_SESSIONS: AtomicUsize = AtomicUsize::new(0);
/// 실행 중인 뷰어(다른 컴퓨터에 접속 중) 수.
static VIEWERS: AtomicUsize = AtomicUsize::new(0);

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// 실행 중인 호스트 프로세스.
#[derive(Default)]
struct HostProc(Mutex<Option<Child>>);

/// 같은 폴더에 있는 도우미 실행 파일 경로. 개발 중에는 cargo target 폴더, 배포본에서는 앱 번들 안.
fn helper(name: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or_else(|| tr("실행 파일 폴더를 찾을 수 없습니다", "Can't find the app folder"))?;
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let path = dir.join(file);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "{}: {}",
            tr(
                "필요한 파일이 없습니다. 앱을 다시 설치해 주세요",
                "A required file is missing. Please reinstall the app"
            ),
            path.display()
        ))
    }
}

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 콘솔 창이 깜빡이지 않게.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// 도우미를 실행하고 표준 출력 JSON 을 돌려준다.
fn run_json(name: &str, args: &[&str]) -> Result<serde_json::Value, String> {
    let out = no_window(helper_cmd(name)?.args(args)).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().last().unwrap_or("{}");
    serde_json::from_str(line).map_err(|e| format!("출력 해석 실패: {e}: {line}"))
}

/// 도우미를 실행하면서 표준 입력으로 비밀 값을 넘긴다 (명령줄 인자는 ps 에 보이므로 쓰지 않는다).
fn run_json_stdin(name: &str, args: &[&str], input: &str) -> Result<serde_json::Value, String> {
    use std::io::Write;
    let mut child =
        no_window(helper_cmd(name)?.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()))
            .spawn()
            .map_err(|e| e.to_string())?;
    child.stdin.take().expect("piped").write_all(format!("{input}\n").as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("Error: ").to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.lines().last().unwrap_or("{}")).map_err(|e| e.to_string())
}

#[tauri::command]
fn host_info() -> Result<serde_json::Value, String> {
    run_json("legato-host", &["info", "--json"])
}

#[tauri::command]
fn host_running(proc: State<HostProc>) -> bool {
    let mut g = proc.0.lock().expect("뮤텍스");
    match g.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            *g = None;
            false
        }
        None => false,
    }
}

/// 자식 프로세스의 출력 한 줄씩을 프론트로 보낸다.
/// 뷰어가 인증에 성공했을 때 `connected` 이벤트 데이터로 한 번 불리는 콜백.
type OnConnected = Box<dyn FnOnce(&serde_json::Value) + Send>;

/// `on_connected` 는 뷰어가 인증까지 성공했을 때 한 번 불린다.
fn pipe_lines(
    app: AppHandle,
    reader: impl std::io::Read + Send + 'static,
    event: &'static str,
    tag: String,
    mut on_connected: Option<OnConnected>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let payload = match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(v) if v.get("event").is_some() => {
                    if event == "host" {
                        match v["event"].as_str() {
                            Some("session_started") => {
                                HOST_SESSIONS.fetch_add(1, Ordering::SeqCst);
                            }
                            Some("session_ended") => {
                                let mut cur = HOST_SESSIONS.load(Ordering::SeqCst);
                                // 0 아래로 내려가지 않게 비교-교환으로 줄인다.
                                while let Err(actual) = HOST_SESSIONS.compare_exchange(
                                    cur,
                                    cur.saturating_sub(1),
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                ) {
                                    cur = actual;
                                }
                            }
                            _ => {}
                        }
                    }
                    if v["event"] == "connected"
                        && let Some(f) = on_connected.take()
                    {
                        f(&v["data"]);
                    }
                    serde_json::json!({ "tag": tag, "kind": "event", "value": v })
                }
                // 색상 코드는 지우고 사람용 로그로 보낸다.
                _ => serde_json::json!({ "tag": tag, "kind": "log", "value": strip_ansi(&line) }),
            };
            let _ = app.emit(event, payload);
        }
        if event == "host" && tag == "stdout" {
            HOST_SESSIONS.store(0, Ordering::SeqCst);
        }
        let _ = app.emit(event, serde_json::json!({ "tag": tag, "kind": "exit" }));
    });
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[tauri::command]
fn host_start(app: AppHandle, proc: State<HostProc>) -> Result<(), String> {
    let mut g = proc.0.lock().expect("뮤텍스");
    if let Some(c) = g.as_mut()
        && matches!(c.try_wait(), Ok(None))
    {
        return Ok(());
    }
    let mut child = no_window(
        helper_cmd("legato-host")?.arg("serve").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()),
    )
    .spawn()
    .map_err(|e| format!("{}: {e}", tr("호스트를 실행하지 못했습니다", "Couldn't start the host")))?;
    pipe_lines(app.clone(), child.stdout.take().expect("piped"), "host", "stdout".into(), None);
    pipe_lines(app, child.stderr.take().expect("piped"), "host", "stderr".into(), None);
    *g = Some(child);
    Ok(())
}

#[tauri::command]
fn host_stop(proc: State<HostProc>) {
    if let Some(mut c) = proc.0.lock().expect("뮤텍스").take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// 표시 비밀번호를 새로 만든다. 호스트가 켜져 있으면 다시 시작해야 적용된다.
#[tauri::command]
fn host_reset_password() -> Result<String, String> {
    let v = run_json("legato-host", &["set-password", "--generate", "--json"])?;
    v.get("password")
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .ok_or_else(|| tr("비밀번호를 만들지 못했습니다", "Couldn't create a password"))
}

/// 고정 접속 코드를 설정한다. 호스트에는 해시만 저장된다.
#[tauri::command]
fn host_set_access_code(code: String) -> Result<(), String> {
    run_json_stdin("legato-host", &["set-access-code"], &code).map(|_| ())
}

#[tauri::command]
fn host_clear_access_code() -> Result<(), String> {
    run_json("legato-host", &["clear-access-code"]).map(|_| ())
}

#[tauri::command]
fn host_configure(rotate: Option<bool>, allow_display: Option<bool>) -> Result<(), String> {
    let mut args = vec!["configure".to_string()];
    if let Some(v) = rotate {
        args.push(format!("--rotate-display-password={v}"));
    }
    if let Some(v) = allow_display {
        args.push(format!("--allow-display-password={v}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_json("legato-host", &refs).map(|_| ())
}

/// Windows 서비스 모드 상태 (다른 OS 는 supported=false).
#[tauri::command]
fn service_status() -> Result<serde_json::Value, String> {
    run_json("legato-host", &["service", "status", "--json"])
}

/// 서비스 설치·제거. 관리자 권한이 필요해서 UAC 확인 창을 띄워 실행한다.
#[tauri::command]
async fn service_change(install: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = helper("legato-host")?;
        let dir = legato_core::paths::config_dir().map_err(|e| e.to_string())?;
        let action = if install { "install" } else { "uninstall" };
        // PowerShell 문자열은 작은따옴표 안에서 ' 를 '' 로 쓴다.
        let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let args = format!("@({}, {}, 'service', {})", q("--config-dir"), q(&dir.to_string_lossy()), q(action));
        let script = format!(
            "$p = Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode",
            q(&exe.to_string_lossy()),
            args
        );
        let status = tauri::async_runtime::spawn_blocking(move || {
            no_window(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script])).status()
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(tr(
                "서비스를 바꾸지 못했습니다 (관리자 권한을 거부했거나 오류가 났습니다)",
                "Couldn't change the service (administrator permission was denied or an error occurred)",
            ))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = install;
        Err(tr("서비스 모드는 Windows 전용입니다", "Service mode is Windows only"))
    }
}

#[tauri::command]
fn login_agent_status() -> Result<serde_json::Value, String> {
    run_json("legato-host", &["login-agent", "status", "--json"])
}

/// macOS 로그인 화면 에이전트 설치·제거. root 권한이 필요해서 관리자 암호 창(osascript)으로 실행한다.
#[tauri::command]
async fn login_agent_change(install: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let exe = helper("legato-host")?;
        let dir = legato_core::paths::config_dir().map_err(|e| e.to_string())?;
        // 셸 작은따옴표 → AppleScript 문자열 순서로 감싼다.
        let sh = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let cmd = format!(
            "{} --config-dir {} login-agent {}",
            sh(&exe.to_string_lossy()),
            sh(&dir.to_string_lossy()),
            if install { "install" } else { "uninstall" }
        );
        let apple = format!(
            "do shell script \"{}\" with administrator privileges with prompt \"{}\"",
            cmd.replace('\\', "\\\\").replace('"', "\\\""),
            if load_settings().lang == "en" {
                "Legato wants to change the login-window access setting."
            } else {
                "Legato 가 로그인 화면 접속 설정을 바꾸려고 합니다."
            }
        );
        let out = tauri::async_runtime::spawn_blocking(move || Command::new("osascript").args(["-e", &apple]).output())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.contains("-128") {
                tr("취소했습니다", "Cancelled")
            } else {
                format!("{}: {err}", tr("설정을 바꾸지 못했습니다", "Couldn't change the setting"))
            })
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = install;
        Err(tr("로그인 화면 접속은 macOS 전용입니다", "Login-window access is macOS only"))
    }
}

#[tauri::command]
fn host_request_permissions() -> Result<serde_json::Value, String> {
    run_json("legato-host", &["request-permissions"])
}

fn saved_code(address: &str) -> Result<Option<String>, String> {
    keychain::get(address).map_err(|e| format!("{}: {e}", tr("키체인을 읽지 못했습니다", "Couldn't read the keychain")))
}

#[tauri::command]
fn forget_code(address: String) -> Result<(), String> {
    keychain::delete(&address)
}

/// 뷰어를 띄워 원격 컴퓨터에 접속한다. 비밀번호는 명령줄 대신 환경 변수로 넘긴다 (ps 에 안 보이게).
/// - `use_saved`: 키체인에 저장한 접속 코드로 접속 (입력값 무시)
/// - `remember`: 접속에 성공하면 입력값을 키체인에 저장 (고정 접속 코드용)
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn connect(
    app: AppHandle,
    id: String,
    address: String,
    password: String,
    use_saved: bool,
    remember: bool,
    windowed: bool,
    accept_new_fingerprint: bool,
) -> Result<(), String> {
    let secret = if use_saved {
        saved_code(&address)?.ok_or_else(|| tr("저장된 접속 코드가 없습니다", "No saved access code"))?
    } else {
        password
    };
    let mut cmd = helper_cmd("legato-viewer")?;
    cmd.arg(&address)
        .env("LEGATO_PASSWORD", &secret)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if windowed {
        cmd.arg("--windowed");
    }
    if let Some(v) = load_settings().caps_hangul {
        cmd.arg(format!("--caps-hangul={v}"));
    }
    if accept_new_fingerprint {
        cmd.arg("--accept-new-fingerprint");
    }
    let mut child = no_window(&mut cmd)
        .spawn()
        .map_err(|e| format!("{}: {e}", tr("뷰어를 실행하지 못했습니다", "Couldn't start the viewer")))?;
    // 고정 접속 코드로 인증에 성공한 경우에만 저장한다 (틀린 값이나 매번 바뀌는 표시 비밀번호는 저장하지 않게).
    let on_connected: Option<OnConnected> = if remember && !use_saved {
        let (app2, addr, id2) = (app.clone(), address.clone(), id.clone());
        Some(Box::new(move |data: &serde_json::Value| {
            if data["via_access_code"] != true {
                let _ = app2.emit(
                    "code-saved",
                    serde_json::json!({ "tag": id2, "address": addr, "error": tr("표시 비밀번호로 접속해 저장하지 않았습니다 (고정 접속 코드만 저장합니다)", "Not saved because you connected with the display password (only access codes are saved)") }),
                );
                return;
            }
            let result = keychain::set(&addr, &secret);
            let _ = app2.emit("code-saved", serde_json::json!({ "tag": id2, "address": addr, "error": result.err() }));
        }))
    } else {
        None
    };
    pipe_lines(app.clone(), child.stdout.take().expect("piped"), "viewer", id.clone(), on_connected);
    pipe_lines(app, child.stderr.take().expect("piped"), "viewer", id, None);
    // 종료 코드를 거둬 좀비 프로세스가 남지 않게 한다.
    VIEWERS.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _ = child.wait();
        VIEWERS.fetch_sub(1, Ordering::SeqCst);
    });
    Ok(())
}

/// 앱 버전과 서명 상태 (설정 탭 표시용).
#[tauri::command]
fn app_info(app: AppHandle) -> serde_json::Value {
    serde_json::json!({
        "version": app.package_info().version.to_string(),
        // macOS 만 의미 있음: ad-hoc 서명이면 업데이트마다 권한이 초기화된다.
        "adhoc": cfg!(target_os = "macos") && update::current_is_adhoc(),
    })
}

#[tauri::command]
async fn check_update(app: AppHandle) -> Result<update::UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || update::check(&current)).await.map_err(|e| e.to_string())?
}

/// 업데이트를 받아 설치한다. 원격 세션이 있으면 `force` 없이는 미룬다 (접속 중에 끊기지 않게).
#[tauri::command]
async fn install_update(app: AppHandle, version: String, force: bool) -> Result<(), String> {
    let (hosted, viewing) = (HOST_SESSIONS.load(Ordering::SeqCst), VIEWERS.load(Ordering::SeqCst));
    if !force && (hosted > 0 || viewing > 0) {
        return Err(format!(
            "BUSY: {}",
            if load_settings().lang == "en" {
                format!("Remote sessions are open (connected to this computer: {hosted}, you are viewing: {viewing})")
            } else {
                format!("원격 세션이 열려 있습니다 (이 컴퓨터에 접속 {hosted}, 내가 접속 중 {viewing})")
            }
        ));
    }
    let resume_host = host_running(app.state::<HostProc>());
    tauri::async_runtime::spawn_blocking(move || update::install(&version, resume_host))
        .await
        .map_err(|e| e.to_string())??;
    // 설치 스크립트가 앱 종료를 기다린다. 호스트를 먼저 끄고 앱을 끝낸다.
    if let Some(mut c) = app.state::<HostProc>().0.lock().expect("뮤텍스").take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let h = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        h.exit(0);
    });
    Ok(())
}

/// 앱 설정 (`app.json`).
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct AppSettings {
    /// 앱이 켜지면 호스트도 자동으로 켠다 (회사에서 집 Mac 에 접속하는 용도).
    auto_host: bool,
    /// 화면 언어 (`ko` / `en`). 호스트·뷰어에도 `LEGATO_LANG` 으로 넘긴다.
    lang: String,
    /// 앱을 켤 때와 6시간마다 새 버전을 확인한다.
    auto_update_check: bool,
    /// 뷰어에서 Caps Lock 을 원격 한/영 키로 쓸지. 없으면 OS 기본값 (macOS 켬).
    caps_hangul: Option<bool>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { auto_host: false, lang: "ko".into(), auto_update_check: true, caps_hangul: None }
    }
}

fn save_settings(s: &AppSettings) -> Result<(), String> {
    let data = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    legato_core::paths::write_private(&settings_path()?, data.as_bytes()).map_err(|e| e.to_string())
}

/// 앱 화면 언어로 문구를 고른다 (`app.json` 의 lang).
pub(crate) fn tr(ko: &str, en: &str) -> String {
    if load_settings().lang == "en" { en.to_string() } else { ko.to_string() }
}

/// 도우미 실행 파일을 띄울 명령. 앱 언어를 넘겨 호스트·뷰어 문구도 같은 언어로 나오게 한다.
fn helper_cmd(name: &str) -> Result<Command, String> {
    let mut cmd = Command::new(helper(name)?);
    cmd.env("LEGATO_LANG", load_settings().lang);
    Ok(cmd)
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(legato_core::paths::config_dir().map_err(|e| e.to_string())?.join("app.json"))
}

fn load_settings() -> AppSettings {
    settings_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn get_settings(app: AppHandle) -> serde_json::Value {
    use tauri_plugin_autostart::ManagerExt;
    let s = load_settings();
    serde_json::json!({
        "auto_host": s.auto_host,
        "launch_at_login": app.autolaunch().is_enabled().unwrap_or(false),
        "lang": s.lang,
        "auto_update_check": s.auto_update_check,
        "caps_hangul": s.caps_hangul.unwrap_or(cfg!(target_os = "macos")),
        "os": if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "other" },
    })
}

#[tauri::command]
fn set_settings(
    app: AppHandle,
    auto_host: Option<bool>,
    launch_at_login: Option<bool>,
    lang: Option<String>,
    auto_update_check: Option<bool>,
    caps_hangul: Option<bool>,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mut s = load_settings();
    if let Some(v) = auto_host {
        s.auto_host = v;
    }
    if let Some(v) = lang {
        s.lang = if v.starts_with("en") { "en".into() } else { "ko".into() };
    }
    if let Some(v) = auto_update_check {
        s.auto_update_check = v;
    }
    if let Some(v) = caps_hangul {
        s.caps_hangul = Some(v);
    }
    save_settings(&s)?;
    if let Some(v) = launch_at_login {
        let al = app.autolaunch();
        if v { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 주소록 항목. 비밀 값은 여기 저장하지 않는다 (키체인에만).
/// `saved_code` 는 키체인에 접속 코드를 넣어 뒀는지 표시만 한다. 앱을 켤 때 키체인을 읽으면
/// 서명이 바뀐 직후 macOS 가 키체인 접근 확인 창을 띄우므로, 실제로 접속할 때만 읽는다.
#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    name: String,
    address: String,
    #[serde(default)]
    saved_code: bool,
}

fn book_path() -> Result<PathBuf, String> {
    Ok(legato_core::paths::config_dir().map_err(|e| e.to_string())?.join("addressbook.json"))
}

#[tauri::command]
fn load_addresses() -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(book_path()?) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
        Err(_) => Ok(vec![]),
    }
}

#[tauri::command]
fn save_addresses(entries: Vec<Entry>) -> Result<(), String> {
    let data = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    legato_core::paths::write_private(&book_path()?, data.as_bytes()).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        // 로그인 시 실행되면 `--autostart` 인자가 붙는다 (창을 최소화해서 시작).
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(HostProc::default())
        .setup(|app| {
            // 업데이트 직후 다시 열렸으면 그 전에 켜져 있던 호스트를 다시 켠다.
            let resume = std::env::args().any(|a| a == "--resume-host");
            if load_settings().auto_host || resume {
                let handle = app.handle().clone();
                if let Err(e) = host_start(handle.clone(), handle.state::<HostProc>()) {
                    eprintln!("호스트 자동 시작 실패: {e}");
                }
            }
            if std::env::args().any(|a| a == "--autostart")
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.minimize();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host_info,
            host_running,
            host_start,
            host_stop,
            host_reset_password,
            host_set_access_code,
            host_clear_access_code,
            host_configure,
            host_request_permissions,
            service_status,
            service_change,
            login_agent_status,
            login_agent_change,
            connect,
            forget_code,
            get_settings,
            set_settings,
            app_info,
            check_update,
            install_update,
            load_addresses,
            save_addresses
        ])
        .on_window_event(|window, event| {
            // 앱을 닫으면 호스트도 같이 끈다.
            if let tauri::WindowEvent::Destroyed = event
                && let Some(mut c) = window.app_handle().state::<HostProc>().0.lock().expect("뮤텍스").take()
            {
                let _ = c.kill();
            }
        })
        .run(tauri::generate_context!())
        .expect("Legato 앱 실행 실패");
}
