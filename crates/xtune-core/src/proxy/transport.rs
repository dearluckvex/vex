use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result};
use craft_tls::TlsConnector;
use craft_tls::client::TlsStream;
use craft_tls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use craft_tls::rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

use crate::config::model::TlsConfig;

/// Default timeout for TCP connect + TLS handshake.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Socket send/receive buffer size (256KB — improves throughput on high-BDP links).
const SOCKET_BUF_SIZE: usize = 256 * 1024;
/// TCP keepalive interval (keeps connections alive through NATs/firewalls).
const TCP_KEEPALIVE_SECS: u64 = 30;

/// Globally cached root certificate store (built once, reused everywhere).
static ROOT_CERT_STORE: OnceLock<craft_tls::rustls::RootCertStore> = OnceLock::new();

fn get_root_cert_store() -> &'static craft_tls::rustls::RootCertStore {
    ROOT_CERT_STORE.get_or_init(|| {
        let mut store = craft_tls::rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        store
    })
}

/// Map a fingerprint name from config to a craftls FingerprintBuilder.
fn fingerprint_builder(name: Option<&str>) -> craft_tls::rustls::craft::FingerprintBuilder {
    use craft_tls::rustls::craft::*;
    match name.map(|s| s.to_lowercase()).as_deref() {
        Some("chrome") | None => CHROME_112.builder(),
        Some("safari") => SAFARI_17_1.builder(),
        Some("firefox") => FIREFOX_105.builder(),
        Some("chrome108") => CHROME_108.builder(),
        // Default to Chrome 112 for any unrecognized fingerprint
        _ => CHROME_112.builder(),
    }
}

/// Apply performance-critical socket options to a connected TCP stream:
/// TCP_NODELAY, enlarged send/receive buffers, and TCP keepalive.
fn tune_socket(stream: &TcpStream) {
    stream.set_nodelay(true).ok();
    let sock = SockRef::from(stream);
    sock.set_send_buffer_size(SOCKET_BUF_SIZE).ok();
    sock.set_recv_buffer_size(SOCKET_BUF_SIZE).ok();
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(TCP_KEEPALIVE_SECS));
    sock.set_tcp_keepalive(&keepalive).ok();
}

/// Resolve address string and connect TCP with a timeout, applying socket tuning.
async fn connect_tcp(addr: &str, timeout: Duration) -> Result<TcpStream> {
    let tcp = tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TCP connection to {} timed out after {}s",
                addr,
                timeout.as_secs()
            )
        })?
        .with_context(|| format!("TCP connection to {} failed", addr))?;
    tune_socket(&tcp);
    Ok(tcp)
}

/// Build a reusable TLS connector from config. Call once per outbound
/// and store the result — avoids rebuilding root cert store and
/// ClientConfig on every connection.
pub fn build_tls_connector(config: Option<&TlsConfig>) -> TlsConnector {
    let skip_verify = config.map(|c| c.skip_cert_verify).unwrap_or(false);
    let alpn = config.and_then(|c| c.alpn.as_ref());
    let fp_name = config.and_then(|c| c.fingerprint.as_deref());

    let base_config = if skip_verify {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(get_root_cert_store().clone())
            .with_no_client_auth()
    };

    let fp = if alpn.is_some() {
        fingerprint_builder(fp_name).do_not_override_alpn()
    } else {
        fingerprint_builder(fp_name)
    };
    let mut tls_config = base_config.with_fingerprint(fp);

    if let Some(alpn) = alpn {
        tls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    TlsConnector::from(Arc::new(tls_config))
}

/// Connect to a remote server with TLS using a pre-built connector.
pub async fn connect_tls_with(
    tcp: TcpStream,
    sni: &str,
    connector: &TlsConnector,
) -> Result<TlsStream<TcpStream>> {
    let server_name = ServerName::try_from(sni.to_string())?;
    let stream = connector
        .connect(server_name, tcp)
        .await
        .with_context(|| format!("TLS handshake failed with SNI '{}'", sni))?;
    Ok(stream)
}

/// Connect to a remote server with TLS using browser fingerprint emulation.
pub async fn connect_tls(
    tcp: TcpStream,
    sni: &str,
    config: Option<&TlsConfig>,
) -> Result<TlsStream<TcpStream>> {
    let connector = build_tls_connector(config);
    connect_tls_with(tcp, sni, &connector).await
}

/// Connect to a remote server over TCP, optionally wrapping with TLS.
/// Returns a boxed ProxyStream.
///
/// Applies a connection timeout (default 15s) to both the TCP connect
/// and the TLS handshake to prevent indefinite hangs.
pub async fn connect_with_tls(
    server: &str,
    port: u16,
    tls_config: Option<&TlsConfig>,
    use_tls: bool,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    connect_with_tls_timeout(server, port, tls_config, use_tls, DEFAULT_CONNECT_TIMEOUT).await
}

/// Like [`connect_with_tls`] but with a pre-built TLS connector for
/// connection reuse (avoids rebuilding root certs + fingerprints per call).
pub async fn connect_with_connector(
    server: &str,
    port: u16,
    connector: &TlsConnector,
    sni: &str,
    use_tls: bool,
    timeout: Duration,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    let addr = format!("{}:{}", server, port);
    let tcp = connect_tcp(&addr, timeout).await?;

    if use_tls {
        let tls = tokio::time::timeout(timeout, connect_tls_with(tcp, sni, connector))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "TLS handshake with {} (SNI: {}) timed out after {}s",
                    addr,
                    sni,
                    timeout.as_secs()
                )
            })??;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

/// Like [`connect_with_tls`] but with an explicit timeout.
pub async fn connect_with_tls_timeout(
    server: &str,
    port: u16,
    tls_config: Option<&TlsConfig>,
    use_tls: bool,
    timeout: Duration,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    let addr = format!("{}:{}", server, port);
    let tcp = connect_tcp(&addr, timeout).await?;

    if use_tls {
        let sni = tls_config.and_then(|c| c.sni.as_deref()).unwrap_or(server);
        let tls = tokio::time::timeout(timeout, connect_tls(tcp, sni, tls_config))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "TLS handshake with {} (SNI: {}) timed out after {}s",
                    addr,
                    sni,
                    timeout.as_secs()
                )
            })??;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

/// Certificate verifier that accepts all certificates (for skip_cert_verify).
#[derive(Debug)]
struct InsecureVerifier;

impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}
