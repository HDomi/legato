//! 비밀번호 인증.
//!
//! 비밀번호 자체는 네트워크로 보내지 않는다. 호스트가 salt·nonce 를 주면 뷰어가
//! `HMAC(PBKDF2(비밀번호), 도메인 || nonce || TLS 채널 바인딩)` 을 돌려준다.
//! 채널 바인딩은 QUIC TLS exporter 값이라 중간자가 다른 TLS 세션으로 응답을 중계해도 검증에 실패한다.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// PBKDF2 반복 횟수 기본값.
pub const DEFAULT_ITERATIONS: u32 = 200_000;

/// 고정 접속 코드 최소 길이. 바뀌지 않는 비밀이라 표시 비밀번호보다 길게 요구한다.
pub const MIN_ACCESS_CODE_LEN: usize = 10;

const MAC_DOMAIN: &[u8] = b"legato-auth-v1";
const SERVER_MAC_DOMAIN: &[u8] = b"legato-auth-v1-server";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-legato-auth";

/// 호스트가 저장하는 비밀번호 검증 정보. 원문 비밀번호는 저장하지 않는다.
/// 다만 `key` 는 이 프로토콜에서 비밀번호와 같은 효력이라 파일 권한으로 보호해야 한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordRecord {
    #[serde(with = "hex_array16")]
    pub salt: [u8; 16],
    pub iterations: u32,
    #[serde(with = "hex_array32")]
    pub key: [u8; 32],
}

impl PasswordRecord {
    /// 비밀번호로 새 레코드를 만든다. salt 는 매번 새로 뽑는다.
    pub fn new(password: &str) -> Self {
        let mut salt = [0u8; 16];
        rand::rng().fill(&mut salt);
        let key = derive_key(password, &salt, DEFAULT_ITERATIONS);
        Self { salt, iterations: DEFAULT_ITERATIONS, key }
    }

    /// 뷰어가 보낸 MAC 이 맞는지 상수 시간으로 비교한다.
    pub fn verify(&self, nonce: &[u8; 32], binding: &[u8; 32], mac: &[u8; 32]) -> bool {
        mac_eq(&compute_mac(&self.key, nonce, binding), mac)
    }

    /// 챌린지 슬롯 정보.
    pub fn slot(&self) -> crate::protocol::AuthSlot {
        crate::protocol::AuthSlot { salt: self.salt, iterations: self.iterations }
    }

    /// 호스트 증명 MAC.
    pub fn server_proof(&self, nonce: &[u8; 32], binding: &[u8; 32]) -> [u8; 32] {
        compute_server_mac(&self.key, nonce, binding)
    }
}

/// 비밀번호에서 32바이트 키를 만든다 (PBKDF2-HMAC-SHA256).
pub fn derive_key(password: &str, salt: &[u8; 16], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut out);
    out
}

/// 챌린지 응답 MAC 을 계산한다 (뷰어 → 호스트).
pub fn compute_mac(key: &[u8; 32], nonce: &[u8; 32], binding: &[u8; 32]) -> [u8; 32] {
    hmac3(key, MAC_DOMAIN, nonce, binding)
}

/// 호스트 증명 MAC 을 계산한다 (호스트 → 뷰어). 도메인이 달라 뷰어 응답을 그대로 되돌려 쓸 수 없다.
pub fn compute_server_mac(key: &[u8; 32], nonce: &[u8; 32], binding: &[u8; 32]) -> [u8; 32] {
    hmac3(key, SERVER_MAC_DOMAIN, nonce, binding)
}

/// 상수 시간 비교.
pub fn mac_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.ct_eq(b).into()
}

fn hmac3(key: &[u8; 32], domain: &[u8], nonce: &[u8; 32], binding: &[u8; 32]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC 키 길이는 자유");
    mac.update(domain);
    mac.update(nonce);
    mac.update(binding);
    mac.finalize().into_bytes().into()
}

/// QUIC 연결에서 채널 바인딩 값을 뽑는다. 양쪽이 같은 TLS 세션이면 같은 값이 나온다.
pub fn channel_binding(conn: &quinn::Connection) -> anyhow::Result<[u8; 32]> {
    let mut out = [0u8; 32];
    conn.export_keying_material(&mut out, EXPORTER_LABEL, b"")
        .map_err(|_| anyhow::anyhow!("TLS exporter 값을 만들 수 없음"))?;
    Ok(out)
}

/// 슬롯 목록과 뷰어 응답을 비교해 맞은 슬롯 번호를 돌려준다.
/// 응답 시간으로 어느 슬롯인지 새지 않게 모든 슬롯을 끝까지 검사한다.
pub fn match_slot(
    records: &[&PasswordRecord],
    nonce: &[u8; 32],
    binding: &[u8; 32],
    macs: &[[u8; 32]],
) -> Option<usize> {
    if macs.len() != records.len() {
        return None;
    }
    let mut found = None;
    for (i, (rec, mac)) in records.iter().zip(macs).enumerate() {
        if rec.verify(nonce, binding, mac) && found.is_none() {
            found = Some(i);
        }
    }
    found
}

/// 새 nonce 를 만든다.
pub fn new_nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::rng().fill(&mut n);
    n
}

/// 사람이 읽고 입력하기 쉬운 임의 비밀번호를 만든다 (예: `k7mq-x2ph-9wtr`).
/// 헷갈리는 글자(0, o, 1, l, i)는 뺐다. 약 62비트 엔트로피.
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    let mut rng = rand::rng();
    let chars: Vec<char> = (0..12).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// 잠금 기준 주소. IPv6 는 같은 사람이 /64 안에서 주소를 바꿔 가며 시도할 수 있어 /64 로 묶는다.
pub fn limiter_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut seg = v6.segments();
                seg[4..].fill(0);
                IpAddr::V6(std::net::Ipv6Addr::from(seg))
            }
        },
    }
}

/// 주소(IPv6 는 /64) 별 인증 실패 횟수를 세서 무차별 대입을 막는다.
/// - 5번 실패하면 30초 잠그고, 이후 실패마다 잠금 시간을 두 배로 늘린다 (최대 1시간).
/// - 모든 주소를 합쳐 1분에 20번 넘게 실패하면 30초 동안 모든 인증을 거절한다 (주소를 바꿔 가며 시도하는 공격 대비).
/// - 기록은 최대 10,000개, 1시간 지나면 지운다 (메모리 고갈 방지).
#[derive(Default)]
pub struct AuthLimiter {
    entries: HashMap<IpAddr, LimiterEntry>,
    global_failures: std::collections::VecDeque<Instant>,
    global_locked_until: Option<Instant>,
}

struct LimiterEntry {
    failures: u32,
    locked_until: Option<Instant>,
    last: Instant,
}

impl AuthLimiter {
    const FREE_ATTEMPTS: u32 = 5;
    const BASE_LOCK: Duration = Duration::from_secs(30);
    const MAX_LOCK: Duration = Duration::from_secs(3600);
    const GLOBAL_WINDOW: Duration = Duration::from_secs(60);
    const GLOBAL_MAX_FAILURES: usize = 20;
    const GLOBAL_LOCK: Duration = Duration::from_secs(30);
    const MAX_ENTRIES: usize = 10_000;
    const ENTRY_TTL: Duration = Duration::from_secs(3600);

    /// 지금 시도할 수 있는지 확인한다. 잠겨 있으면 남은 시간을 돌려준다.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let now = Instant::now();
        if let Some(until) = self.global_locked_until
            && until > now
        {
            return Err(until - now);
        }
        match self.entries.get(&limiter_key(ip)).and_then(|e| e.locked_until) {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    /// 실패를 기록한다.
    pub fn record_failure(&mut self, ip: IpAddr) {
        let now = Instant::now();
        self.prune(now);
        self.global_failures.push_back(now);
        while self.global_failures.front().is_some_and(|t| now.duration_since(*t) > Self::GLOBAL_WINDOW) {
            self.global_failures.pop_front();
        }
        if self.global_failures.len() > Self::GLOBAL_MAX_FAILURES {
            self.global_locked_until = Some(now + Self::GLOBAL_LOCK);
        }
        let e =
            self.entries.entry(limiter_key(ip)).or_insert(LimiterEntry { failures: 0, locked_until: None, last: now });
        e.last = now;
        e.failures += 1;
        if e.failures >= Self::FREE_ATTEMPTS {
            let exp = (e.failures - Self::FREE_ATTEMPTS).min(16);
            let lock = Self::BASE_LOCK.saturating_mul(1 << exp).min(Self::MAX_LOCK);
            e.locked_until = Some(Instant::now() + lock);
        }
    }

    /// 성공하면 그 주소 기록을 지운다.
    pub fn record_success(&mut self, ip: IpAddr) {
        self.entries.remove(&limiter_key(ip));
    }

    /// 오래된 기록을 지우고, 그래도 너무 많으면 가장 오래된 것부터 지운다.
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| e.locked_until.is_some_and(|u| u > now) || now.duration_since(e.last) < Self::ENTRY_TTL);
        if self.entries.len() >= Self::MAX_ENTRIES {
            let mut by_age: Vec<(IpAddr, Instant)> = self.entries.iter().map(|(k, e)| (*k, e.last)).collect();
            by_age.sort_by_key(|(_, t)| *t);
            for (k, _) in by_age.into_iter().take(Self::MAX_ENTRIES / 10) {
                self.entries.remove(&k);
            }
        }
    }
}

mod hex_array16 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(v: &[u8; 16], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 16], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(s).map_err(D::Error::custom)?;
        v.try_into().map_err(|_| D::Error::custom("길이가 16바이트가 아님"))
    }
}

mod hex_array32 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(s).map_err(D::Error::custom)?;
        v.try_into().map_err(|_| D::Error::custom("길이가 32바이트가 아님"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 맞는_비밀번호만_통과() {
        let rec = PasswordRecord::new("abcd-efgh-jkmn");
        let nonce = new_nonce();
        let binding = [7u8; 32];
        let good = compute_mac(&derive_key("abcd-efgh-jkmn", &rec.salt, rec.iterations), &nonce, &binding);
        let bad = compute_mac(&derive_key("wrong", &rec.salt, rec.iterations), &nonce, &binding);
        assert!(rec.verify(&nonce, &binding, &good));
        assert!(!rec.verify(&nonce, &binding, &bad));
        // 채널 바인딩이 다르면 같은 비밀번호라도 실패해야 한다.
        assert!(!rec.verify(&nonce, &[8u8; 32], &good));
        // 호스트 증명은 뷰어 응답과 달라야 한다.
        assert_ne!(rec.server_proof(&nonce, &binding), good);
    }

    #[test]
    fn 여러_슬롯_중_맞는_슬롯_찾기() {
        let display = PasswordRecord::new("abcd-efgh-jkmn");
        let code = PasswordRecord::new("my-secret-code-123");
        let nonce = new_nonce();
        let binding = [3u8; 32];
        let recs = [&display, &code];
        let macs_for = |secret: &str| -> Vec<[u8; 32]> {
            recs.iter().map(|r| compute_mac(&derive_key(secret, &r.salt, r.iterations), &nonce, &binding)).collect()
        };
        assert_eq!(match_slot(&recs, &nonce, &binding, &macs_for("abcd-efgh-jkmn")), Some(0));
        assert_eq!(match_slot(&recs, &nonce, &binding, &macs_for("my-secret-code-123")), Some(1));
        assert_eq!(match_slot(&recs, &nonce, &binding, &macs_for("nope")), None);
        // 개수가 다르면 실패.
        assert_eq!(match_slot(&recs, &nonce, &binding, &macs_for("abcd-efgh-jkmn")[..1]), None);
    }

    #[test]
    fn 다섯번_실패하면_잠김() {
        let mut l = AuthLimiter::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..4 {
            l.record_failure(ip);
            assert!(l.check(ip).is_ok());
        }
        l.record_failure(ip);
        assert!(l.check(ip).is_err());
        l.record_success(ip);
        assert!(l.check(ip).is_ok());
    }

    #[test]
    fn ipv6_는_64_단위로_묶는다() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(limiter_key(a), limiter_key(b));
        assert_ne!(limiter_key(a), limiter_key(c));
        let mut l = AuthLimiter::default();
        for _ in 0..5 {
            l.record_failure(a);
        }
        assert!(l.check(b).is_err(), "같은 /64 의 다른 주소도 잠겨야 함");
        assert!(l.check(c).is_ok());
    }

    #[test]
    fn 전체_실패가_많으면_모두_잠근다() {
        let mut l = AuthLimiter::default();
        for i in 0..21u8 {
            l.record_failure(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 1, i)));
        }
        assert!(l.check("192.168.0.9".parse().unwrap()).is_err());
    }

    #[test]
    fn 비밀번호_형식() {
        let p = generate_password();
        assert_eq!(p.len(), 14);
        assert_eq!(p.matches('-').count(), 2);
    }

    #[test]
    fn 레코드_json_왕복() {
        let rec = PasswordRecord::new("x");
        let s = serde_json::to_string(&rec).unwrap();
        let back: PasswordRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(rec.key, back.key);
        assert_eq!(rec.salt, back.salt);
    }
}
