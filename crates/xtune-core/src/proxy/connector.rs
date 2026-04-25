use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// Trait combining AsyncRead + AsyncWrite for boxed proxy streams.
pub trait ProxyStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ProxyStream for T {}

/// Boxed proxy stream for dynamic dispatch.
pub type BoxProxyStream = Box<dyn ProxyStream>;

/// Outbound connector trait. Implementations provide different ways to
/// connect to a remote target (direct, Shadowsocks, VMess, etc.).
pub trait Outbound: Send + Sync + 'static {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>>;

    fn name(&self) -> &str;
}

/// Default timeout for direct TCP connections.
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Direct TCP connection (no proxy, used for testing and direct mode).
pub struct DirectOutbound;

impl Outbound for DirectOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let addr = format!("{}:{}", host, port);
        Box::pin(async move {
            let stream = tokio::time::timeout(DIRECT_CONNECT_TIMEOUT, TcpStream::connect(&addr))
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Direct connection to {} timed out after {}s",
                        addr,
                        DIRECT_CONNECT_TIMEOUT.as_secs()
                    )
                })?
                .with_context(|| format!("Direct connection to {} failed", addr))?;
            stream.set_nodelay(true).ok();
            let sock = SockRef::from(&stream);
            sock.set_send_buffer_size(256 * 1024).ok();
            sock.set_recv_buffer_size(256 * 1024).ok();
            let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(30));
            sock.set_tcp_keepalive(&keepalive).ok();
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "direct"
    }
}

/// Outbound wrapper that retries failed connections with exponential backoff.
///
/// Handles transient failures (brief network blips, momentary server overload)
/// that self-resolve on retry. Default: up to 3 attempts, 200 ms → 2 s delay.
pub struct RetryOutbound {
    inner: Arc<dyn Outbound>,
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

impl RetryOutbound {
    pub fn new(inner: Arc<dyn Outbound>) -> Self {
        Self {
            inner,
            max_attempts: 3,
            base_delay_ms: 200,
            max_delay_ms: 2000,
        }
    }
}

impl Outbound for RetryOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            let mut last_err = None;
            let mut delay_ms = self.base_delay_ms;
            for attempt in 0..self.max_attempts {
                match self.inner.connect(&host, port).await {
                    Ok(stream) => {
                        if attempt > 0 {
                            tracing::debug!(
                                "{}:{} connected on attempt {}/{}",
                                host,
                                port,
                                attempt + 1,
                                self.max_attempts
                            );
                        }
                        return Ok(stream);
                    }
                    Err(e) => {
                        tracing::debug!(
                            "{}:{} attempt {}/{} failed: {:#}",
                            host,
                            port,
                            attempt + 1,
                            self.max_attempts,
                            e
                        );
                        last_err = Some(e);
                        if attempt + 1 < self.max_attempts {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            delay_ms = (delay_ms * 2).min(self.max_delay_ms);
                        }
                    }
                }
            }
            Err(last_err.unwrap())
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// Wraps an `Arc<dyn Outbound>` for convenience cloning.
#[derive(Clone)]
pub struct SharedOutbound(pub Arc<dyn Outbound>);

impl SharedOutbound {
    pub fn direct() -> Self {
        Self(Arc::new(DirectOutbound))
    }

    /// Wrap this outbound with automatic retry on transient failures.
    pub fn with_retry(self, max_attempts: u32) -> Self {
        Self(Arc::new(RetryOutbound {
            inner: self.0,
            max_attempts,
            base_delay_ms: 200,
            max_delay_ms: 2000,
        }))
    }

    pub async fn connect(&self, host: &str, port: u16) -> Result<BoxProxyStream> {
        self.0.connect(host, port).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_direct_outbound() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
        });

        let outbound = DirectOutbound;
        let mut stream = outbound
            .connect(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();

        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server.await.unwrap();
    }
}
