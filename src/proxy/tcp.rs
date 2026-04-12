use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::net::SocketAddr;

pub struct TcpForwarder {
    target: SocketAddr,
}

impl TcpForwarder {
    pub fn new(addr: SocketAddr) -> Self {
        Self { target: addr }
    }
    
    pub async fn forward(
        &self,
        client: TcpStream,
        src_addr: SocketAddr,
    ) -> Result<(u64, u64), Box<dyn std::error::Error>> {
        let server = TcpStream::connect(self.target).await?;
        
        let (mut client_read, mut client_write) = client.into_split();
        let (mut server_read, mut server_write) = server.into_split();
        
        let client_to_server = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            let mut bytes_transferred = 0u64;
            
            loop {
                match client_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        bytes_transferred += n as u64;
                        if server_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            bytes_transferred
        });
        
        let server_to_client = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            let mut bytes_transferred = 0u64;
            
            loop {
                match server_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        bytes_transferred += n as u64;
                        if client_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            bytes_transferred
        });
        
        let (client_bytes, server_bytes) = tokio::join!(client_to_server, server_to_client);
        
        let client_transferred = client_bytes.unwrap_or(0);
        let server_transferred = server_bytes.unwrap_or(0);
        
        log::info!(
            "[TCP] Connection closed: {} | C->S: {} B, S->C: {} B",
            src_addr, client_transferred, server_transferred
        );
        
        Ok((client_transferred, server_transferred))
    }
}
