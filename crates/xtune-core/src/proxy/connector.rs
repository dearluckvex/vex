use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
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
            let stream = TcpStream::connect(&addr).await?;
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "direct"
    }
}

/// Wraps an `Arc<dyn Outbound>` for convenience cloning.
#[derive(Clone)]
pub struct SharedOutbound(pub Arc<dyn Outbound>);

impl SharedOutbound {
    pub fn direct() -> Self {
        Self(Arc::new(DirectOutbound))
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
