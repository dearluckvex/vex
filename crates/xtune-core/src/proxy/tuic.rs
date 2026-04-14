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
    skip_cert_verify: bool,
    alpn: Vec<String>,
    congestion: CongestionControl,
    // Keep the endpoint alive alongside the connection — dropping the endpoint
    // shuts down the UDP socket and QUIC driver, breaking the connection.
    state: tokio::sync::Mutex<Option<(quinn::Endpoint, quinn::Connection)>>,
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

        Ok(Self {
            server: server.to_string(),
            port,
            uuid,
            password: password.to_string(),
            sni,
            skip_cert_verify,
            alpn,
            congestion: CongestionControl::from_str(congestion),
            state: tokio::sync::Mutex::new(None),
        })
    }

    /// Get or create QUIC connection, authenticating on first connect.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        let mut guard = self.state.lock().await;

        // Check if existing connection is still alive
        if let Some((_, ref conn)) = *guard {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }

        // Create new connection (endpoint + connection)
        let (endpoint, conn) = self.create_connection().await?;
        *guard = Some((endpoint, conn.clone()));

        // Authenticate
        self.authenticate(&conn).await?;

        Ok(conn)
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        ensure_crypto_provider();

        // Build rustls config for QUIC
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let rustls_config = if self.skip_cert_verify {
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureQuicVerifier))
                .with_no_client_auth()
        } else {
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth()
        };

        let mut rustls_config = rustls_config;
        if !self.alpn.is_empty() {
            rustls_config.alpn_protocols =
                self.alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
        }

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
            .map_err(|e| anyhow::anyhow!("QUIC TLS config error: {}", e))?;

        let mut client_config = quinn::ClientConfig::new(Arc::new(quic_config));

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
        client_config.transport_config(Arc::new(transport));

        // Resolve server address
        let addr = resolve_server(&self.server, self.port).await?;

        // Create endpoint (bind to any local address)
        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);

        // Connect
        let connection = endpoint.connect(addr, &self.sni)?.await?;
        tracing::info!(
            "TUIC QUIC connection established to {}:{}",
            self.server,
            self.port
        );

        Ok((endpoint, connection))
    }

    /// Send authentication on a unidirectional stream.
    async fn authenticate(&self, conn: &quinn::Connection) -> Result<()> {
        let mut send = conn.open_uni().await?;

        let token = export_auth_token(conn, &self.uuid, &self.password)?;
        send.write_u8(TUIC_VERSION).await?;
        send.write_u8(CMD_AUTHENTICATE).await?;
        send.write_all(&self.uuid).await?;
        send.write_all(&token).await?;

        send.finish()?;
        tracing::debug!("TUIC authentication sent");

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

            // Send Connect command
            send.write_u8(TUIC_VERSION).await?;
            send.write_u8(CMD_CONNECT).await?;
            write_address(&mut send, &host, port).await?;

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

/// Resolve server hostname to SocketAddr.
async fn resolve_server(server: &str, port: u16) -> Result<SocketAddr> {
    use tokio::net::lookup_host;
    let addrs: Vec<_> = lookup_host(format!("{}:{}", server, port)).await?.collect();
    addrs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve {}", server))
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
}
