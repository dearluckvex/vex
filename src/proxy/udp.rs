use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct UdpForwarder {
    target: SocketAddr,
}

impl UdpForwarder {
    pub fn new(addr: SocketAddr) -> Self {
        Self { target: addr }
    }
    
    pub async fn forward(
        &self,
        socket: &UdpSocket,
        data: &[u8],
        src_addr: SocketAddr,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut buf = [0u8; 4096];
        
        // Send to target
        socket.send_to(data, self.target).await?;
        
        // Receive response with timeout
        let (n, _) = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.recv_from(&mut buf)
        ).await {
            Ok(result) => result?,
            Err(_) => {
                log::warn!("[UDP] Response timeout from {} for {}", self.target, src_addr);
                return Err("Timeout".into());
            }
        };
        
        log::debug!(
            "[UDP] Response from {} to {}: {} bytes",
            self.target, src_addr, n
        );
        
        Ok(n)
    }
}
