//! 로그인 화면(로그아웃 상태)에서도 접속을 받기 위한 LaunchAgent.
//!
//! `/Library/LaunchAgents` 에 `LimitLoadToSessionType = LoginWindow` 에이전트를 둔다. 로그인 화면이 떠 있는
//! 동안만 launchd 가 `legato-host serve` 를 돌리고, 사용자가 로그인하면 끝난다 (그때부터는 앱의 호스트가 받는다).
//!
//! 에이전트는 root 로 돌기 때문에, 사용자가 쓸 수 있는 파일을 실행하거나 읽으면 사용자 권한의 악성 코드가
//! root 권한을 얻을 수 있다. 그래서 설치할 때
//! - 실행 파일을 root 만 쓸 수 있는 `/Library/PrivilegedHelperTools` 로 복사하고
//! - 설정(신원·비밀번호 기록)을 root 만 읽을 수 있는 `/Library/Application Support/Legato/loginwindow` 로 복사한다.
//!
//! 복사본이라 앱을 업데이트하거나 접속 코드를 바꾸면 다시 설치해야 한다. 이를 알 수 있게 설치 당시 원본의
//! 해시를 누구나 읽을 수 있는 stamp 파일에 남기고, `status` 가 지금 원본과 비교해 `stale` 을 알려 준다.
//! 설치·제거는 root 권한이 필요하다 (앱이 관리자 암호 창으로 실행).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use legato_core::t;
use sha2::{Digest, Sha256};

pub const LABEL: &str = "com.hdomi.legato.loginwindow";
const HELPER: &str = "/Library/PrivilegedHelperTools/com.hdomi.legato.host";
const SUPPORT_DIR: &str = "/Library/Application Support/Legato";
const LOG_DIR: &str = "/Library/Logs/Legato";
/// 복사하는 설정 파일.
const CONFIG_FILES: [&str; 2] = ["identity.json", "host.json"];

fn plist_path() -> PathBuf {
    PathBuf::from(format!("/Library/LaunchAgents/{LABEL}.plist"))
}

fn data_dir() -> PathBuf {
    Path::new(SUPPORT_DIR).join("loginwindow")
}

fn stamp_path() -> PathBuf {
    Path::new(SUPPORT_DIR).join("loginwindow.stamp")
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

/// 실행 파일과 설정 파일 내용으로 만든 해시. 복사본이 원본과 같은지 확인하는 데 쓴다.
fn source_digest(exe: &Path, config_dir: &Path) -> Result<String> {
    let mut h = Sha256::new();
    h.update(std::fs::read(exe).with_context(|| format!("{} 읽기 실패", exe.display()))?);
    for name in CONFIG_FILES {
        let p = config_dir.join(name);
        let bytes = std::fs::read(&p).with_context(|| format!("{} 읽기 실패", p.display()))?;
        h.update(name.as_bytes());
        if name == "host.json" {
            // 표시 비밀번호는 켤 때마다 바뀌므로 빼고 비교한다 (나머지 키는 serde_json 이 정렬해 둔다).
            let mut v: serde_json::Value = serde_json::from_slice(&bytes).context("host.json 형식 오류")?;
            if let Some(o) = v.as_object_mut() {
                o.remove("display_password");
            }
            h.update(v.to_string().as_bytes());
        } else {
            h.update(&bytes);
        }
    }
    Ok(hex::encode(h.finalize()))
}

/// (설치됨, 원본이 바뀌어 다시 설치해야 함)
pub fn status(config_dir: &Path) -> (bool, bool) {
    let installed = plist_path().exists();
    if !installed {
        return (false, false);
    }
    let stamp = std::fs::read_to_string(stamp_path()).unwrap_or_default();
    let current = std::env::current_exe().ok().and_then(|exe| source_digest(&exe, config_dir).ok());
    let stale = current.is_none_or(|d| d != stamp.trim());
    (true, stale)
}

/// root 소유, 지정한 권한으로 파일을 원자적으로 쓴다.
/// 임시 파일은 (root 만 쓸 수 있는) 대상 폴더 안에 새로 만든다. 누구나 쓰는 /tmp 는 쓰지 않는다.
fn install_bytes(data: &[u8], dst: &Path, mode: u32) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dst.with_extension("installing");
    if std::fs::symlink_metadata(&tmp).is_ok() {
        std::fs::remove_file(&tmp).with_context(|| format!("{} 지우기 실패", tmp.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("{} 만들기 실패", tmp.display()))?;
    f.write_all(data).with_context(|| format!("{} 쓰기 실패", tmp.display()))?;
    f.sync_all().ok();
    drop(f);
    std::os::unix::fs::chown(&tmp, Some(0), Some(0)).context("소유자 변경 실패")?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).context("권한 변경 실패")?;
    std::fs::rename(&tmp, dst).with_context(|| format!("{} 교체 실패", dst.display()))?;
    Ok(())
}

fn install_file(src: &Path, dst: &Path, mode: u32) -> Result<()> {
    let data = std::fs::read(src).with_context(|| format!("{} 읽기 실패", src.display()))?;
    install_bytes(&data, dst, mode)
}

/// root 소유 폴더를 만들고 권한을 맞춘다 (이미 있으면 권한만 다시 맞춘다).
fn root_dir(path: &Path, mode: u32) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("{} 만들기 실패", path.display()))?;
    std::os::unix::fs::chown(path, Some(0), Some(0)).context("소유자 변경 실패")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).context("권한 변경 실패")?;
    Ok(())
}

pub fn install(config_dir: &Path) -> Result<()> {
    if !is_root() {
        bail!(
            "{}",
            t!(
                "root 권한이 필요합니다. 앱에서 켜거나 sudo 로 실행해 주세요",
                "Root permission is required. Turn it on from the app or run with sudo"
            )
        );
    }
    let exe = std::env::current_exe().context("실행 파일 경로를 알 수 없음")?;
    if CONFIG_FILES.iter().any(|f| !config_dir.join(f).exists()) {
        bail!(
            "{}",
            t!(
                "설정이 아직 없습니다. 먼저 앱에서 호스트를 한 번 켜고 고정 접속 코드를 정해 주세요",
                "No settings yet. Turn on the host in the app once and set an access code first"
            )
        );
    }
    let digest = source_digest(&exe, config_dir)?;

    // 1. 실행 파일 복사
    root_dir(Path::new("/Library/PrivilegedHelperTools"), 0o755)?;
    install_file(&exe, Path::new(HELPER), 0o755)?;

    // 2. 설정 복사 (root 만 읽게)
    root_dir(Path::new(SUPPORT_DIR), 0o755)?;
    let data = data_dir();
    root_dir(&data, 0o700)?;
    install_file(&config_dir.join("identity.json"), &data.join("identity.json"), 0o600)?;
    // 로그인 화면에서는 고정 접속 코드로만 받는다. 예전에 화면에 보였던 표시 비밀번호는 빼고 복사한다.
    let mut host: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_dir.join("host.json")).context("host.json 읽기 실패")?)
            .context("host.json 형식 오류")?;
    if host.get("access_code").is_none_or(|v| v.is_null()) {
        bail!("{}", t!("고정 접속 코드를 먼저 정해 주세요", "Set an access code first"));
    }
    if let Some(o) = host.as_object_mut() {
        o.insert("display_password".into(), serde_json::Value::Null);
        o.insert("allow_display_password".into(), serde_json::Value::Bool(false));
    }
    install_bytes(host.to_string().as_bytes(), &data.join("host.json"), 0o600)?;
    root_dir(Path::new(LOG_DIR), 0o700)?;
    // 예전 버전이 남긴 로그도 root 만 읽게 맞춘다.
    let old_log = Path::new(LOG_DIR).join("loginwindow.log");
    if old_log.exists() {
        std::fs::set_permissions(&old_log, std::fs::Permissions::from_mode(0o600)).context("로그 권한 변경 실패")?;
    }

    // 3. LaunchAgent
    let args = [
        HELPER.to_string(),
        "--config-dir".into(),
        data.to_string_lossy().to_string(),
        "--log-file".into(),
        Path::new(LOG_DIR).join("loginwindow.log").to_string_lossy().to_string(),
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
    // launchd 는 root 소유 · 644 가 아니면 읽지 않는다.
    install_bytes(plist.as_bytes(), &plist_path(), 0o644)?;

    // 4. 원본 해시 (누구나 읽을 수 있게)
    install_bytes(digest.as_bytes(), &stamp_path(), 0o644)
}

pub fn uninstall() -> Result<()> {
    if !is_root() {
        bail!(
            "{}",
            t!(
                "root 권한이 필요합니다. 앱에서 끄거나 sudo 로 실행해 주세요",
                "Root permission is required. Turn it off from the app or run with sudo"
            )
        );
    }
    for path in [plist_path(), PathBuf::from(HELPER), stamp_path()] {
        if std::fs::symlink_metadata(&path).is_ok() {
            std::fs::remove_file(&path).with_context(|| format!("{} 지우기 실패", path.display()))?;
        }
    }
    let data = data_dir();
    if data.exists() {
        std::fs::remove_dir_all(&data).with_context(|| format!("{} 지우기 실패", data.display()))?;
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
