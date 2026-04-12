use std::net::SocketAddr;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use crate::packet::IpPacket;

pub struct ProxyStats {
    pub packets_received: u64,
    pub packets_forwarded: u64,
    pub bytes_received: u64,
    pub bytes_forwarded: u64,
    pub active_connections: usize,
}

pub struct PacketProxy {
    stats: Arc<Mutex<ProxyStats>>,
    active_connections: Arc<Mutex<HashMap<String, u64>>>,
}

impl PacketProxy {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(Mutex::new(ProxyStats {
                packets_received: 0,
                packets_forwarded: 0,
                bytes_received: 0,
                bytes_forwarded: 0,
                active_connections: 0,
            })),
            active_connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    pub async fn process_packet(&self, packet: IpPacket) {
        let mut stats = self.stats.lock().await;
        stats.packets_received += 1;
        stats.bytes_received += packet.raw.len() as u64;
        drop(stats);
        
        match packet.protocol {
            crate::packet::Protocol::Tcp => {
                self.handle_tcp(packet).await;
            }
            crate::packet::Protocol::Udp => {
                self.handle_udp(packet).await;
            }
            crate::packet::Protocol::Other(_) => {
                log::debug!("Unsupported protocol from {:?}", packet.src_ip);
            }
        }
    }
    
    async fn handle_tcp(&self, packet: IpPacket) {
        let _conn_key = packet.connection_key();
        let mut conns = self.active_connections.lock().await;
        
        if !conns.contains_key(&_conn_key) {
            conns.insert(_conn_key.clone(), 1);
            log::info!("[TCP] New connection: {}", &_conn_key);
        }
        drop(conns);
        
        self.update_forwarded_stats(packet.raw.len()).await;
    }
    
    async fn handle_udp(&self, packet: IpPacket) {
        let conn_key = packet.connection_key();
        
        match packet.dst_port {
            Some(53) => {
                log::debug!("[DNS] Query from {} to port 53", packet.src_ip);
            }
            Some(port) => {
                log::debug!("[UDP] {} -> port {}", packet.src_ip, port);
            }
            None => {}
        }
        
        self.update_forwarded_stats(packet.raw.len()).await;
    }
    
    async fn update_forwarded_stats(&self, bytes: usize) {
        let mut stats = self.stats.lock().await;
        stats.packets_forwarded += 1;
        stats.bytes_forwarded += bytes as u64;
    }
    
    pub async fn get_stats(&self) -> ProxyStats {
        let stats = self.stats.lock().await;
        ProxyStats {
            packets_received: stats.packets_received,
            packets_forwarded: stats.packets_forwarded,
            bytes_received: stats.bytes_received,
            bytes_forwarded: stats.bytes_forwarded,
            active_connections: stats.active_connections,
        }
    }
}
