//! 뷰어가 기억하는 호스트 지문 목록 (TOFU: 처음 접속 때 기억하고 이후 바뀌면 거부).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

pub struct KnownHosts {
    path: PathBuf,
    map: BTreeMap<String, String>,
}

impl KnownHosts {
    pub fn load() -> Result<Self> {
        let path = crate::paths::config_dir()?.join("known_hosts.json");
        let map = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        };
        Ok(Self { path, map })
    }

    /// `호스트:포트` 키로 저장된 지문.
    pub fn get(&self, host_key: &str) -> Option<&str> {
        self.map.get(host_key).map(String::as_str)
    }

    pub fn set(&mut self, host_key: &str, fingerprint: &str) -> Result<()> {
        self.map.insert(host_key.to_string(), fingerprint.to_string());
        crate::paths::write_private(&self.path, serde_json::to_string_pretty(&self.map)?.as_bytes())
    }
}
