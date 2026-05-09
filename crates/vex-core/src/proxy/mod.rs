pub mod connector;
pub mod factory;
pub mod http;
pub mod hysteria2;
pub mod pool;
pub mod quic_conn;
pub mod relay;
pub mod routing;
pub mod service;
pub mod socks5;
pub mod speedtest;
pub mod ss;
pub mod transport;
pub mod trojan;
pub mod tuic;
pub mod tun;
pub mod vless;
pub mod vmess;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Proxy connection state.
#[derive(Debug, Clone, PartialEq)]
pub enum ProxyState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Connection and traffic statistics, shared across server tasks.
#[derive(Debug, Clone)]
pub struct ProxyStats {
    inner: Arc<StatsInner>,
}

#[derive(Debug)]
struct StatsInner {
    active_connections: AtomicU64,
    total_connections: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

impl Default for ProxyStats {
    fn default() -> Self {
        Self {
            inner: Arc::new(StatsInner {
                active_connections: AtomicU64::new(0),
                total_connections: AtomicU64::new(0),
                bytes_sent: AtomicU64::new(0),
                bytes_received: AtomicU64::new(0),
            }),
        }
    }
}

impl ProxyStats {
    pub fn active_connections(&self) -> u64 {
        self.inner.active_connections.load(Ordering::Relaxed)
    }

    pub fn total_connections(&self) -> u64 {
        self.inner.total_connections.load(Ordering::Relaxed)
    }

    pub fn bytes_sent(&self) -> u64 {
        self.inner.bytes_sent.load(Ordering::Relaxed)
    }

    pub fn bytes_received(&self) -> u64 {
        self.inner.bytes_received.load(Ordering::Relaxed)
    }

    pub(crate) fn add_connection(&self) {
        self.inner
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
        self.inner.total_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn remove_connection(&self) {
        self.inner
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub(crate) fn add_bytes(&self, sent: u64, received: u64) {
        self.inner.bytes_sent.fetch_add(sent, Ordering::Relaxed);
        self.inner
            .bytes_received
            .fetch_add(received, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.inner.active_connections.store(0, Ordering::Relaxed);
        self.inner.total_connections.store(0, Ordering::Relaxed);
        self.inner.bytes_sent.store(0, Ordering::Relaxed);
        self.inner.bytes_received.store(0, Ordering::Relaxed);
    }
}
