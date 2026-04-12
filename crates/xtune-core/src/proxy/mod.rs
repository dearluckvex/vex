pub mod socks5;
pub mod http;
pub mod connector;

/// Proxy connection state
#[derive(Debug, Clone, PartialEq)]
pub enum ProxyState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}
