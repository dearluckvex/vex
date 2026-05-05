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
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::quic_conn::{
    QuicConnectionState, build_quic_client_config, ensure_quic_crypto_provider,
    resolve_server_addrs,
};

const HY2_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);
const HY2_CONNECT_ATTEMPTS_PER_ADDR: usize = 2;
const HY2_CONNECT_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);

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
    conn_state: QuicConnectionState,
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
        ensure_quic_crypto_provider();
        let quic_client_config = build_quic_client_config(skip_cert_verify, &alpn)?;

        Ok(Self {
            server: server.to_string(),
            port,
            password: password.to_string(),
            sni,
            quic_client_config,
            conn_state: QuicConnectionState::new(),
        })
    }

    /// Get or create QUIC connection with authentication.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        if let Some(conn) = self.conn_state.get_existing().await {
            return Ok(conn);
        }
        let (endpoint, conn) = self.create_connection().await?;
        self.authenticate_h3(&conn).await?;
        Ok(self.conn_state.store_if_dead(endpoint, conn).await)
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let mut client_config = self.quic_client_config.clone();

        // Transport config: BBR congestion control for Hysteria2
        let mut transport = quinn::TransportConfig::default();
        transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        transport.receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
        transport.send_window(16 * 1024 * 1024);
        client_config.transport_config(Arc::new(transport));

        let addrs = resolve_server_addrs(&self.server, self.port).await?;
        tracing::debug!(
            "Hysteria2 resolved {}:{} -> {:?}",
            self.server,
            self.port,
            addrs
        );

        let mut last_err = None;

        for addr in addrs {
            for attempt in 1..=HY2_CONNECT_ATTEMPTS_PER_ADDR {
                let bind_addr: std::net::SocketAddr = if addr.is_ipv4() {
                    "0.0.0.0:0".parse()?
                } else {
                    "[::]:0".parse()?
                };
                let mut endpoint = quinn::Endpoint::client(bind_addr)?;
                endpoint.set_default_client_config(client_config.clone());

                let connecting = endpoint.connect(addr, &self.sni)?;
                match tokio::time::timeout(HY2_CONNECT_TIMEOUT, connecting).await {
                    Ok(Ok(connection)) => {
                        tracing::info!(
                            "Hysteria2 QUIC connection established to {}:{} ({}) on attempt {}/{}",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            HY2_CONNECT_ATTEMPTS_PER_ADDR
                        );
                        return Ok((endpoint, connection));
                    }
                    Ok(Err(err)) => {
                        last_err = Some(anyhow::anyhow!(
                            "Hysteria2 QUIC to {}:{} via {} failed on attempt {}/{}: {}",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            HY2_CONNECT_ATTEMPTS_PER_ADDR,
                            err
                        ));
                    }
                    Err(_) => {
                        last_err = Some(anyhow::anyhow!(
                            "Hysteria2 QUIC to {}:{} via {} timed out on attempt {}/{} after {}s",
                            self.server,
                            self.port,
                            addr,
                            attempt,
                            HY2_CONNECT_ATTEMPTS_PER_ADDR,
                            HY2_CONNECT_TIMEOUT.as_secs()
                        ));
                    }
                }

                if attempt < HY2_CONNECT_ATTEMPTS_PER_ADDR {
                    tokio::time::sleep(HY2_CONNECT_RETRY_BACKOFF).await;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!(
                "Hysteria2 QUIC connection to {}:{} failed without a concrete error",
                self.server,
                self.port
            )
        }))
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

impl Outbound for Hysteria2Outbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            let addr_str = format!("{}:{}", host, port);

            // Two attempts: handles the race where the cached QUIC connection
            // dies between get_connection() and open_bi() (stale connection).
            let mut last_err = anyhow::anyhow!("Hysteria2: no connection attempt made");
            for attempt in 1u8..=2 {
                let conn = self.get_connection().await?;
                let (mut send, mut recv) = match conn.open_bi().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        last_err = anyhow::anyhow!(
                            "Hysteria2 open_bi failed (attempt {}): {}",
                            attempt,
                            e
                        );
                        tracing::debug!("{last_err} — will retry with fresh connection");
                        continue;
                    }
                };

                // Hysteria2 TCP request (per protocol spec):
                //   varint 0x401 (TCPRequest ID)
                //   varint address_length
                //   bytes  address string ("host:port")
                //   varint padding_length (0)
                //   bytes  padding (empty)
                let addr_bytes = addr_str.as_bytes();
                write_varint(&mut send, 0x0401).await?;
                write_varint(&mut send, addr_bytes.len() as u64).await?;
                send.write_all(addr_bytes).await?;
                write_varint(&mut send, 0).await?; // no padding
                send.flush().await?;

                // Read the server's TCPResponse:
                //   uint8  status (0x00 = OK, 0x01 = Error)
                //   varint message_length + bytes message string + varint padding
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
                return Ok(Box::new(stream) as BoxProxyStream);
            }
            Err(last_err)
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
