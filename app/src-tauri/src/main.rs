//! Legato 데스크톱 앱. 주소록·설정 UI 를 보여주고, 실제 원격 기능은
//! 같은 폴더의 `legato-host` / `legato-viewer` 프로세스를 띄워서 처리한다.
//! 뷰어를 별도 프로세스로 두는 이유: 뷰어 창의 이벤트 루프가 Tauri 와 메인 스레드를 두고 충돌하지 않게,
//! 그리고 세션 하나가 죽어도 앱은 멀쩡하게.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// 실행 중인 호스트 프로세스.
#[derive(Default)]
struct HostProc(Mutex<Option<Child>>);

/// 같은 폴더에 있는 도우미 실행 파일 경로. 개발 중에는 cargo target 폴더, 배포본에서는 앱 번들 안.
fn helper(name: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("실행 파일 폴더를 찾을 수 없음")?;
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let path = dir.join(file);
    if path.exists() { Ok(path) } else { Err(format!("{} 이 없어. 먼저 빌드해줘", path.display())) }
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
    let out = no_window(Command::new(helper(name)?).args(args)).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().last().unwrap_or("{}");
    serde_json::from_str(line).map_err(|e| format!("출력 해석 실패: {e}: {line}"))
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
fn pipe_lines(app: AppHandle, reader: impl std::io::Read + Send + 'static, event: &'static str, tag: String) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let payload = match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(v) if v.get("event").is_some() => serde_json::json!({ "tag": tag, "kind": "event", "value": v }),
                // 색상 코드는 지우고 사람용 로그로 보낸다.
                _ => serde_json::json!({ "tag": tag, "kind": "log", "value": strip_ansi(&line) }),
            };
            let _ = app.emit(event, payload);
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
        Command::new(helper("legato-host")?)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .spawn()
    .map_err(|e| format!("호스트 실행 실패: {e}"))?;
    pipe_lines(app.clone(), child.stdout.take().expect("piped"), "host", "stdout".into());
    pipe_lines(app, child.stderr.take().expect("piped"), "host", "stderr".into());
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

/// 새 비밀번호를 만든다. 호스트가 켜져 있으면 다시 시작해야 적용된다.
#[tauri::command]
fn host_reset_password() -> Result<String, String> {
    let v = run_json("legato-host", &["set-password", "--generate", "--json"])?;
    v.get("password").and_then(|p| p.as_str()).map(str::to_string).ok_or("비밀번호 생성 실패".into())
}

#[tauri::command]
fn host_request_permissions() -> Result<serde_json::Value, String> {
    run_json("legato-host", &["request-permissions"])
}

/// 뷰어를 띄워 원격 컴퓨터에 접속한다. 비밀번호는 명령줄 대신 환경 변수로 넘긴다 (ps 에 안 보이게).
#[tauri::command]
fn connect(app: AppHandle, id: String, address: String, password: String, windowed: bool, accept_new_fingerprint: bool) -> Result<(), String> {
    let mut cmd = Command::new(helper("legato-viewer")?);
    cmd.arg(&address)
        .env("LEGATO_PASSWORD", password)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if windowed {
        cmd.arg("--windowed");
    }
    if accept_new_fingerprint {
        cmd.arg("--accept-new-fingerprint");
    }
    let mut child = no_window(&mut cmd).spawn().map_err(|e| format!("뷰어 실행 실패: {e}"))?;
    pipe_lines(app.clone(), child.stdout.take().expect("piped"), "viewer", id.clone());
    pipe_lines(app, child.stderr.take().expect("piped"), "viewer", id);
    // 종료 코드를 거둬 좀비 프로세스가 남지 않게 한다.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 주소록 항목. 비밀번호는 저장하지 않는다.
#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    name: String,
    address: String,
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
        .manage(HostProc::default())
        .invoke_handler(tauri::generate_handler![
            host_info,
            host_running,
            host_start,
            host_stop,
            host_reset_password,
            host_request_permissions,
            connect,
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
