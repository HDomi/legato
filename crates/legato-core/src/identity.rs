//! 호스트 TLS 신원. 처음 실행할 때 자체 서명 인증서를 만들고 계속 같은 걸 쓴다.
//! 뷰어는 이 인증서의 SHA-256 지문으로 호스트를 알아본다.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
pub struct HostIdentity {
    #[serde(with = "hex_vec")]
    pub cert_der: Vec<u8>,
    #[serde(with = "hex_vec")]
    pub key_der: Vec<u8>,
}

impl HostIdentity {
    /// 파일에서 읽고, 없으면 새로 만들어 저장한다.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let s =
                std::fs::read_to_string(path).with_context(|| format!("신원 파일 읽기 실패: {}", path.display()))?;
            return serde_json::from_str(&s).context("신원 파일 형식 오류");
        }
        let id = Self::generate()?;
        crate::paths::write_private(path, serde_json::to_string_pretty(&id)?.as_bytes())?;
        Ok(id)
    }

    /// 새 자체 서명 인증서를 만든다. 이름 검증은 하지 않으므로 SAN 은 고정값.
    pub fn generate() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec!["legato.local".to_string()]).context("인증서 생성 실패")?;
        Ok(Self { cert_der: ck.cert.der().to_vec(), key_der: ck.signing_key.serialize_der() })
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert_der)
    }
}

/// 인증서 DER 의 SHA-256 지문 (소문자 hex 64자).
pub fn fingerprint(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

/// 사람이 눈으로 대조하기 쉬운 짧은 형태 (앞 16바이트, 4자씩 묶음).
pub fn short_fingerprint(fp: &str) -> String {
    fp.chars()
        .take(32)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|c| c.iter().collect::<String>().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

mod hex_vec {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        hex::decode(String::deserialize(d)?).map_err(D::Error::custom)
    }
}
