//! Hysteria2 protocol client implementation.
//!
//! Hysteria2 is a QUIC-based proxy protocol that uses HTTP/3 for authentication
//! and raw QUIC streams for TCP proxying. It tunnels TCP connections over QUIC
//! with HTTP/3-style masquerading.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Once;
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};

/// Hysteria2 outbound connector.
///
/// Uses QUIC (quinn) for transport with HTTP/3 authentication and
/// raw bidirectional streams for TCP proxy requests.
pub struct Hysteria2Outbound {
    server: String,
    port: u16,
    password: String,
    sni: String,
    /// Pre-built QUIC client config (avoids rebuilding root certs per reconnect)
    quic_client_config: quinn::ClientConfig,
    state: tokio::sync::RwLock<Option<(quinn::Endpoint, quinn::Connection)>>,
}

impl Hysteria2Outbound {
    pub fn new(
        server: &str,
        port: u16,
        password: &str,
        tls_config: Option<&TlsConfig>,
    ) -> Result<Self> {
        let sni = tls_config
            .and_then(|t| t.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let skip_cert_verify = tls_config.map(|t| t.skip_cert_verify).unwrap_or(false);
        let alpn = tls_config
            .and_then(|t| t.alpn.as_ref())
            .cloned()
            .unwrap_or_else(|| vec!["h3".to_string()]);

        // Build QUIC TLS config once
        ensure_crypto_provider();
        let quic_client_config = build_quic_client_config(skip_cert_verify, &alpn)?;

        Ok(Self {
            server: server.to_string(),
            port,
            password: password.to_string(),
            sni,
            quic_client_config,
            state: tokio::sync::RwLock::new(None),
        })
    }

    /// Get or create QUIC connection with authentication.
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
        // Double-check after acquiring write lock (another task may have reconnected)
        if let Some((_, ref conn)) = *guard {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }

        let (endpoint, conn) = self.create_connection().await?;
        *guard = Some((endpoint, conn.clone()));

        // Authenticate via HTTP/3
        self.authenticate_h3(&conn).await?;

        Ok(conn)
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let mut client_config = self.quic_client_config.clone();

        // Transport config: BBR congestion control for Hysteria2
        let mut transport = quinn::TransportConfig::default();
        transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(15)));
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        transport.receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
        transport.send_window(16 * 1024 * 1024);
        client_config.transport_config(Arc::new(transport));

        let addr = resolve_server(&self.server, self.port).await?;
        tracing::debug!(
            "Hysteria2 resolved {}:{} -> {}",
            self.server,
            self.port,
            addr
        );

        // Bind to matching address family
        let bind_addr: std::net::SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config);

        let connecting = endpoint.connect(addr, &self.sni)?;
        let connection = tokio::time::timeout(std::time::Duration::from_secs(15), connecting)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Hysteria2 connection to {}:{} timed out (UDP may be blocked by firewall)",
                    self.server,
                    self.port
                )
            })??;

        tracing::info!(
            "Hysteria2 QUIC connection established to {}:{} ({})",
            self.server,
            self.port,
            addr,
        );

        Ok((endpoint, connection))
    }

    /// Authenticate via HTTP/3 POST /auth request.
    ///
    /// Per the Hysteria2 spec, the client sends an HTTP/3 request:
    ///   POST /auth with Hysteria-Auth header containing the password.
    /// The server responds with status 233 if authentication succeeds.
    async fn authenticate_h3(&self, conn: &quinn::Connection) -> Result<()> {
        let h3_conn = h3_quinn::Connection::new(conn.clone());
        let (_conn, mut sender) = h3::client::new(h3_conn).await?;

        // Build the auth request
        let req = http::Request::builder()
            .method("POST")
            .uri("https://hysteria/auth")
            .header("Hysteria-Auth", &self.password)
            .header("Hysteria-CC-RX", "0")
            .body(())
            .map_err(|e| anyhow::anyhow!("Failed to build auth request: {}", e))?;

        let mut stream = sender.send_request(req).await?;
        stream.finish().await?;

        let resp = stream.recv_response().await?;

        if resp.status() != 233 {
            bail!(
                "Hysteria2 auth failed: server returned status {} (expected 233)",
                resp.status()
            );
        }

        tracing::info!("Hysteria2 authentication successful (HTTP/3 status 233)");

        // Drop the h3 client — we now use raw QUIC streams for proxy requests
        drop(sender);

        Ok(())
    }
}

fn ensure_crypto_provider() {
    static INSTALL_PROVIDER: Once = Once::new();
    INSTALL_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Outbound for Hysteria2Outbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            let conn = self.get_connection().await?;

            // Open bidirectional stream for this TCP connection
            let (mut send, mut recv) = conn.open_bi().await?;

            // Hysteria2 TCP request (per protocol spec):
            //   varint 0x401 (TCPRequest ID)
            //   varint address_length
            //   bytes  address string ("host:port")
            //   varint padding_length (0)
            //   bytes  padding (empty)
            let addr_str = format!("{}:{}", host, port);
            let addr_bytes = addr_str.as_bytes();

            write_varint(&mut send, 0x0401).await?;
            write_varint(&mut send, addr_bytes.len() as u64).await?;
            send.write_all(addr_bytes).await?;
            write_varint(&mut send, 0).await?; // no padding
            send.flush().await?;

            // Read the server's TCPResponse:
            //   uint8  status (0x00 = OK, 0x01 = Error)
            //   varint message_length
            //   bytes  message string
            //   varint padding_length
            //   bytes  padding
            let mut status_buf = [0u8; 1];
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                recv.read_exact(&mut status_buf),
            )
            .await;

            if status_buf[0] == 0x01 {
                bail!("Hysteria2 TCP connect to {} rejected by server", addr_str);
            }

            tracing::debug!("Hysteria2 TCP connect to {}", addr_str);

            let stream = Hy2BidiStream { send, recv };
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "hysteria2"
    }
}

/// Combined bidirectional QUIC stream for Hysteria2.
struct Hy2BidiStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

impl AsyncRead for Hy2BidiStream {
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

impl AsyncWrite for Hy2BidiStream {
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

impl Unpin for Hy2BidiStream {}

// --- Protocol helpers ---

/// Write a QUIC variable-length integer.
async fn write_varint(writer: &mut quinn::SendStream, value: u64) -> Result<()> {
    if value < 64 {
        writer.write_all(&[value as u8]).await?;
    } else if value < 16384 {
        let bytes = ((value as u16) | 0x4000).to_be_bytes();
        writer.write_all(&bytes).await?;
    } else if value < 1_073_741_824 {
        let bytes = ((value as u32) | 0x80000000).to_be_bytes();
        writer.write_all(&bytes).await?;
    } else {
        let bytes = (value | 0xC000000000000000).to_be_bytes();
        writer.write_all(&bytes).await?;
    }
    Ok(())
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
            .with_custom_certificate_verifier(Arc::new(InsecureHy2Verifier))
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

/// Resolve server hostname to SocketAddr, preferring IPv4.
async fn resolve_server(server: &str, port: u16) -> Result<SocketAddr> {
    use tokio::net::lookup_host;
    let addrs: Vec<_> = lookup_host(format!("{}:{}", server, port)).await?.collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve {}", server))
}

/// Insecure certificate verifier for skip_cert_verify option.
#[derive(Debug)]
struct InsecureHy2Verifier;

impl rustls::client::danger::ServerCertVerifier for InsecureHy2Verifier {
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
    fn test_hysteria2_outbound_new() {
        let outbound = Hysteria2Outbound::new("server.com", 443, "my_password", None).unwrap();
        assert_eq!(outbound.name(), "hysteria2");
        assert_eq!(outbound.server, "server.com");
        assert_eq!(outbound.port, 443);
        assert_eq!(outbound.sni, "server.com");
    }

    #[test]
    fn test_hysteria2_with_tls_config() {
        let tls = TlsConfig {
            sni: Some("custom.sni.com".to_string()),
            skip_cert_verify: true,
            alpn: Some(vec!["h3".to_string()]),
            fingerprint: None,
        };
        let outbound = Hysteria2Outbound::new("server.com", 443, "pass", Some(&tls)).unwrap();
        assert_eq!(outbound.sni, "custom.sni.com");
    }
}
