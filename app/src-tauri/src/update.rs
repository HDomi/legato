//! 앱 안 업데이터. GitHub 최신 릴리스를 확인하고, 받은 파일을 검증한 뒤 교체한다.
//!
//! - macOS: `Legato-<v>-arm64-mac.zip` 을 받아 SHA256SUMS 와 대조하고, 번들 id · 버전 · 서명을 확인한다.
//!   지금 앱이 인증서로 서명돼 있으면 새 앱도 같은 지정 요구사항을 만족해야 한다 (다른 사람이 서명한 앱으로 바뀌지 않게).
//!   앱이 끝나면 보조 스크립트가 `/Applications` 의 앱을 바꾸고(실패하면 원래 앱으로 복구) 다시 연다.
//! - Windows: `Legato_<v>_x64-setup.exe` 를 받아 대조한 뒤 실행하고 앱을 끝낸다.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use sha2::{Digest, Sha256};

const REPO: &str = "HDomi/legato";
const USER_AGENT: &str = "Legato-Updater";
/// 받을 파일 최대 크기 (잘못된 응답으로 디스크를 채우지 않게).
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub notes: String,
    pub url: String,
    /// 이 기기용 설치 파일이 릴리스에 있는지.
    pub has_asset: bool,
    /// 지금 앱이 ad-hoc 서명이라 업데이트하면 macOS 권한을 다시 허용해야 하는지.
    pub permissions_reset: bool,
}

/// `v1.2.3` / `1.2.3` → (1,2,3). 형식이 다르면 None.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() { None } else { Some(v) }
}

fn asset_name(version: &str) -> String {
    if cfg!(windows) { format!("Legato_{version}_x64-setup.exe") } else { format!("Legato-{version}-arm64-mac.zip") }
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("{} ({url}): {e}", crate::tr("요청에 실패했습니다", "Request failed")))
}

/// 최신 릴리스 태그. GitHub API 는 비로그인 호출 한도(IP 당 시간당 60회)가 있어서 회사처럼
/// 공인 IP 를 여럿이 쓰면 막힐 수 있다. 그래서 한도가 없는 `releases/latest` 리다이렉트로 버전을 알아낸다.
fn latest_tag() -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).build().into();
    let url = format!("https://github.com/{REPO}/releases/latest");
    let res = agent
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("{} ({url}): {e}", crate::tr("요청에 실패했습니다", "Request failed")))?;
    let loc = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| crate::tr("최신 릴리스를 찾지 못했습니다", "Couldn't find the latest release"))?;
    let tag = loc.rsplit('/').next().unwrap_or_default().to_string();
    if parse_version(&tag).is_none() {
        return Err(format!(
            "{}: {tag}",
            crate::tr("최신 릴리스 태그 형식이 올바르지 않습니다", "Unexpected release tag")
        ));
    }
    Ok(tag)
}

/// 릴리스 노트 (선택). API 한도에 걸리면 빈 문자열.
fn release_notes(tag: &str) -> String {
    get(&format!("https://api.github.com/repos/{REPO}/releases/tags/{tag}"))
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| v["body"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn sha256sums(version: &str) -> Result<String, String> {
    let mut res = get(&release_file_url(version, "SHA256SUMS.txt"))?;
    res.body_mut().read_to_string().map_err(|e| e.to_string())
}

/// 최신 릴리스 정보를 가져온다.
pub fn check(current: &str) -> Result<UpdateInfo, String> {
    let tag = latest_tag()?;
    let latest = tag.trim_start_matches('v').to_string();
    let available = match (parse_version(&latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    };
    // 이 기기용 파일이 있는지는 체크섬 목록으로 판단한다 (API 를 쓰지 않으려고).
    let has_asset = available && sha256sums(&latest).is_ok_and(|s| expected_hash(&s, &asset_name(&latest)).is_some());
    Ok(UpdateInfo {
        current: current.to_string(),
        notes: if available { release_notes(&tag) } else { String::new() },
        url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
        latest,
        available,
        has_asset,
        permissions_reset: cfg!(target_os = "macos") && current_is_adhoc(),
    })
}

/// 파일을 받아 저장하고 SHA-256 을 돌려준다.
fn download(url: &str, dest: &Path) -> Result<String, String> {
    let res = get(url)?;
    let mut reader = res.into_body().into_reader().take(MAX_DOWNLOAD + 1);
    let mut file = std::fs::File::create(dest).map_err(|e| {
        format!("{} ({}): {e}", crate::tr("파일을 만들지 못했습니다", "Couldn't create the file"), dest.display())
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", crate::tr("받는 중 오류가 났습니다", "Download error")))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_DOWNLOAD {
            return Err(crate::tr("파일이 너무 큽니다", "The file is too large"));
        }
        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut file, &buf[..n])
            .map_err(|e| format!("{}: {e}", crate::tr("저장하지 못했습니다", "Couldn't save")))?;
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA256SUMS.txt 에서 파일 해시를 찾는다.
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name).then(|| hash.to_lowercase())
    })
}

fn release_file_url(version: &str, name: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{version}/{name}")
}

/// 업데이트를 받아 검증하고 설치를 준비한다. 성공하면 앱을 끝내야 설치가 진행된다.
/// `resume_host`: 다시 열린 앱이 호스트를 바로 켜도록 한다.
pub fn install(version: &str, resume_host: bool) -> Result<(), String> {
    if parse_version(version).is_none() {
        return Err(format!("{}: {version}", crate::tr("버전 형식이 올바르지 않습니다", "Invalid version")));
    }
    let dir = cache_dir()?.join(version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let name = asset_name(version);
    let sums = sha256sums(version)?;
    let want = expected_hash(&sums, &name).ok_or_else(|| {
        format!(
            "{}: {name}",
            crate::tr("체크섬 목록에 이 기기용 파일이 없습니다", "The checksum list has no file for this device")
        )
    })?;
    let file = dir.join(&name);
    let got = download(&release_file_url(version, &name), &file)?;
    if got != want {
        let _ = std::fs::remove_file(&file);
        return Err(format!(
            "{} ({want} / {got})",
            crate::tr(
                "받은 파일의 체크섬이 달라 설치하지 않았습니다",
                "The download's checksum didn't match, so it wasn't installed"
            )
        ));
    }
    platform_install(&dir, &file, version, resume_host)
}

fn cache_dir() -> Result<PathBuf, String> {
    let base = if cfg!(windows) {
        std::env::temp_dir().join("Legato-update")
    } else {
        let home = std::env::var_os("HOME").ok_or_else(|| "HOME".to_string())?;
        PathBuf::from(home).join("Library/Caches/com.hdomi.legato/update")
    };
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base)
}

/// 지금 실행 중인 앱 번들 경로 (`…/Legato.app`).
#[cfg(target_os = "macos")]
fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension().is_some_and(|e| e == "app")).then(|| app.to_path_buf())
}

#[cfg(target_os = "macos")]
fn designated_requirement(app: &Path) -> Option<String> {
    let out = Command::new("codesign").args(["-d", "-r-"]).arg(app).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    text.lines().find_map(|l| l.strip_prefix("designated => ").map(str::to_string))
}

/// ad-hoc 서명이면 지정 요구사항이 cdhash 라서 업데이트마다 권한이 초기화된다.
#[cfg(target_os = "macos")]
pub fn current_is_adhoc() -> bool {
    current_bundle().and_then(|a| designated_requirement(&a)).is_none_or(|r| r.contains("cdhash"))
}

#[cfg(not(target_os = "macos"))]
pub fn current_is_adhoc() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn plist_value(app: &Path, key: &str) -> Option<String> {
    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn platform_install(dir: &Path, zip: &Path, version: &str, resume_host: bool) -> Result<(), String> {
    let current = current_bundle().ok_or_else(|| {
        crate::tr(
            "앱 번들 안에서 실행 중이 아니라 업데이트할 수 없습니다",
            "Not running from the app bundle, so it can't update itself",
        )
    })?;
    let parent =
        current.parent().ok_or_else(|| crate::tr("앱 폴더를 찾을 수 없습니다", "Can't find the app folder"))?;
    if parent.starts_with("/Volumes") || current.to_string_lossy().contains("/AppTranslocation/") {
        return Err(crate::tr(
            "DMG 안이나 격리된 위치에서 실행 중입니다. 응용 프로그램 폴더로 옮긴 뒤 다시 시도해 주세요",
            "Running from the DMG or a quarantined location. Move the app to Applications and try again",
        ));
    }
    let probe = parent.join(".legato-write-test");
    std::fs::write(&probe, b"").map_err(|_| {
        format!(
            "{} ({})",
            crate::tr("이 폴더에 쓸 수 없어 교체할 수 없습니다", "Can't write to this folder, so it can't be replaced"),
            parent.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);

    let unpack = dir.join("unpacked");
    let st = Command::new("ditto").args(["-x", "-k"]).arg(zip).arg(&unpack).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(crate::tr("압축을 풀지 못했습니다", "Couldn't unpack the update"));
    }
    let new_app = unpack.join("Legato.app");
    if plist_value(&new_app, "CFBundleIdentifier").as_deref() != Some("com.hdomi.legato") {
        return Err(crate::tr("받은 앱의 번들 ID 가 다릅니다", "The downloaded app has a different bundle ID"));
    }
    if plist_value(&new_app, "CFBundleShortVersionString").as_deref() != Some(version) {
        return Err(crate::tr(
            "받은 앱의 버전이 릴리스와 다릅니다",
            "The downloaded app's version doesn't match the release",
        ));
    }
    let ok = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&new_app)
        .status()
        .map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err(crate::tr(
            "받은 앱의 서명을 확인하지 못했습니다",
            "The downloaded app's signature couldn't be verified",
        ));
    }
    // 지금 앱이 인증서로 서명돼 있으면 새 앱도 같은 요구사항을 만족해야 한다.
    if let Some(req) = designated_requirement(&current)
        && !req.contains("cdhash")
    {
        let st = Command::new("codesign")
            .arg("--verify")
            .arg(format!("-R={req}"))
            .arg(&new_app)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(crate::tr(
                "받은 앱이 지금 앱과 다른 인증서로 서명되어 있어 설치하지 않았습니다",
                "The downloaded app is signed with a different certificate, so it wasn't installed",
            ));
        }
    }

    // 앱이 끝나기를 기다렸다가 교체하는 보조 스크립트. 실패하면 원래 앱을 되돌린다.
    let log = dir.join("install.log");
    let script = dir.join("install.sh");
    let args = if resume_host { "--args --resume-host" } else { "" };
    let body = format!(
        r#"#!/bin/bash
exec >>"{log}" 2>&1
echo "$(date) 업데이트 시작: v{version}"
PID={pid}
for i in $(seq 1 120); do kill -0 $PID 2>/dev/null || break; sleep 0.5; done
CUR="{cur}"
OLD="{cur}.old-update"
rm -rf "$OLD"
if mv "$CUR" "$OLD" && ditto "{new}" "$CUR"; then
  rm -rf "$OLD"
  # macOS 26 의 xattr 에는 -r 이 없어서 find 로 하나씩 지운다.
  find "$CUR" -exec xattr -d com.apple.quarantine {{}} + 2>/dev/null
  echo "$(date) 교체 완료"
else
  echo "$(date) 교체 실패, 원래 앱 복구"
  rm -rf "$CUR"; mv "$OLD" "$CUR"
fi
open "$CUR" {args}
"#,
        log = log.display(),
        pid = std::process::id(),
        cur = current.display(),
        new = new_app.display(),
    );
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    Command::new("/bin/bash")
        .arg(&script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            format!("{}: {e}", crate::tr("설치 스크립트를 실행하지 못했습니다", "Couldn't run the install script"))
        })?;
    Ok(())
}

#[cfg(windows)]
fn platform_install(_dir: &Path, exe: &Path, _version: &str, _resume_host: bool) -> Result<(), String> {
    // NSIS 설치 프로그램을 띄운다. 앱이 끝나야 파일을 덮어쓸 수 있으므로 호출한 쪽이 곧바로 앱을 끝낸다.
    Command::new(exe).spawn().map_err(|e| {
        format!("{}: {e}", crate::tr("설치 프로그램을 실행하지 못했습니다", "Couldn't run the installer"))
    })?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_install(_dir: &Path, _file: &Path, _version: &str, _resume_host: bool) -> Result<(), String> {
    Err(crate::tr("이 OS 에서는 앱 안 업데이트를 지원하지 않습니다", "In-app updates aren't supported on this OS"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 버전_비교() {
        assert_eq!(parse_version("v0.1.10"), Some((0, 1, 10)));
        assert!(parse_version("0.1.10") > parse_version("0.1.9"));
        assert_eq!(parse_version("0.1"), None);
        assert_eq!(parse_version("0.1.1-beta"), None);
    }

    /// 실제 GitHub 에 접속한다. 네트워크가 없으면 실패하므로 기본으로는 돌리지 않는다.
    #[test]
    #[ignore]
    fn 실제_최신_릴리스_확인() {
        let info = check("0.0.1").unwrap();
        assert!(info.available);
        assert!(info.has_asset, "이 기기용 파일이 릴리스에 있어야 함");
    }

    #[test]
    fn 체크섬_줄_찾기() {
        let sums = "abc  Legato-0.1.1-arm64.dmg\nDEF  Legato-0.1.1-arm64-mac.zip\n";
        assert_eq!(expected_hash(sums, "Legato-0.1.1-arm64-mac.zip").as_deref(), Some("def"));
        assert_eq!(expected_hash(sums, "없음"), None);
    }
}
