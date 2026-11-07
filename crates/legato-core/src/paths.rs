//! 설정 파일 위치.
//!
//! macOS: `~/Library/Application Support/com.hdomi.Legato/`
//! Windows: `%APPDATA%\hdomi\Legato\config\`
//! 환경 변수 `LEGATO_CONFIG_DIR` 이 있으면 그 경로를 쓴다 (테스트·여러 인스턴스용).

use std::path::PathBuf;

use anyhow::{Context, Result};

/// 설정 폴더를 돌려주고, 없으면 만든다.
pub fn config_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("LEGATO_CONFIG_DIR") {
        Some(d) => PathBuf::from(d),
        None => directories::ProjectDirs::from("com", "hdomi", "Legato")
            .context("홈 폴더를 찾을 수 없음")?
            .config_dir()
            .to_path_buf(),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("설정 폴더 생성 실패: {}", dir.display()))?;
    Ok(dir)
}

/// 비밀 정보가 든 파일을 본인만 읽을 수 있게 쓴다.
///
/// 임시 파일은 새로 만들어야만(이미 있으면 실패) 쓰고, 처음부터 0600 으로 만든다.
/// 남아 있던 임시 파일(심볼릭 링크일 수 있음)은 먼저 지운다. 그다음 원자적으로 이름을 바꾼다.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    // 심볼릭 링크를 따라가지 않고 링크 자체를 지운다.
    if std::fs::symlink_metadata(&tmp).is_ok() {
        std::fs::remove_file(&tmp).with_context(|| format!("남은 임시 파일 지우기 실패: {}", tmp.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).with_context(|| format!("파일 쓰기 실패: {}", tmp.display()))?;
    f.write_all(data).with_context(|| format!("파일 쓰기 실패: {}", tmp.display()))?;
    f.sync_all().ok();
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("파일 교체 실패: {}", path.display()))?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn 비밀_파일은_0600_이고_남은_링크를_따라가지_않는다() {
        let dir = std::env::temp_dir().join(format!("legato-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("victim");
        std::fs::write(&target, b"keep").unwrap();
        let path = dir.join("secret.json");
        // 공격자가 임시 파일 자리에 링크를 심어 둔 상황.
        std::os::unix::fs::symlink(&target, dir.join("secret.tmp")).unwrap();
        super::write_private(&path, b"data").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep", "링크 대상은 바뀌면 안 됨");
        assert_eq!(std::fs::read(&path).unwrap(), b"data");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
