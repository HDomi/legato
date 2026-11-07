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
pub fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).with_context(|| format!("파일 쓰기 실패: {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("파일 교체 실패: {}", path.display()))?;
    Ok(())
}
