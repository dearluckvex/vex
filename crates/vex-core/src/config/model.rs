use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Proxy routing mode
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// All traffic goes through the proxy node
    Global,
    /// China-direct / overseas-proxy based on built-in rules
    Rule,
    /// All traffic connects directly (no proxy)
    Direct,
}

impl Default for ProxyMode {
    fn default() -> Self {
        Self::Global
    }
}

/// Decode a percent-encoded display name.
///
/// Iteratively URL-decodes up to 3 times to handle multi-level encoding
/// commonly seen in subscription share links. Also replaces `+` with space
/// and trims surrounding whitespace.
pub fn decode_display_name(value: &str) -> String {
    let mut decoded = value.replace('+', " ");
    for _ in 0..3 {
        match urlencoding::decode(&decoded) {
            Ok(next) if next.as_ref() != decoded => decoded = next.into_owned(),
            _ => break,
        }
    }
    decoded.trim().to_string()
}

/// Decode and normalize the names of all nodes in-place.
/// Returns `true` if any name was changed.
pub fn normalize_node_names(nodes: &mut [Node]) -> bool {
    let mut changed = false;
    for node in nodes {
        let normalized = decode_display_name(&node.name);
        if normalized != node.name {
            node.name = normalized;
            changed = true;
        }
    }
    changed
}

/// Unified application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Local proxy listen address
    pub listen_addr: String,
    /// Local SOCKS5 proxy port
    pub socks_port: u16,
    /// Local HTTP proxy port
    pub http_port: u16,
    /// Proxy routing mode
    #[serde(default)]
    pub proxy_mode: ProxyMode,
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
            proxy_mode: ProxyMode::default(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    /// User-defined tags for grouping/filtering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
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
    /// Last update timestamp (Unix seconds)
    pub last_updated: Option<u64>,
    /// How often to auto-refresh (hours). 0 = manual only. Default: 24.
    #[serde(default = "default_refresh_hours")]
    pub refresh_interval_hours: u64,
}

fn default_refresh_hours() -> u64 {
    24
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
    /// Whether this rule is active (defaults to true)
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_single_encoded() {
        assert_eq!(
            decode_display_name("%E9%A9%AC%E6%9D%A5%E8%A5%BF%E4%BA%9A"),
            "马来西亚"
        );
    }

    #[test]
    fn decode_double_encoded() {
        assert_eq!(
            decode_display_name("%25E9%25A9%25AC%25E6%259D%25A5%25E8%25A5%25BF%25E4%25BA%259A"),
            "马来西亚"
        );
    }

    #[test]
    fn decode_plus_as_space() {
        assert_eq!(decode_display_name("Hong+Kong+01"), "Hong Kong 01");
    }

    #[test]
    fn decode_mixed_encoding() {
        assert_eq!(
            decode_display_name("%F0%9F%87%B2%F0%9F%87%BE+Malaysia"),
            "🇲🇾 Malaysia"
        );
    }

    #[test]
    fn decode_trims_whitespace() {
        assert_eq!(decode_display_name("  test  "), "test");
    }

    #[test]
    fn decode_plain_text_unchanged() {
        assert_eq!(decode_display_name("Tokyo 01"), "Tokyo 01");
    }

    #[test]
    fn normalize_nodes_decodes_names() {
        let mut nodes = vec![Node {
            name: "%E9%A6%99%E6%B8%AF".to_string(),
            server: "1.2.3.4".to_string(),
            port: 443,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "test".to_string(),
                udp: false,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }];
        assert!(normalize_node_names(&mut nodes));
        assert_eq!(nodes[0].name, "香港");
    }
}
