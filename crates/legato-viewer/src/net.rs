//! 호스트 접속, 상호 인증, 제어·영상 스트림 처리.

use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use legato_core::auth::{channel_binding, compute_mac, compute_server_mac, derive_key, mac_eq};
use legato_core::framing::{read_msg, write_msg};
use legato_core::known_hosts::KnownHosts;
use legato_core::protocol::{ClientMsg, DisplayInfo, MAX_AUTH_SLOTS, Os, PROTOCOL_VERSION, ServerMsg};
use legato_core::t;

/// 호스트 인증서 지문이 기억한 것과 다름 (앱이 "새 지문 믿고 다시 접속" 을 보여주는 데 쓴다).
#[derive(Debug)]
pub struct FingerprintChanged(pub String);

impl std::fmt::Display for FingerprintChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match legato_core::i18n::lang() {
            legato_core::i18n::Lang::Ko => write!(
                f,
                "{} 의 인증서 지문이 예전과 다릅니다. 호스트를 다시 설치했거나 중간에서 가로채는 중일 수 있습니다. 호스트 화면의 지문과 같은지 확인한 뒤에만 새 지문을 믿고 다시 접속해 주세요.",
                self.0
            ),
            legato_core::i18n::Lang::En => write!(
                f,
                "The certificate fingerprint of {} has changed. The host may have been reinstalled, or someone may be intercepting the connection. Only trust the new fingerprint after checking it matches the one shown on the host.",
                self.0
            ),
        }
    }
}

impl std::error::Error for FingerprintChanged {}

/// 인증까지 끝난 세션.
pub struct Session {
    pub endpoint: quinn::Endpoint,
    pub conn: quinn::Connection,
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
    pub host_name: String,
    pub host_os: Os,
    pub displays: Vec<DisplayInfo>,
    /// 고정 접속 코드로 인증했는지.
    pub via_access_code: bool,
}

/// 호스트가 요구하는 PBKDF2 반복 횟수 하한. 가짜 호스트가 낮은 값을 보내 받은 응답을 빠르게 추측하지 못하게 한다.
const MIN_ITERATIONS: u32 = 100_000;

/// 인증 중 연결이 닫혔을 때 이유를 읽을 수 있는 문장으로 바꾼다.
fn closed_during_auth(conn: &quinn::Connection, e: anyhow::Error) -> anyhow::Error {
    match conn.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(c)) if c.reason.as_ref() == b"busy" => anyhow::anyhow!(
            "{}",
            t!(
                "같은 주소에서 이미 인증 중입니다. 잠시 뒤에 다시 시도해 주세요",
                "Another login from this address is in progress. Please try again shortly"
            )
        ),
        _ => e,
    }
}

/// 접속하고 비밀번호로 인증한다.
/// 처음 보는 호스트는 상호 인증이 끝난 뒤 지문을 기억한다. 기억한 지문과 다르면 거부한다.
pub async fn connect(
    addr: SocketAddr,
    host_key: &str,
    password: &str,
    accept_new_fingerprint: bool,
) -> Result<Session> {
    let mut known = KnownHosts::load()?;
    let expected = if accept_new_fingerprint { None } else { known.get(host_key).map(str::to_string) };
    let c = legato_core::transport::connect(addr, expected.clone()).await.map_err(|e| {
        if expected.is_some() && format!("{e:#}").contains("지문 불일치") {
            anyhow::Error::new(FingerprintChanged(host_key.to_string()))
        } else {
            e
        }
    })?;
    let conn = c.connection;
    let (mut send, mut recv) = conn.open_bi().await.context("제어 스트림 열기 실패")?;

    write_msg(
        &mut send,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            client_name: client_name(),
            client_os: Os::current(),
            lang: legato_core::i18n::lang(),
        },
    )
    .await
    .map_err(|e| closed_during_auth(&conn, e))?;
    let challenge: ServerMsg = read_msg(&mut recv)
        .await
        .map_err(|e| closed_during_auth(&conn, e))?
        .context(t!("호스트가 연결을 닫았습니다", "The host closed the connection"))?;
    let (slots, nonce) = match challenge {
        ServerMsg::Challenge { slots, nonce } => (slots, nonce),
        ServerMsg::Error { message } => bail!("{message}"),
        other => {
            bail!("{} ({other:?})", t!("호스트가 예상하지 못한 응답을 보냈습니다", "Unexpected reply from the host"))
        }
    };
    if slots.is_empty() || slots.len() > MAX_AUTH_SLOTS {
        bail!(
            "{} ({})",
            t!("호스트가 잘못된 인증 정보를 보냈습니다", "The host sent invalid auth parameters"),
            slots.len()
        );
    }
    if let Some(s) = slots.iter().find(|s| !(MIN_ITERATIONS..=10_000_000).contains(&s.iterations)) {
        bail!(
            "{} ({})",
            t!("호스트가 잘못된 인증 정보를 보냈습니다", "The host sent invalid auth parameters"),
            s.iterations
        );
    }
    let binding = channel_binding(&conn)?;
    let password = password.to_string();
    // 입력한 비밀 하나로 슬롯마다 키를 만든다 (표시 비밀번호인지 접속 코드인지 몰라도 된다).
    // PBKDF2 는 CPU 를 오래 쓰므로 블로킹 스레드에서 돌린다.
    let keys: Vec<[u8; 32]> = tokio::task::spawn_blocking(move || {
        slots.iter().map(|s| derive_key(&password, &s.salt, s.iterations)).collect()
    })
    .await?;
    let macs = keys.iter().map(|k| compute_mac(k, &nonce, &binding)).collect();
    write_msg(&mut send, &ClientMsg::AuthResponse { macs }).await?;

    let reply: ServerMsg = read_msg(&mut recv)
        .await?
        .context(t!("호스트가 인증 중에 연결을 닫았습니다", "The host closed the connection during authentication"))?;
    let (host_name, host_os, displays, via_access_code) = match reply {
        ServerMsg::AuthOk { slot, via_access_code, server_mac, host_name, host_os, displays } => {
            let key = keys.get(slot as usize).context("호스트가 없는 슬롯 번호를 보냄")?;
            if !mac_eq(&server_mac, &compute_server_mac(key, &nonce, &binding)) {
                bail!(
                    "{}",
                    t!(
                        "호스트가 비밀번호를 증명하지 못했습니다. 가짜 호스트일 수 있어 연결을 끊었습니다",
                        "The host couldn't prove it knows the password. It may be an impostor, so the connection was closed"
                    )
                );
            }
            (host_name, host_os, displays, via_access_code)
        }
        ServerMsg::AuthFailed { reason } => bail!("{}: {reason}", t!("인증 실패", "Authentication failed")),
        ServerMsg::Error { message } => bail!("{message}"),
        other => {
            bail!("{} ({other:?})", t!("호스트가 예상하지 못한 응답을 보냈습니다", "Unexpected reply from the host"))
        }
    };
    if expected.as_deref() != Some(c.fingerprint.as_str()) {
        known.set(host_key, &c.fingerprint)?;
    }
    Ok(Session { endpoint: c.endpoint, conn, send, recv, host_name, host_os, displays, via_access_code })
}

fn client_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "viewer".into())
}
