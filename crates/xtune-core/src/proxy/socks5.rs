use anyhow::{Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::ProxyStats;
use super::connector::SharedOutbound;

/// SOCKS5 proxy server.
pub struct Socks5Server {
    listener: TcpListener,
    outbound: SharedOutbound,
    stats: ProxyStats,
}

impl Socks5Server {
    pub async fn bind(
        addr: &str,
        port: u16,
        outbound: SharedOutbound,
        stats: ProxyStats,
    ) -> Result<Self> {
        let listener = TcpListener::bind(format!("{}:{}", addr, port)).await?;
        tracing::info!("SOCKS5 server listening on {}:{}", addr, port);
        Ok(Self {
            listener,
            outbound,
            stats,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => {
                    tracing::info!("SOCKS5 server shutting down");
                    break;
                }
                result = self.listener.accept() => {
                    let (stream, peer) = result?;
                    let outbound = self.outbound.clone();
                    let stats = self.stats.clone();
                    tokio::spawn(async move {
                        stats.add_connection();
                        let result = handle_socks5(stream, outbound).await;
                        stats.remove_connection();
                        if let Err(e) = result {
                            tracing::debug!("SOCKS5 connection from {} error: {}", peer, e);
                        }
                    });
                }
            }
        }
        Ok(())
    }
}

/// Parse the SOCKS5 destination address from a stream.
async fn read_socks5_addr(stream: &mut TcpStream) -> Result<(String, u16)> {
    let atyp = stream.read_u8().await?;
    let host = match atyp {
        0x01 => {
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await?;
            std::net::Ipv4Addr::from(buf).to_string()
        }
        0x03 => {
            let len = stream.read_u8().await?;
            let mut buf = vec![0u8; len as usize];
            stream.read_exact(&mut buf).await?;
            String::from_utf8(buf)?
        }
        0x04 => {
            let mut buf = [0u8; 16];
            stream.read_exact(&mut buf).await?;
            std::net::Ipv6Addr::from(buf).to_string()
        }
        _ => bail!("Unsupported SOCKS5 address type: 0x{:02x}", atyp),
    };
    let port = stream.read_u16().await?;
    Ok((host, port))
}

/// Build a SOCKS5 reply packet.
fn socks5_reply(rep: u8) -> [u8; 10] {
    // VER=5, REP, RSV=0, ATYP=1(IPv4), BND.ADDR=0.0.0.0, BND.PORT=0
    [0x05, rep, 0x00, 0x01, 0, 0, 0, 0, 0, 0]
}

/// Handle a single SOCKS5 connection.
async fn handle_socks5(mut stream: TcpStream, outbound: SharedOutbound) -> Result<()> {
    // --- Auth Negotiation ---
    let version = stream.read_u8().await?;
    if version != 0x05 {
        bail!("Invalid SOCKS version: {}", version);
    }
    let nmethods = stream.read_u8().await?;
    let mut methods = vec![0u8; nmethods as usize];
    stream.read_exact(&mut methods).await?;

    // We accept NO AUTHENTICATION (0x00)
    if !methods.contains(&0x00) {
        // Reply: no acceptable methods
        stream.write_all(&[0x05, 0xFF]).await?;
        bail!("Client does not support NO AUTH method");
    }
    // Reply: chose NO AUTH
    stream.write_all(&[0x05, 0x00]).await?;

    // --- Request ---
    let ver = stream.read_u8().await?;
    if ver != 0x05 {
        bail!("Invalid SOCKS5 request version: {}", ver);
    }
    let cmd = stream.read_u8().await?;
    let _rsv = stream.read_u8().await?;

    match cmd {
        0x01 => {
            // CONNECT
            let (host, port) = read_socks5_addr(&mut stream).await?;
            tracing::debug!("SOCKS5 CONNECT to {}:{}", host, port);

            match outbound.connect(&host, port).await {
                Ok(mut remote) => {
                    stream.write_all(&socks5_reply(0x00)).await?; // success
                    let (up, down) =
                        tokio::io::copy_bidirectional(&mut stream, &mut remote).await?;
                    tracing::trace!(
                        "SOCKS5 relay {}:{} done: up={} down={}",
                        host,
                        port,
                        up,
                        down
                    );
                }
                Err(e) => {
                    stream.write_all(&socks5_reply(0x04)).await?; // host unreachable
                    return Err(e);
                }
            }
        }
        0x02 => {
            // BIND - not supported
            stream.write_all(&socks5_reply(0x07)).await?;
            bail!("SOCKS5 BIND not supported");
        }
        0x03 => {
            // UDP ASSOCIATE - not yet implemented
            stream.write_all(&socks5_reply(0x07)).await?;
            bail!("SOCKS5 UDP ASSOCIATE not yet implemented");
        }
        _ => {
            stream.write_all(&socks5_reply(0x07)).await?;
            bail!("Unknown SOCKS5 command: 0x{:02x}", cmd);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::connector::DirectOutbound;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Helper: start a simple echo server.
    async fn echo_server() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn test_socks5_connect_ipv4() {
        let (echo_port, _echo) = echo_server().await;

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = Socks5Server::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let socks_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        // Connect as SOCKS5 client
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();

        // Auth: NO AUTH
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // CONNECT to echo server (IPv4: 127.0.0.1)
        let mut req = vec![0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1];
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x05); // version
        assert_eq!(resp[1], 0x00); // success

        // Send and receive data through tunnel
        client.write_all(b"hello socks5").await.unwrap();
        let mut buf = [0u8; 12];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello socks5");

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_socks5_connect_domain() {
        let (echo_port, _echo) = echo_server().await;

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = Socks5Server::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let socks_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();

        // Auth
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // CONNECT with domain name "localhost"
        let domain = b"localhost";
        let mut req = vec![0x05, 0x01, 0x00, 0x03, domain.len() as u8];
        req.extend_from_slice(domain);
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[1], 0x00); // success

        client.write_all(b"domain test").await.unwrap();
        let mut buf = [0u8; 11];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"domain test");

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = server_handle.await;
    }
}
