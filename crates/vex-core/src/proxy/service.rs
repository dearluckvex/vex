use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::connector::SharedOutbound;
use super::http::HttpProxyServer;
use super::socks5::Socks5Server;
use super::{ProxyState, ProxyStats};

/// Central proxy service that manages SOCKS5 and HTTP proxy servers.
pub struct ProxyService {
    state_tx: watch::Sender<ProxyState>,
    state_rx: watch::Receiver<ProxyState>,
    shutdown_tx: Option<watch::Sender<bool>>,
    socks5_handle: Option<JoinHandle<()>>,
    http_handle: Option<JoinHandle<()>>,
    stats: ProxyStats,
    outbound: SharedOutbound,
}

impl ProxyService {
    /// Create a new proxy service with a direct outbound connector.
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(ProxyState::Disconnected);
        Self {
            state_tx,
            state_rx,
            shutdown_tx: None,
            socks5_handle: None,
            http_handle: None,
            stats: ProxyStats::default(),
            outbound: SharedOutbound::direct(),
        }
    }
}

impl Default for ProxyService {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyService {
    /// Create with a specific outbound connector.
    pub fn with_outbound(outbound: SharedOutbound) -> Self {
        let (state_tx, state_rx) = watch::channel(ProxyState::Disconnected);
        Self {
            state_tx,
            state_rx,
            shutdown_tx: None,
            socks5_handle: None,
            http_handle: None,
            stats: ProxyStats::default(),
            outbound,
        }
    }

    /// Set the outbound connector (e.g., switch to a proxy protocol).
    pub fn set_outbound(&mut self, outbound: SharedOutbound) {
        self.outbound = outbound;
    }

    /// Start the proxy service (SOCKS5 + HTTP servers).
    pub async fn start(
        &mut self,
        listen_addr: &str,
        socks_port: u16,
        http_port: u16,
    ) -> Result<()> {
        if self.shutdown_tx.is_some() {
            anyhow::bail!("Proxy service is already running");
        }

        self.set_state(ProxyState::Connecting);
        self.stats.reset();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Start SOCKS5 server
        let socks5 = Socks5Server::bind(
            listen_addr,
            socks_port,
            self.outbound.clone(),
            self.stats.clone(),
        )
        .await?;

        // Start HTTP proxy server
        let http = HttpProxyServer::bind(
            listen_addr,
            http_port,
            self.outbound.clone(),
            self.stats.clone(),
        )
        .await?;

        let socks5_rx = shutdown_rx.clone();
        let http_rx = shutdown_rx;

        self.socks5_handle = Some(tokio::spawn(async move {
            if let Err(e) = socks5.run(socks5_rx).await {
                tracing::error!("SOCKS5 server error: {}", e);
            }
        }));

        self.http_handle = Some(tokio::spawn(async move {
            if let Err(e) = http.run(http_rx).await {
                tracing::error!("HTTP proxy server error: {}", e);
            }
        }));

        self.shutdown_tx = Some(shutdown_tx);
        self.set_state(ProxyState::Connected);

        tracing::info!(
            "Proxy service started: SOCKS5={}:{}, HTTP={}:{}",
            listen_addr,
            socks_port,
            listen_addr,
            http_port
        );

        Ok(())
    }

    /// Stop the proxy service.
    ///
    /// Each server task is aborted and awaited with a 500 ms deadline so that
    /// a stuck task (e.g. a lingering QUIC session draining) never blocks the
    /// UI for more than ~1 s total.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        let deadline = std::time::Duration::from_millis(500);
        if let Some(h) = self.socks5_handle.take() {
            h.abort();
            let _ = tokio::time::timeout(deadline, h).await;
        }
        if let Some(h) = self.http_handle.take() {
            h.abort();
            let _ = tokio::time::timeout(deadline, h).await;
        }
        self.set_state(ProxyState::Disconnected);
        tracing::info!("Proxy service stopped");
    }

    /// Check if the service is running.
    pub fn is_running(&self) -> bool {
        matches!(*self.state_rx.borrow(), ProxyState::Connected)
    }

    /// Get the current proxy state.
    pub fn state(&self) -> ProxyState {
        self.state_rx.borrow().clone()
    }

    /// Subscribe to state changes.
    pub fn subscribe_state(&self) -> watch::Receiver<ProxyState> {
        self.state_rx.clone()
    }

    /// Get proxy connection statistics.
    pub fn stats(&self) -> &ProxyStats {
        &self.stats
    }

    fn set_state(&self, state: ProxyState) {
        let _ = self.state_tx.send(state);
    }
}

impl Drop for ProxyService {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(h) = self.socks5_handle.take() {
            h.abort();
        }
        if let Some(h) = self.http_handle.take() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_proxy_service_lifecycle() {
        let mut service = ProxyService::new();
        assert_eq!(service.state(), ProxyState::Disconnected);
        assert!(!service.is_running());

        // Start on random ports
        service.start("127.0.0.1", 0, 0).await.unwrap();
        assert!(service.is_running());
        assert_eq!(service.state(), ProxyState::Connected);

        // Stop
        service.stop().await;
        assert!(!service.is_running());
        assert_eq!(service.state(), ProxyState::Disconnected);
    }

    #[tokio::test]
    async fn test_proxy_service_full_flow() {
        // Echo server
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo_listener.local_addr().unwrap().port();
        let _echo = tokio::spawn(async move {
            while let Ok((mut s, _)) = echo_listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if s.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });

        let mut service = ProxyService::new();

        // Use port 0 for auto-assignment - but we need to know the actual ports.
        // For testing, use specific ports in the ephemeral range.
        let socks_port = 19876;
        let http_port = 19877;
        service
            .start("127.0.0.1", socks_port, http_port)
            .await
            .unwrap();

        // Test via SOCKS5
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();
        // SOCKS5 handshake
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);
        // CONNECT
        let mut req = vec![0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1];
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();
        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[1], 0x00);
        // Data
        client.write_all(b"service test").await.unwrap();
        let mut buf = [0u8; 12];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"service test");
        drop(client);

        // Test via HTTP CONNECT
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", http_port))
            .await
            .unwrap();
        let connect = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            echo_port
        );
        client.write_all(connect.as_bytes()).await.unwrap();
        let mut resp_buf = vec![0u8; 256];
        let n = client.read(&mut resp_buf).await.unwrap();
        let resp_str = std::str::from_utf8(&resp_buf[..n]).unwrap();
        assert!(resp_str.contains("200"));
        client.write_all(b"http test").await.unwrap();
        let mut buf = [0u8; 9];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"http test");

        service.stop().await;
    }
}
