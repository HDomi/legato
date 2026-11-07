//! 호스트 접속, 상호 인증, 제어·영상 스트림 처리.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, bail};
use legato_core::auth::{compute_mac, compute_server_mac, derive_key, mac_eq, channel_binding};
use legato_core::framing::{read_msg, write_msg};
use legato_core::known_hosts::KnownHosts;
use legato_core::protocol::{ClientMsg, DisplayInfo, MAX_AUTH_SLOTS, Os, PROTOCOL_VERSION, ServerMsg};

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

/// 접속하고 비밀번호로 인증한다.
/// 처음 보는 호스트는 상호 인증이 끝난 뒤 지문을 기억한다. 기억한 지문과 다르면 거부한다.
pub async fn connect(addr: SocketAddr, host_key: &str, password: &str, accept_new_fingerprint: bool) -> Result<Session> {
    let mut known = KnownHosts::load()?;
    let expected = if accept_new_fingerprint { None } else { known.get(host_key).map(str::to_string) };
    let c = legato_core::transport::connect(addr, expected.clone()).await.map_err(|e| {
        if expected.is_some() && format!("{e:#}").contains("지문 불일치") {
            anyhow!(
                "{host_key} 의 인증서 지문이 예전과 달라. 호스트를 다시 설치했거나 중간에서 가로채는 중일 수 있어. \
                 확실하면 --accept-new-fingerprint 로 다시 접속해 ({e:#})"
            )
        } else {
            e
        }
    })?;
    let conn = c.connection;
    let (mut send, mut recv) = conn.open_bi().await.context("제어 스트림 열기 실패")?;

    write_msg(&mut send, &ClientMsg::Hello { version: PROTOCOL_VERSION, client_name: client_name(), client_os: Os::current() }).await?;
    let challenge: ServerMsg = read_msg(&mut recv).await?.context("호스트가 연결을 닫음")?;
    let (slots, nonce) = match challenge {
        ServerMsg::Challenge { slots, nonce } => (slots, nonce),
        ServerMsg::Error { message } => bail!("{message}"),
        other => bail!("예상하지 못한 메시지: {other:?}"),
    };
    if slots.is_empty() || slots.len() > MAX_AUTH_SLOTS {
        bail!("호스트가 이상한 인증 슬롯 수를 보냄: {}", slots.len());
    }
    if let Some(s) = slots.iter().find(|s| !(10_000..=10_000_000).contains(&s.iterations)) {
        bail!("호스트가 이상한 PBKDF2 반복 횟수를 보냄: {}", s.iterations);
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

    let reply: ServerMsg = read_msg(&mut recv).await?.context("호스트가 인증 중 연결을 닫음")?;
    let (host_name, host_os, displays, via_access_code) = match reply {
        ServerMsg::AuthOk { slot, via_access_code, server_mac, host_name, host_os, displays } => {
            let key = keys.get(slot as usize).context("호스트가 없는 슬롯 번호를 보냄")?;
            if !mac_eq(&server_mac, &compute_server_mac(key, &nonce, &binding)) {
                bail!("호스트가 비밀번호를 증명하지 못했어. 가짜 호스트일 수 있어서 연결을 끊었어");
            }
            (host_name, host_os, displays, via_access_code)
        }
        ServerMsg::AuthFailed { reason } => bail!("인증 실패: {reason}"),
        ServerMsg::Error { message } => bail!("{message}"),
        other => bail!("예상하지 못한 메시지: {other:?}"),
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
