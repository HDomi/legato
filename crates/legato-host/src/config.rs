//! 호스트 설정 파일 (`host.json`) 과 TLS 신원 (`identity.json`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use legato_core::auth::PasswordRecord;
use legato_core::identity::HostIdentity;
use serde::{Deserialize, Serialize};

/// 호스트 설정. 자격 증명은 두 가지다.
/// - 표시 비밀번호: 화면에 보여주고 알려주는 용도라 원문을 저장한다. 기본으로 켤 때마다 새로 만든다.
/// - 고정 접속 코드: 본인만 아는 비밀. 해시만 저장하고 직접 바꾸기 전에는 바뀌지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    pub port: u16,
    pub display_password: Option<String>,
    /// 호스트를 켤 때마다 표시 비밀번호를 새로 만든다.
    pub rotate_display_password: bool,
    /// 표시 비밀번호로 접속을 허용한다. 끄면 고정 접속 코드로만 들어올 수 있다.
    pub allow_display_password: bool,
    pub access_code: Option<PasswordRecord>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            port: legato_core::DEFAULT_PORT,
            display_password: None,
            rotate_display_password: true,
            allow_display_password: true,
            access_code: None,
        }
    }
}

fn config_path() -> Result<PathBuf> {
    Ok(legato_core::paths::config_dir()?.join("host.json"))
}

impl HostConfig {
    pub fn load() -> Result<Self> {
        let p = config_path()?;
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("설정 파일 형식 오류: {}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("설정 파일 읽기 실패: {}", p.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        legato_core::paths::write_private(&config_path()?, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

pub fn load_identity() -> Result<HostIdentity> {
    HostIdentity::load_or_create(&legato_core::paths::config_dir()?.join("identity.json"))
}
