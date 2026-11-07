//! 로그인 화면(로그아웃 상태)에서도 접속을 받기 위한 LaunchAgent.
//!
//! `/Library/LaunchAgents` 에 `LimitLoadToSessionType = LoginWindow` 에이전트를 둔다. 로그인 화면이 떠 있는
//! 동안만 launchd 가 `legato-host serve` 를 돌리고, 사용자가 로그인하면 끝난다 (그때부터는 앱의 호스트가 받는다).
//! 이 에이전트는 root 로 돌기 때문에 사용자 설정 파일을 바꾸지 않게 읽기 전용(`--readonly-config`)으로 띄운다.
//! 설치·제거는 root 권한이 필요하다 (앱이 관리자 암호 창으로 실행).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const LABEL: &str = "com.hdomi.legato.loginwindow";

fn plist_path() -> PathBuf {
    PathBuf::from(format!("/Library/LaunchAgents/{LABEL}.plist"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn is_root() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() == 0 }
}

pub fn status() -> bool {
    plist_path().exists()
}

pub fn install(config_dir: &Path) -> Result<()> {
    if !is_root() {
        bail!("root 권한이 필요해 (sudo 로 실행하거나 앱에서 설치해줘)");
    }
    let exe = std::env::current_exe().context("실행 파일 경로를 알 수 없음")?;
    if !config_dir.join("identity.json").exists() || !config_dir.join("host.json").exists() {
        bail!("설정이 아직 없어. 먼저 앱에서 호스트를 한 번 켜고 고정 접속 코드를 정해줘");
    }
    let log_dir = PathBuf::from("/Library/Logs/Legato");
    std::fs::create_dir_all(&log_dir).context("로그 폴더 만들기 실패")?;
    let args = [
        exe.to_string_lossy().to_string(),
        "--config-dir".into(),
        config_dir.to_string_lossy().to_string(),
        "--log-file".into(),
        log_dir.join("loginwindow.log").to_string_lossy().to_string(),
        "serve".into(),
        "--readonly-config".into(),
    ];
    let args_xml: String = args.iter().map(|a| format!("    <string>{}</string>\n", xml_escape(a))).collect();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>LimitLoadToSessionType</key>
  <string>LoginWindow</string>
  <key>ProgramArguments</key>
  <array>
{args_xml}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#
    );
    let path = plist_path();
    std::fs::write(&path, plist).with_context(|| format!("{} 쓰기 실패", path.display()))?;
    // launchd 는 root 소유 · 644 가 아니면 읽지 않는다.
    let _ = std::process::Command::new("chown").args(["root:wheel"]).arg(&path).status();
    let _ = std::process::Command::new("chmod").args(["644"]).arg(&path).status();
    Ok(())
}

pub fn uninstall() -> Result<()> {
    if !is_root() {
        bail!("root 권한이 필요해 (sudo 로 실행하거나 앱에서 제거해줘)");
    }
    let path = plist_path();
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("{} 지우기 실패", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn xml_이스케이프() {
        assert_eq!(xml_escape(r#"/a b/<c>&"d""#), "/a b/&lt;c&gt;&amp;&quot;d&quot;");
    }
}
