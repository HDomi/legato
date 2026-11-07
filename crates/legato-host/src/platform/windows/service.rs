//! Windows 서비스 모드.
//!
//! LocalSystem 서비스가 지금 콘솔 세션에 SYSTEM 토큰으로 호스트 헬퍼(`legato-host serve`)를 띄운다.
//! 헬퍼가 SYSTEM 이라서 다음이 된다:
//! - 보안 데스크톱(UAC 확인 창, 잠금 화면, 로그인 화면) 캡처와 입력
//! - 관리자 권한으로 실행된 창에 입력 (UIPI 제한 없음)
//! - Ctrl+Alt+Del (SendSAS, SoftwareSASGeneration 정책 필요)
//!
//! 세션이 바뀌거나(로그인·사용자 전환) 헬퍼가 끝나면 새 세션에 다시 띄운다.

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
    let exe = std::env::current_exe().context("실행 파일 경로를 알 수 없음")?;
    let log = config_dir.join("service.log");
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("서비스 관리자를 열 수 없어 (관리자 권한으로 실행해줘)")?;
    // 이미 있으면 지우고 다시 만든다 (경로·인자가 바뀌었을 수 있음).
    if let Ok(old) =
        manager.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS)
    {
        let _ = old.stop();
        let _ = old.delete();
        std::thread::sleep(Duration::from_secs(1));
    }
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
    let _ =
        service.set_description("Legato 원격 접속을 받는다. 로그인 화면 · UAC 확인 창까지 원격으로 볼 수 있게 한다.");

    // Ctrl+Alt+Del(SendSAS)을 서비스가 보낼 수 있게 허용 (1 = 서비스).
    let _ = std::process::Command::new("reg")
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
    // 방화벽: 이 실행 파일의 UDP 수신 허용.
    let _ = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")])
        .status();
    let _ = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={FIREWALL_RULE}"),
            "dir=in",
            "action=allow",
            "protocol=UDP",
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
        .context("서비스 관리자를 열 수 없어 (관리자 권한으로 실행해줘)")?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS)
        .context("서비스가 설치돼 있지 않거나 권한이 없어")?;
    let _ = service.stop();
    service.delete().context("서비스 지우기 실패")?;
    let _ = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")])
        .status();
    Ok(())
}

/// (설치됨, 실행 중)
pub fn status() -> (bool, bool) {
    let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) else {
        return (false, false);
    };
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(s) => (true, s.query_status().is_ok_and(|st| st.current_state == ServiceState::Running)),
        Err(_) => (false, false),
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
    let log_dir = legato_core::paths::config_dir()?;
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
        for a in [OsStr::new("--log-file"), log.as_os_str(), OsStr::new("serve")] {
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
