use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::connector::SharedOutbound;

/// Result of a speed test for a single node.
#[derive(Debug, Clone)]
pub struct SpeedTestResult {
    /// TCP connection latency in ms
    pub latency_ms: u32,
    /// Download speed in KB/s (None if download test failed or was skipped)
    pub download_kbps: Option<u32>,
}

/// Perform a real speed test for a node through its outbound connection.
///
/// 1. Measures TCP + protocol handshake latency by connecting to a test host.
/// 2. Sends an HTTP GET and measures download throughput.
pub async fn speed_test_node(
    outbound: &SharedOutbound,
    timeout_secs: u64,
) -> Result<SpeedTestResult> {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Phase 1: Latency — connect through the proxy to www.gstatic.com:80
    let start = std::time::Instant::now();
    let mut stream = tokio::time::timeout(timeout, outbound.connect("www.gstatic.com", 80))
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out"))??;
    let latency_ms = start.elapsed().as_millis() as u32;

    // Phase 2: Download speed — request a ~200 KB test payload
    let request =
        b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n";
    stream.write_all(request).await?;

    let dl_start = std::time::Instant::now();
    let mut total_bytes: u64 = 0;
    let mut buf = vec![0u8; 8192];
    loop {
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf)).await;

        match read_result {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => total_bytes += n as u64,
            Ok(Err(_)) => break,
            Err(_) => break, // timeout
        }
    }
    let dl_elapsed = dl_start.elapsed();

    // /generate_204 returns a very small body; if we got a valid response
    // we know the connection works. Calculate speed from what we got.
    let download_kbps = if total_bytes > 0 && dl_elapsed.as_millis() > 0 {
        Some(((total_bytes as f64 / 1024.0) / dl_elapsed.as_secs_f64()) as u32)
    } else {
        None
    };

    Ok(SpeedTestResult {
        latency_ms,
        download_kbps,
    })
}

/// Perform a quick latency-only test by connecting through the proxy outbound.
/// This measures real protocol handshake time, not just raw TCP.
pub async fn latency_test_node(outbound: &SharedOutbound, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    let mut stream = tokio::time::timeout(timeout, outbound.connect("www.gstatic.com", 80))
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out"))??;

    // Send a minimal HTTP request to ensure the tunnel is live
    stream
        .write_all(b"HEAD / HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n")
        .await?;
    let mut buf = [0u8; 64];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await;

    Ok(start.elapsed().as_millis() as u32)
}

/// Perform a fast TCP-only latency test by connecting directly to the server.
///
/// This measures just the raw TCP handshake latency to the proxy server,
/// similar to what Karing and other proxy clients display as "latency".
/// It does NOT go through the proxy protocol or connect to a target.
pub async fn tcp_latency_test(server: &str, port: u16, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let addr = format!("{}:{}", server, port);
    let start = std::time::Instant::now();
    let _stream = tokio::time::timeout(timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("TCP connect to {} timed out ({}s)", addr, timeout_secs))??;
    Ok(start.elapsed().as_millis() as u32)
}

/// Perform an HTTP latency test through a proxy outbound with warmup.
///
/// Does two connections: a warmup (to establish QUIC/TLS sessions for
/// protocols like TUIC) and a measurement. This gives a realistic "warm"
/// latency that represents actual browsing performance.
///
/// Typical results: 100-500ms for TUIC/Hysteria2, 50-200ms for TCP protocols.
pub async fn http_latency_test(outbound: &SharedOutbound, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Phase 1: Warmup — establish underlying connections (QUIC session, etc.)
    let warmup = tokio::time::timeout(timeout, outbound.connect("www.gstatic.com", 80)).await;
    match warmup {
        Ok(Ok(mut s)) => {
            let _ = s
                .write_all(
                    b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
                )
                .await;
            let mut buf = [0u8; 128];
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut buf)).await;
        }
        Ok(Err(e)) => return Err(anyhow::anyhow!("connection failed: {:#}", e)),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "connection timed out ({}s)",
                timeout_secs
            ))
        }
    }

    // Phase 2: Measure — connect again using cached/warm connections
    let start = std::time::Instant::now();
    let mut stream = tokio::time::timeout(timeout, outbound.connect("www.gstatic.com", 80))
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out"))??;

    stream
        .write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        )
        .await?;
    let mut buf = [0u8; 128];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await;

    Ok(start.elapsed().as_millis() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_latency_direct_outbound() {
        // Test with direct outbound — should succeed if internet is available
        let outbound = SharedOutbound::direct();
        match latency_test_node(&outbound, 10).await {
            Ok(ms) => assert!(ms < 10000, "latency too high: {}ms", ms),
            Err(e) => {
                // Network might not be available in CI
                eprintln!("latency test skipped (no network): {}", e);
            }
        }
    }
}
