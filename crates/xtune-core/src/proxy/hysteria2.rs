//! Hysteria2 protocol client implementation.
//!
//! Hysteria2 is a QUIC-based proxy protocol that uses HTTP/3-like semantics.
//! It tunnels TCP connections over QUIC streams with password authentication.

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

/// Hysteria2 outbound connector.
///
/// Uses QUIC (quinn) for transport with HTTP/3-style CONNECT requests.
pub struct Hysteria2Outbound {
    server: String,
    port: u16,
    password: String,
    sni: String,
    skip_cert_verify: bool,
    alpn: Vec<String>,
    state: tokio::sync::Mutex<Option<(quinn::Endpoint, quinn::Connection)>>,
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

        Ok(Self {
            server: server.to_string(),
            port,
            password: password.to_string(),
            sni,
            skip_cert_verify,
            alpn,
            state: tokio::sync::Mutex::new(None),
        })
    }

    /// Get or create QUIC connection with authentication.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        let mut guard = self.state.lock().await;

        // Reuse existing connection if alive
        if let Some((_, ref conn)) = *guard {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }

        let (endpoint, conn) = self.create_connection().await?;
        *guard = Some((endpoint, conn.clone()));

        // Authenticate via a unidirectional stream
        self.authenticate(&conn).await?;

        Ok(conn)
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        ensure_crypto_provider();

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let rustls_config = if self.skip_cert_verify {
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureHy2Verifier))
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
            .map_err(|e| anyhow::anyhow!("Hysteria2 QUIC TLS config error: {}", e))?;

        let mut client_config = quinn::ClientConfig::new(Arc::new(quic_config));

        // Transport config: BBR congestion control for Hysteria2
        let mut transport = quinn::TransportConfig::default();
        transport.congestion_controller_factory(Arc::new(
            quinn::congestion::BbrConfig::default(),
        ));
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(15)));
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        transport.receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
        transport.send_window(16 * 1024 * 1024);
        client_config.transport_config(Arc::new(transport));

        let addr = resolve_server(&self.server, self.port).await?;
        tracing::debug!("Hysteria2 resolved {}:{} -> {}", self.server, self.port, addr);

        // Bind to matching address family
        let bind_addr: std::net::SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config);

        let connecting = endpoint.connect(addr, &self.sni)?;
        let connection = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            connecting,
        )
        .await
        .map_err(|_| anyhow::anyhow!(
            "Hysteria2 connection to {}:{} timed out after 15s",
            self.server, self.port
        ))??;

        tracing::info!(
            "Hysteria2 QUIC connection established to {}:{} ({})",
            self.server,
            self.port,
            addr,
        );

        Ok((endpoint, connection))
    }

    /// Send Hysteria2 authentication request.
    ///
    /// Hysteria2 auth is sent as an HTTP/3-style request on a unidirectional stream:
    /// The server validates the password and allows subsequent CONNECT requests.
    async fn authenticate(&self, conn: &quinn::Connection) -> Result<()> {
        let mut send = conn.open_uni().await?;

        // Hysteria2 auth frame: HTTP/3-like header with auth token
        // Format: varint frame_type(0x401) + varint length + auth_data
        // The auth_data is the password string
        let auth_data = self.password.as_bytes();

        // Write Hysteria2 client auth frame
        write_varint(&mut send, 0x0401).await?; // Hysteria2 auth frame type
        write_varint(&mut send, auth_data.len() as u64).await?;
        send.write_all(auth_data).await?;
        send.flush().await?;
        send.finish()?;

        tracing::info!("Hysteria2 authentication sent");
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
            let (mut send, recv) = conn.open_bi().await?;

            // Hysteria2 TCP request header:
            // varint request_id(0x0401) + address_type + address + port
            write_varint(&mut send, 0x0401).await?;

            // Write address
            if let Ok(ip) = host.parse::<IpAddr>() {
                match ip {
                    IpAddr::V4(v4) => {
                        send.write_all(&[0x01]).await?; // IPv4
                        send.write_all(&v4.octets()).await?;
                    }
                    IpAddr::V6(v6) => {
                        send.write_all(&[0x03]).await?; // IPv6
                        send.write_all(&v6.octets()).await?;
                    }
                }
            } else {
                let domain = host.as_bytes();
                if domain.len() > 255 {
                    bail!("Domain name too long: {}", host);
                }
                send.write_all(&[0x00]).await?; // Domain
                write_varint(&mut send, domain.len() as u64).await?;
                send.write_all(domain).await?;
            }
            send.write_all(&port.to_be_bytes()).await?;
            // Padding (0 bytes)
            write_varint(&mut send, 0).await?;
            send.flush().await?;

            tracing::debug!("Hysteria2 connect to {}:{}", host, port);

            let stream = Hy2BidiStream {
                send,
                recv,
            };
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
        let outbound =
            Hysteria2Outbound::new("server.com", 443, "pass", Some(&tls)).unwrap();
        assert_eq!(outbound.sni, "custom.sni.com");
        assert!(outbound.skip_cert_verify);
    }
}
