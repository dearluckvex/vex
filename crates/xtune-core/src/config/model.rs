use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unified application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Local proxy listen address
    pub listen_addr: String,
    /// Local SOCKS5 proxy port
    pub socks_port: u16,
    /// Local HTTP proxy port
    pub http_port: u16,
    /// Proxy nodes
    pub nodes: Vec<Node>,
    /// Active node index
    pub active_node: Option<usize>,
    /// Subscriptions
    pub subscriptions: Vec<Subscription>,
    /// Routing rules
    pub rules: Vec<RoutingRule>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1".to_string(),
            socks_port: 1080,
            http_port: 1087,
            nodes: Vec::new(),
            active_node: None,
            subscriptions: Vec::new(),
            rules: Vec::new(),
        }
    }
}

/// A proxy node (server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Display name
    pub name: String,
    /// Server address (hostname or IP)
    pub server: String,
    /// Server port
    pub port: u16,
    /// Proxy protocol
    pub protocol: ProxyProtocol,
    /// Transport layer config
    pub transport: Option<TransportConfig>,
    /// Measured latency in ms (None if not tested)
    #[serde(skip)]
    pub latency_ms: Option<u32>,
    /// Extra metadata
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

/// Supported proxy protocols
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProxyProtocol {
    /// Shadowsocks
    Shadowsocks {
        cipher: String,
        password: String,
        #[serde(default)]
        udp: bool,
    },
    /// VMess (V2Ray)
    VMess {
        uuid: String,
        alter_id: u32,
        cipher: String,
        #[serde(default)]
        udp: bool,
    },
    /// VLESS
    VLess {
        uuid: String,
        #[serde(default)]
        flow: Option<String>,
        #[serde(default)]
        udp: bool,
    },
    /// TUIC v5
    Tuic {
        uuid: String,
        password: String,
        #[serde(default = "default_congestion")]
        congestion_control: String,
        #[serde(default)]
        udp: bool,
    },
    /// Trojan
    Trojan {
        password: String,
        #[serde(default)]
        udp: bool,
    },
    /// Hysteria2
    Hysteria2 {
        password: String,
        #[serde(default)]
        udp: bool,
    },
}

fn default_congestion() -> String {
    "bbr".to_string()
}

/// Transport layer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Transport type
    #[serde(rename = "type")]
    pub transport_type: TransportType,
    /// TLS settings
    pub tls: Option<TlsConfig>,
    /// WebSocket settings
    pub ws: Option<WsConfig>,
    /// Reality settings
    pub reality: Option<RealityConfig>,
}

/// Transport types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Tcp,
    Tls,
    WebSocket,
    Quic,
    Reality,
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub sni: Option<String>,
    #[serde(default)]
    pub skip_cert_verify: bool,
    pub alpn: Option<Vec<String>>,
    pub fingerprint: Option<String>,
}

/// WebSocket configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsConfig {
    pub path: Option<String>,
    pub host: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// XTLS Reality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealityConfig {
    pub public_key: String,
    pub short_id: String,
    pub sni: Option<String>,
}

/// Subscription source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub url: String,
    /// Format hint: "clash", "v2ray", "karing", "auto"
    #[serde(default = "default_format")]
    pub format: String,
    /// Last update timestamp
    pub last_updated: Option<u64>,
}

fn default_format() -> String {
    "auto".to_string()
}

/// Routing rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Rule type: "domain", "domain-suffix", "ip-cidr", "geoip"
    pub rule_type: String,
    /// Match pattern
    pub pattern: String,
    /// Target: "direct", "proxy", "reject"
    pub target: String,
}
