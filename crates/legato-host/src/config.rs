//! 호스트 설정 파일 (`host.json`) 과 TLS 신원 (`identity.json`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use legato_core::auth::PasswordRecord;
use legato_core::identity::HostIdentity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    pub port: u16,
    pub password: Option<PasswordRecord>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { port: legato_core::DEFAULT_PORT, password: None }
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
