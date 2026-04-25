//! TUIC v5 protocol client implementation.
//!
//! TUIC v5 runs over QUIC (quinn), providing multiplexed streams with
//! a single persistent QUIC connection per outbound.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Once;
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};

// TUIC v5 protocol constants
const TUIC_VERSION: u8 = 0x05;
const CMD_AUTHENTICATE: u8 = 0x00;
const CMD_CONNECT: u8 = 0x01;
#[allow(dead_code)]
const CMD_PACKET: u8 = 0x02;
#[allow(dead_code)]
const CMD_DISSOCIATE: u8 = 0x03;
#[allow(dead_code)]
const CMD_HEARTBEAT: u8 = 0x04;
const TUIC_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);
const TUIC_CONNECT_ATTEMPTS_PER_ADDR: usize = 2;
const TUIC_CONNECT_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);

// Address types
const ADDR_TYPE_DOMAIN: u8 = 0x00;
const ADDR_TYPE_IPV4: u8 = 0x01;
const ADDR_TYPE_IPV6: u8 = 0x02;

/// TUIC v5 outbound connector.
pub struct TuicOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    password: String,
    sni: String,
    congestion: CongestionControl,
    /// Pre-built QUIC client config (avoids rebuilding root certs per reconnect)
    quic_client_config: quinn::ClientConfig,
    state: tokio::sync::RwLock<Option<(quinn::Endpoint, quinn::Connection)>>,
}

#[derive(Clone, Copy, Debug)]
enum CongestionControl {
    Cubic,
    NewReno,
    Bbr,
}

impl CongestionControl {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cubic" => Self::Cubic,
            "new_reno" | "newreno" => Self::NewReno,
            "bbr" => Self::Bbr,
            _ => Self::Cubic,
        }
    }
}

impl TuicOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        password: &str,
        congestion: &str,
        tls_config: Option<&TlsConfig>,
    ) -> Result<Self> {
        let uuid = parse_uuid_bytes(uuid_str)?;
        let sni = tls_config
            .and_then(|t| t.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let skip_cert_verify = tls_config.map(|t| t.skip_cert_verify).unwrap_or(false);
        let alpn = tls_config
            .and_then(|t| t.alpn.as_ref())
            .cloned()
            .unwrap_or_else(|| vec!["h3".to_string()]);
        let congestion = CongestionControl::from_str(congestion);

        // Build QUIC TLS config once
        ensure_crypto_provider();
        let quic_client_config = build_quic_client_config(skip_cert_verify, &alpn)?;

        Ok(Self {
            server: server.to_string(),
            port,
            uuid,
            password: password.to_string(),
            sni,
            congestion,
            quic_client_config,
            state: tokio::sync::RwLock::new(None),
        })
    }

    /// Get or create QUIC connection, authenticating on first connect.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        // Fast path: read lock to check/reuse existing connection
        {
            let guard = self.state.read().await;
            if let Some((_, ref conn)) = *guard {
                if conn.close_reason().is_none() {
                    return Ok(conn.clone());
                }
            }
        }

        // Slow path: write lock to create a new connection
        let mut guard = self.state.write().await;
        // Double-check after acquiring write lock
        if let Some((_, ref conn)) = *guard {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
            tracing::debug!("TUIC connection closed, reconnecting...");
        }

        // Create new connection (endpoint + connection)
        let (endpoint, conn) = self.create_connection().await.map_err(|e| {
            tracing::error!(
                "TUIC connection to {}:{} failed: {}",
                self.server,
                self.port,
                e
            );
            e
        })?;
        *guard = Some((endpoint, conn.clone()));

        // Authenticate
        self.authenticate(&conn).await?;

        Ok(conn)
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let mut client_config = self.quic_client_config.clone();

        // Set transport config with congestion control
        let mut transport = quinn::TransportConfig::default();
        match self.congestion {
            CongestionControl::Bbr => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::BbrConfig::default(),
                ));
            }
            CongestionControl::NewReno => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::NewRenoConfig::default(),
                ));
            }
            CongestionControl::Cubic => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::CubicConfig::default(),
                ));
            }
        }
        // Keep-alive for long-lived connections
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(15)));
        // Max idle timeout to detect dead connections
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        client_config.transport_config(Arc::new(transport));

        let addrs = resolve_server_addrs(&self.server, self.port).await?;
        tracing::debug!("TUIC resolved {}:{} -> {:?}", self.server, self.port, addrs);

        let mut last_err = None;

        for addr in addrs {
            for attempt in 1..=TUIC_CONNECT_ATTEMPTS_PER_ADDR {
                let bind_addr: SocketAddr = if addr.is_ipv4() {
                    "0.0.0.0:0".parse()?
                } else {
                    "[::]:0".parse()?
                };
                let mut endpoint = quinn::Endpoint::client(bind_addr)?;
                endpoint.set_default_client_config(client_config.clone());

                let connecting = endpoint.connect(addr, &self.sni)?;
                match tokio::time::timeout(TUIC_CONNECT_TIMEOUT, connecting).await {
                    Ok(Ok(connection)) => {
                        tracing::info!(
                            "TUIC QUIC connection established to {}:{} ({}) on attempt {}/{}",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            TUIC_CONNECT_ATTEMPTS_PER_ADDR
                        );
                        return Ok((endpoint, connection));
                    }
                    Ok(Err(err)) => {
                        last_err = Some(anyhow::anyhow!(
                            "TUIC QUIC connection to {}:{} via {} failed on attempt {}/{}: {}",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            TUIC_CONNECT_ATTEMPTS_PER_ADDR,
                            err
                        ));
                    }
                    Err(_) => {
                        last_err = Some(anyhow::anyhow!(
                            "TUIC QUIC connection to {}:{} via {} timed out on attempt {}/{} after {}s (UDP may be blocked or unstable)",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            TUIC_CONNECT_ATTEMPTS_PER_ADDR,
                            TUIC_CONNECT_TIMEOUT.as_secs()
                        ));
                    }
                }

                if attempt < TUIC_CONNECT_ATTEMPTS_PER_ADDR {
                    tokio::time::sleep(TUIC_CONNECT_RETRY_BACKOFF).await;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!(
                "TUIC QUIC connection to {}:{} failed without a concrete error",
                self.server,
                self.port
            )
        }))
    }

    /// Send authentication on a unidirectional stream.
    async fn authenticate(&self, conn: &quinn::Connection) -> Result<()> {
        let mut send = conn.open_uni().await?;

        let token = export_auth_token(conn, &self.uuid, &self.password)?;
        send.write_u8(TUIC_VERSION).await?;
        send.write_u8(CMD_AUTHENTICATE).await?;
        send.write_all(&self.uuid).await?;
        send.write_all(&token).await?;
        send.flush().await?;

        send.finish()?;
        tracing::info!("TUIC authentication sent");

        Ok(())
    }
}

fn ensure_crypto_provider() {
    static INSTALL_PROVIDER: Once = Once::new();

    INSTALL_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Outbound for TuicOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            let conn = self.get_connection().await?;

            // Open bidirectional stream for this TCP connection
            let (mut send, recv) = conn.open_bi().await?;

            // Send Connect command — must flush so server receives it before relay data
            send.write_u8(TUIC_VERSION).await?;
            send.write_u8(CMD_CONNECT).await?;
            write_address(&mut send, &host, port).await?;
            send.flush().await?;

            tracing::debug!("TUIC connect to {}:{}", host, port);

            let stream = QuicBidiStream { send, recv };
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "tuic"
    }
}

/// Combined bidirectional QUIC stream.
struct QuicBidiStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

impl AsyncRead for QuicBidiStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().recv).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for QuicBidiStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.get_mut().send).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().send).poll_flush(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().send).poll_shutdown(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Unpin for QuicBidiStream {}

// --- Protocol helpers ---

/// Write TUIC address (type + addr + port).
async fn write_address(writer: &mut quinn::SendStream, host: &str, port: u16) -> Result<()> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                writer.write_u8(ADDR_TYPE_IPV4).await?;
                writer.write_all(&v4.octets()).await?;
            }
            IpAddr::V6(v6) => {
                writer.write_u8(ADDR_TYPE_IPV6).await?;
                writer.write_all(&v6.octets()).await?;
            }
        }
    } else {
        let domain = host.as_bytes();
        if domain.len() > 255 {
            bail!("Domain name too long: {}", host);
        }
        writer.write_u8(ADDR_TYPE_DOMAIN).await?;
        writer.write_u8(domain.len() as u8).await?;
        writer.write_all(domain).await?;
    }
    writer.write_all(&port.to_be_bytes()).await?;
    Ok(())
}

/// Parse UUID string to 16 bytes.
fn parse_uuid_bytes(uuid_str: &str) -> Result<[u8; 16]> {
    let uuid = uuid::Uuid::parse_str(uuid_str)?;
    Ok(*uuid.as_bytes())
}

fn export_auth_token(
    conn: &quinn::Connection,
    uuid: &[u8; 16],
    password: &str,
) -> Result<[u8; 32]> {
    let mut token = [0u8; 32];
    conn.export_keying_material(&mut token, uuid, password.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to export TUIC auth token: {:?}", err))?;
    Ok(token)
}

/// Globally cached root cert store for QUIC TLS (built once).
static QUIC_ROOT_CERTS: std::sync::OnceLock<rustls::RootCertStore> = std::sync::OnceLock::new();

fn get_quic_root_certs() -> &'static rustls::RootCertStore {
    QUIC_ROOT_CERTS.get_or_init(|| {
        let mut store = rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        store
    })
}

/// Build a quinn::ClientConfig with TLS settings (reusable across reconnections).
fn build_quic_client_config(
    skip_cert_verify: bool,
    alpn: &[String],
) -> Result<quinn::ClientConfig> {
    let mut rustls_config = if skip_cert_verify {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureQuicVerifier))
            .with_no_client_auth()
    } else {
        rustls::ClientConfig::builder()
            .with_root_certificates(get_quic_root_certs().clone())
            .with_no_client_auth()
    };

    if !alpn.is_empty() {
        rustls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
        .map_err(|e| anyhow::anyhow!("QUIC TLS config error: {}", e))?;

    Ok(quinn::ClientConfig::new(Arc::new(quic_config)))
}

fn prefer_socket_addrs(mut addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
    addrs.dedup();
    addrs
}

/// Resolve server hostname to SocketAddr candidates, preferring IPv4 first.
async fn resolve_server_addrs(server: &str, port: u16) -> Result<Vec<SocketAddr>> {
    use tokio::net::lookup_host;
    let addrs: Vec<_> = lookup_host(format!("{}:{}", server, port)).await?.collect();
    let addrs = prefer_socket_addrs(addrs);
    if addrs.is_empty() {
        bail!("Failed to resolve {}", server);
    }
    Ok(addrs)
}

/// Insecure certificate verifier for skip_cert_verify option.
#[derive(Debug)]
struct InsecureQuicVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureQuicVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uuid() {
        let uuid = parse_uuid_bytes("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid[0], 0x55);
        assert_eq!(uuid[15], 0x00);
    }

    #[test]
    fn test_congestion_control() {
        assert!(matches!(
            CongestionControl::from_str("bbr"),
            CongestionControl::Bbr
        ));
        assert!(matches!(
            CongestionControl::from_str("cubic"),
            CongestionControl::Cubic
        ));
        assert!(matches!(
            CongestionControl::from_str("new_reno"),
            CongestionControl::NewReno
        ));
        assert!(matches!(
            CongestionControl::from_str("unknown"),
            CongestionControl::Cubic
        ));
    }

    #[test]
    fn test_tuic_outbound_new() {
        let outbound = TuicOutbound::new(
            "server.com",
            443,
            "550e8400-e29b-41d4-a716-446655440000",
            "my_password",
            "bbr",
            None,
        )
        .unwrap();
        assert_eq!(outbound.name(), "tuic");
        assert_eq!(outbound.server, "server.com");
        assert_eq!(outbound.port, 443);
    }

    #[test]
    fn test_protocol_constants() {
        assert_eq!(TUIC_VERSION, 0x05);
        assert_eq!(CMD_AUTHENTICATE, 0x00);
        assert_eq!(CMD_CONNECT, 0x01);
        assert_eq!(CMD_PACKET, 0x02);
        assert_eq!(CMD_DISSOCIATE, 0x03);
        assert_eq!(CMD_HEARTBEAT, 0x04);
        assert_eq!(ADDR_TYPE_DOMAIN, 0x00);
        assert_eq!(ADDR_TYPE_IPV4, 0x01);
        assert_eq!(ADDR_TYPE_IPV6, 0x02);
    }

    #[test]
    fn test_prefer_socket_addrs_prefers_ipv4_and_dedups() {
        let ordered = prefer_socket_addrs(vec![
            "[2001:db8::1]:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "[2001:db8::2]:443".parse().unwrap(),
        ]);

        assert_eq!(ordered.len(), 3);
        assert!(ordered[0].is_ipv4());
        assert!(ordered[1].is_ipv6());
        assert!(ordered[2].is_ipv6());
    }
}
