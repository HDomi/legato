//! 호스트 접속, 상호 인증, 제어·영상 스트림 처리.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, bail};
use legato_core::auth::{compute_mac, compute_server_mac, derive_key, mac_eq, channel_binding};
use legato_core::framing::{read_msg, write_msg};
use legato_core::known_hosts::KnownHosts;
use legato_core::protocol::{ClientMsg, DisplayInfo, PROTOCOL_VERSION, ServerMsg};

/// 인증까지 끝난 세션.
pub struct Session {
    pub endpoint: quinn::Endpoint,
    pub conn: quinn::Connection,
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
    pub host_name: String,
    pub displays: Vec<DisplayInfo>,
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

    write_msg(&mut send, &ClientMsg::Hello { version: PROTOCOL_VERSION, client_name: client_name() }).await?;
    let challenge: ServerMsg = read_msg(&mut recv).await?.context("호스트가 연결을 닫음")?;
    let (salt, iterations, nonce) = match challenge {
        ServerMsg::Challenge { salt, iterations, nonce } => (salt, iterations, nonce),
        ServerMsg::Error { message } => bail!("{message}"),
        other => bail!("예상하지 못한 메시지: {other:?}"),
    };
    if !(10_000..=10_000_000).contains(&iterations) {
        bail!("호스트가 이상한 PBKDF2 반복 횟수를 보냄: {iterations}");
    }
    let binding = channel_binding(&conn)?;
    let password = password.to_string();
    // PBKDF2 는 CPU 를 오래 쓰므로 블로킹 스레드에서 돌린다.
    let key = tokio::task::spawn_blocking(move || derive_key(&password, &salt, iterations)).await?;
    write_msg(&mut send, &ClientMsg::AuthResponse { mac: compute_mac(&key, &nonce, &binding) }).await?;

    let reply: ServerMsg = read_msg(&mut recv).await?.context("호스트가 인증 중 연결을 닫음")?;
    let (host_name, displays) = match reply {
        ServerMsg::AuthOk { server_mac, host_name, displays } => {
            if !mac_eq(&server_mac, &compute_server_mac(&key, &nonce, &binding)) {
                bail!("호스트가 비밀번호를 증명하지 못했어. 가짜 호스트일 수 있어서 연결을 끊었어");
            }
            (host_name, displays)
        }
        ServerMsg::AuthFailed { reason } => bail!("인증 실패: {reason}"),
        ServerMsg::Error { message } => bail!("{message}"),
        other => bail!("예상하지 못한 메시지: {other:?}"),
    };
    if expected.as_deref() != Some(c.fingerprint.as_str()) {
        known.set(host_key, &c.fingerprint)?;
    }
    Ok(Session { endpoint: c.endpoint, conn, send, recv, host_name, displays })
}

fn client_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "viewer".into())
}
