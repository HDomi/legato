//! QUIC 엔드포인트 구성.
//!
//! 서버 인증서는 자체 서명이라 일반 CA 검증 대신 SHA-256 지문으로 확인한다.
//! 서명 자체(TLS 1.3 CertificateVerify)는 정상적으로 검증하므로 개인키 없는 쪽은 흉내 낼 수 없다.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};

use crate::identity::{HostIdentity, fingerprint};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// 영상 전송에 맞춘 QUIC 전송 설정.
fn transport_config() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().expect("고정값")));
    // 키프레임 하나가 수 MB 까지 갈 수 있어 수신 창을 넉넉히 잡는다.
    t.stream_receive_window((16u32 * 1024 * 1024).into());
    t.receive_window((32u32 * 1024 * 1024).into());
    t.send_window(32 * 1024 * 1024);
    t
}

/// 호스트용 QUIC 서버 엔드포인트를 연다.
pub fn server_endpoint(identity: &HostIdentity, bind: SocketAddr) -> Result<quinn::Endpoint> {
    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .context("서버 인증서 설정 실패")?;
    tls.alpn_protocols = vec![crate::ALPN.to_vec()];
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    cfg.transport_config(Arc::new(transport_config()));
    // 인증 전 연결이 쌓이지 않게 동시 수락 수를 제한한다.
    cfg.max_incoming(32);
    quinn::Endpoint::server(cfg, bind).with_context(|| format!("UDP {bind} 열기 실패"))
}

/// 서버 인증서 지문을 확인하는 검증기.
#[derive(Debug)]
struct FingerprintVerifier {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        *self.seen.lock().expect("뮤텍스") = Some(fp.clone());
        match &self.expected {
            Some(exp) if *exp != fp => Err(rustls::Error::General(format!(
                "호스트 지문 불일치 (기대 {exp}, 실제 {fp})"
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// 연결 결과. 실제로 본 서버 지문을 함께 돌려준다 (TOFU 저장용).
pub struct Connected {
    pub endpoint: quinn::Endpoint,
    pub connection: quinn::Connection,
    pub fingerprint: String,
}

/// 호스트에 접속한다. `expected` 가 있으면 지문이 다를 때 핸드셰이크를 거부한다.
pub async fn connect(addr: SocketAddr, expected: Option<String>) -> Result<Connected> {
    let seen = Arc::new(Mutex::new(None));
    let provider = provider();
    let verifier = FingerprintVerifier { expected, seen: seen.clone(), provider: provider.clone() };
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    tls.alpn_protocols = vec![crate::ALPN.to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    cfg.transport_config(Arc::new(transport_config()));

    let bind: SocketAddr = if addr.is_ipv6() { "[::]:0".parse()? } else { "0.0.0.0:0".parse()? };
    let mut endpoint = quinn::Endpoint::client(bind).context("클라이언트 UDP 소켓 열기 실패")?;
    endpoint.set_default_client_config(cfg);

    let connecting = endpoint.connect(addr, "legato.local")?;
    let connection = tokio::time::timeout(Duration::from_secs(8), connecting)
        .await
        .map_err(|_| anyhow!("{addr} 응답 없음 (포트·방화벽 확인)"))?
        .with_context(|| format!("{addr} 연결 실패"))?;
    let fingerprint = seen.lock().expect("뮤텍스").clone().ok_or_else(|| anyhow!("서버 인증서를 받지 못함"))?;
    Ok(Connected { endpoint, connection, fingerprint })
}
