//! Windows 서비스 모드.
//!
//! LocalSystem 서비스가 지금 콘솔 세션에 SYSTEM 토큰으로 호스트 헬퍼(`legato-host serve`)를 띄운다.
//! 헬퍼가 SYSTEM 이라서 다음이 된다:
//! - 보안 데스크톱(UAC 확인 창, 잠금 화면, 로그인 화면) 캡처와 입력
//! - 관리자 권한으로 실행된 창에 입력 (UIPI 제한 없음)
//! - Ctrl+Alt+Del (SendSAS, SoftwareSASGeneration 정책 필요)
//!
//! 세션이 바뀌거나(로그인·사용자 전환) 헬퍼가 끝나면 새 세션에 다시 띄운다.
//!
//! SYSTEM 으로 도는 프로세스가 사용자가 쓸 수 있는 파일을 실행하거나 쓰면 권한 상승 통로가 된다. 그래서
//! - 실행 파일은 관리자만 쓸 수 있는 `Program Files\Legato\service` 로 복사해서 실행하고
//! - 로그는 관리자·SYSTEM 만 접근할 수 있는 `ProgramData\Legato\logs` 에 쓰고
//! - 사용자 설정 폴더는 읽기만 한다 (`serve --readonly-config`).
//!
//! 복사본이라 앱을 업데이트하면 다시 설치해야 한다. `status` 가 원본과 비교해 `stale` 을 알려 준다.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityIdentification, SetTokenInformation, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT,
    TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary, TokenSessionId,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
    ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub const SERVICE_NAME: &str = "LegatoHost";
const DISPLAY_NAME: &str = "Legato 원격 호스트";
const FIREWALL_RULE: &str = "Legato 호스트 (UDP)";

/// Windows 폴더 (`C:\Windows`). 환경 변수는 바꿔치기될 수 있어 API 로 얻는다.
fn windows_dir() -> PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return PathBuf::from(r"C:\Windows");
    }
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// 시스템 드라이브 루트 (`C:\`).
fn system_root() -> PathBuf {
    windows_dir().ancestors().last().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(r"C:\"))
}

fn system_tool(name: &str) -> PathBuf {
    windows_dir().join("System32").join(name)
}

/// 복사한 서비스 실행 파일 위치.
pub fn service_exe() -> PathBuf {
    system_root().join("Program Files").join("Legato").join("service").join("legato-host.exe")
}

/// 서비스 로그 폴더.
fn log_dir() -> PathBuf {
    system_root().join("ProgramData").join("Legato").join("logs")
}

/// 폴더를 만들고 관리자(S-1-5-32-544)·SYSTEM(S-1-5-18)만 접근하게 한다.
/// 이미 있던 폴더(다른 사용자가 미리 만들어 둔 경우 포함)도 소유자와 권한을 다시 정한다.
fn admin_only_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("{} 만들기 실패", path.display()))?;
    let icacls = system_tool("icacls.exe");
    let ok = std::process::Command::new(&icacls)
        .arg(path)
        .args(["/setowner", "*S-1-5-32-544", "/T", "/C", "/Q"])
        .status()
        .is_ok_and(|s| s.success())
        && std::process::Command::new(&icacls)
            .arg(path)
            .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/T", "/C", "/Q"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        bail!("{} 권한 설정 실패", path.display());
    }
    Ok(())
}

/// 두 파일 내용이 같은지.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// 명령줄 인자 하나를 Windows 규칙대로 따옴표로 감싼다.
fn quote(arg: &OsStr) -> OsString {
    let s = arg.to_string_lossy();
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return arg.to_os_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out.into()
}

/// 서비스를 설치하고 시작한다 (관리자 권한 필요).
pub fn install(config_dir: &Path, port: u16) -> Result<()> {
    let src = std::env::current_exe().context("실행 파일 경로를 알 수 없음")?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("서비스 관리자를 열 수 없습니다 (관리자 권한으로 실행해 주세요)")?;
    // 이미 있으면 지우고 다시 만든다 (경로·인자가 바뀌었을 수 있음). 실행 파일을 덮어쓰려면 먼저 멈춰야 한다.
    if let Ok(old) =
        manager.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS)
    {
        let _ = old.stop();
        for _ in 0..20 {
            if old.query_status().is_ok_and(|st| st.current_state == ServiceState::Stopped) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = old.delete();
        std::thread::sleep(Duration::from_secs(1));
    }

    // 실행 파일을 관리자 전용 폴더로 복사한다.
    let exe = service_exe();
    let exe_dir = exe.parent().context("서비스 폴더")?;
    admin_only_dir(exe_dir)?;
    // 같은 파일을 실행 중일 수 있어 임시 이름으로 복사한 뒤 바꾼다.
    let tmp = exe.with_extension("exe.new");
    std::fs::copy(&src, &tmp).with_context(|| format!("{} 복사 실패", tmp.display()))?;
    std::fs::rename(&tmp, &exe).with_context(|| format!("{} 교체 실패", exe.display()))?;
    let logs = log_dir();
    admin_only_dir(&logs)?;
    let log = logs.join("service.log");
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: vec![
            "--log-file".into(),
            log.into_os_string(),
            "--config-dir".into(),
            config_dir.as_os_str().to_os_string(),
            "service".into(),
            "run".into(),
        ],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS)
        .context("서비스 만들기 실패")?;
    let _ = service.set_description(
        "Legato 원격 접속을 받습니다. 로그인 화면과 UAC 확인 창까지 원격으로 볼 수 있습니다. / Accepts Legato remote connections, including the login screen and UAC prompts.",
    );

    // Ctrl+Alt+Del(SendSAS)을 서비스가 보낼 수 있게 허용 (1 = 서비스).
    let _ = std::process::Command::new(system_tool("reg.exe"))
        .args([
            "add",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System",
            "/v",
            "SoftwareSASGeneration",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .status();
    // 방화벽: 이 실행 파일의 UDP 수신 허용. 공용 네트워크(카페 등)에서는 열지 않는다.
    let netsh = system_tool("netsh.exe");
    let _ = std::process::Command::new(&netsh)
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")])
        .status();
    let _ = std::process::Command::new(&netsh)
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={FIREWALL_RULE}"),
            "dir=in",
            "action=allow",
            "protocol=UDP",
            "profile=private,domain",
            &format!("localport={port}"),
            &format!("program={}", exe.display()),
        ])
        .status();

    service.start(&[] as &[&OsStr]).context("서비스 시작 실패")?;
    Ok(())
}

/// 서비스를 멈추고 지운다 (관리자 권한 필요).
pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("서비스 관리자를 열 수 없습니다 (관리자 권한으로 실행해 주세요)")?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS)
        .context("서비스가 설치되어 있지 않거나 권한이 없습니다")?;
    let _ = service.stop();
    for _ in 0..20 {
        if service.query_status().is_ok_and(|st| st.current_state == ServiceState::Stopped) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    service.delete().context("서비스 지우기 실패")?;
    let _ = std::process::Command::new(system_tool("netsh.exe"))
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")])
        .status();
    let _ = std::fs::remove_file(service_exe());
    Ok(())
}

/// (설치됨, 실행 중, 복사한 실행 파일이 지금 앱 것과 달라 다시 설치해야 함)
pub fn status() -> (bool, bool, bool) {
    let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) else {
        return (false, false, false);
    };
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(s) => {
            let running = s.query_status().is_ok_and(|st| st.current_state == ServiceState::Running);
            let stale = std::env::current_exe().map(|me| !same_file(&me, &service_exe())).unwrap_or(true);
            (true, running, stale)
        }
        Err(_) => (false, false, false),
    }
}

define_windows_service!(ffi_service_main, service_main);

/// 서비스 관리자가 부르는 진입점으로 넘긴다. `service run` 에서만 쓴다.
pub fn run_dispatcher() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("서비스 디스패처 시작 실패 (서비스 관리자가 실행해야 함)")?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("서비스 종료: {e:#}");
    }
}

fn set_state(handle: &service_control_handler::ServiceStatusHandle, state: ServiceState, accept: ServiceControlAccept) {
    let _ = handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    });
}

/// 실행 중인 헬퍼 프로세스.
struct Helper {
    process: HANDLE,
    thread: HANDLE,
    session: u32,
}

impl Helper {
    fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        unsafe {
            let _ = TerminateProcess(self.process, 0);
            let _ = WaitForSingleObject(self.process, 3000);
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.thread);
        }
    }
}

fn run_service() -> Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    set_state(&handle, ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN);
    tracing::info!("서비스 시작");

    let exe = std::env::current_exe()?;
    let log_dir = log_dir();
    let mut helper: Option<Helper> = None;
    let mut backoff = Duration::from_secs(1);
    loop {
        if stop_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            break;
        }
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == u32::MAX {
            // 콘솔 세션 전환 중.
            continue;
        }
        if let Some(h) = &helper
            && (h.session != session || h.exited())
        {
            tracing::info!("헬퍼 다시 시작 (세션 {} → {session}, 끝남 {})", h.session, h.exited());
            helper = None;
        }
        if helper.is_none() {
            match spawn_helper(&exe, &log_dir, session) {
                Ok(h) => {
                    tracing::info!("세션 {session} 에 헬퍼 시작");
                    helper = Some(h);
                    backoff = Duration::from_secs(1);
                }
                Err(e) => {
                    tracing::warn!("헬퍼 시작 실패: {e:#}");
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
    drop(helper);
    set_state(&handle, ServiceState::Stopped, ServiceControlAccept::empty());
    tracing::info!("서비스 멈춤");
    Ok(())
}

/// 이 서비스(SYSTEM)의 토큰을 복제해 세션 번호만 바꾼 뒤, 그 세션의 기본 데스크톱에 헬퍼를 띄운다.
fn spawn_helper(exe: &Path, log_dir: &Path, session: u32) -> Result<Helper> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_SESSIONID | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut token,
        )
        .context("서비스 토큰 열기 실패")?;
        let mut primary = HANDLE::default();
        let dup = DuplicateTokenEx(
            token,
            TOKEN_ACCESS_MASK(0x0200_0000),
            None,
            SecurityIdentification,
            TokenPrimary,
            &mut primary,
        );
        let _ = CloseHandle(token);
        dup.context("토큰 복제 실패")?;
        let sid = session;
        let set = SetTokenInformation(primary, TokenSessionId, &sid as *const u32 as *const _, 4);
        if let Err(e) = set {
            let _ = CloseHandle(primary);
            bail!("토큰 세션 지정 실패: {e}");
        }

        let log: PathBuf = log_dir.join("host-helper.log");
        let mut cmd = quote(exe.as_os_str());
        // 사용자 설정 폴더에는 쓰지 않는다 (SYSTEM 이 사용자 폴더에 쓰면 링크로 다른 파일을 덮어쓰게 만들 수 있음).
        for a in [OsStr::new("--log-file"), log.as_os_str(), OsStr::new("serve"), OsStr::new("--readonly-config")] {
            cmd.push(" ");
            cmd.push(quote(a));
        }
        let mut cmdline = wide(&cmd);
        let mut desktop = wide(OsStr::new("winsta0\\default"));
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        // 환경 변수(LEGATO_CONFIG_DIR 포함)는 서비스 프로세스 것을 물려준다.
        let r = CreateProcessAsUserW(
            Some(primary),
            PCWSTR::null(),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(primary);
        r.context("헬퍼 프로세스 만들기 실패")?;
        Ok(Helper { process: pi.hProcess, thread: pi.hThread, session })
    }
}

#[cfg(test)]
mod tests {
    use super::quote;
    use std::ffi::OsStr;

    #[test]
    fn 인자_따옴표() {
        assert_eq!(quote(OsStr::new("serve")), "serve");
        assert_eq!(
            quote(OsStr::new(r"C:\Program Files\Legato\legato-host.exe")),
            r#""C:\Program Files\Legato\legato-host.exe""#
        );
        assert_eq!(quote(OsStr::new(r"C:\a b\")), r#""C:\a b\\""#);
    }
}
