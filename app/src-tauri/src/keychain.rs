//! 저장한 접속 코드 보관.
//!
//! - macOS: `security` 도구로 로그인 키체인에 넣는다. 항목은 "모든 앱 접근 허용"(-A) 으로 만든다.
//!   앱별로 묶으면 업데이트로 서명 해시가 바뀔 때마다 키체인 암호를 다시 묻기 때문이다.
//!   대신 이 사용자 계정으로 도는 다른 앱도 읽을 수 있다 (본인만 읽는 파일과 비슷한 보호 수준).
//!   값은 `hex:` 로 시작하는 16진수로 저장한다 (`security` 가 비ASCII 값을 16진수로 내보내 헷갈리지 않게).
//! - Windows: 자격 증명 관리자 (keyring). 앱별 제한이 없어 업데이트와 무관하다.

const SERVICE: &str = "com.hdomi.legato";

#[cfg(target_os = "macos")]
mod imp {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::SERVICE;

    /// `security -i` 명령줄 인자 하나를 큰따옴표로 감싼다.
    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    pub fn set(address: &str, code: &str) -> Result<(), String> {
        let value = format!("hex:{}", hex::encode(code.as_bytes()));
        // 비밀 값이 명령줄(ps)에 보이지 않게 표준 입력으로 넘긴다.
        let line =
            format!("add-generic-password -U -A -s {} -a {} -w {}\n", quote(SERVICE), quote(address), quote(&value));
        let mut child = Command::new("/usr/bin/security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child.stdin.take().expect("piped").write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let err = String::from_utf8_lossy(&out.stderr);
        if out.status.success() && err.trim().is_empty() { Ok(()) } else { Err(err.trim().to_string()) }
    }

    pub fn get(address: &str) -> Result<Option<String>, String> {
        let out = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE, "-a", address, "-w"])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            // 44 = 항목 없음.
            return if out.status.code() == Some(44) {
                Ok(None)
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            };
        }
        let raw = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
        if let Some(h) = raw.strip_prefix("hex:") {
            let bytes = hex::decode(h).map_err(|e| e.to_string())?;
            return String::from_utf8(bytes).map(Some).map_err(|e| e.to_string());
        }
        // 예전 형식(앱이 직접 넣은 원문): 새 형식으로 옮겨 둔다. 비ASCII 원문은 security 가 16진수로 내보낸다.
        let code = if raw.len().is_multiple_of(2) && !raw.is_empty() && raw.chars().all(|c| c.is_ascii_hexdigit()) {
            hex::decode(&raw).ok().and_then(|b| String::from_utf8(b).ok()).filter(|s| !s.is_ascii()).unwrap_or(raw)
        } else {
            raw
        };
        let _ = delete(address);
        let _ = set(address, &code);
        Ok(Some(code))
    }

    pub fn delete(address: &str) -> Result<(), String> {
        let out = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", address])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() || out.status.code() == Some(44) {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::SERVICE;

    fn entry(address: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, address).map_err(|e| e.to_string())
    }

    pub fn set(address: &str, code: &str) -> Result<(), String> {
        entry(address)?.set_password(code).map_err(|e| e.to_string())
    }

    pub fn get(address: &str) -> Result<Option<String>, String> {
        match entry(address)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn delete(address: &str) -> Result<(), String> {
        match entry(address)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub use imp::{delete, get, set};

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// 실제 로그인 키체인을 쓰므로 기본으로는 돌리지 않는다.
    #[test]
    #[ignore]
    fn 키체인_왕복() {
        let addr = "selftest.legato.invalid:1";
        let code = "a \"b\" c\\d $e 'f' 한글 %";
        super::set(addr, code).unwrap();
        assert_eq!(super::get(addr).unwrap().as_deref(), Some(code));
        super::delete(addr).unwrap();
        assert_eq!(super::get(addr).unwrap(), None);
    }
}
