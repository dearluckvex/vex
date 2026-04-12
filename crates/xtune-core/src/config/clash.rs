use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

use super::model::*;

/// Clash YAML configuration top-level structure
#[derive(Debug, Deserialize)]
struct ClashConfig {
    #[serde(default)]
    proxies: Vec<ClashProxy>,
    #[serde(default, rename = "proxy-groups")]
    proxy_groups: Vec<ClashProxyGroup>,
}

#[derive(Debug, Deserialize)]
struct ClashProxy {
    name: String,
    #[serde(rename = "type")]
    proxy_type: String,
    server: String,
    port: u16,
    // Shadowsocks fields
    cipher: Option<String>,
    password: Option<String>,
    // VMess fields
    uuid: Option<String>,
    #[serde(rename = "alterId", default)]
    alter_id: Option<u32>,
    // VLESS fields
    flow: Option<String>,
    // TLS
    #[serde(default)]
    tls: bool,
    servername: Option<String>,
    #[serde(rename = "skip-cert-verify", default)]
    skip_cert_verify: bool,
    alpn: Option<Vec<String>>,
    #[serde(rename = "client-fingerprint")]
    client_fingerprint: Option<String>,
    // Network/Transport
    network: Option<String>,
    #[serde(rename = "ws-opts")]
    ws_opts: Option<ClashWsOpts>,
    // TUIC
    #[serde(rename = "congestion-controller")]
    congestion_controller: Option<String>,
    // Reality
    #[serde(rename = "reality-opts")]
    reality_opts: Option<ClashRealityOpts>,
    // UDP
    #[serde(default)]
    udp: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ClashWsOpts {
    path: Option<String>,
    headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ClashRealityOpts {
    #[serde(rename = "public-key")]
    public_key: Option<String>,
    #[serde(rename = "short-id")]
    short_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClashProxyGroup {
    name: String,
    #[serde(rename = "type")]
    group_type: String,
    #[serde(default)]
    proxies: Vec<String>,
}

/// Parse a Clash YAML configuration string into a list of nodes
pub fn parse_clash_config(yaml_str: &str) -> Result<Vec<Node>> {
    let config: ClashConfig =
        serde_yaml::from_str(yaml_str).context("Failed to parse Clash YAML")?;

    let mut nodes = Vec::new();
    for proxy in config.proxies {
        match convert_clash_proxy(&proxy) {
            Ok(node) => nodes.push(node),
            Err(e) => {
                tracing::warn!("Skipping proxy '{}': {}", proxy.name, e);
            }
        }
    }

    Ok(nodes)
}

/// Check if a YAML string looks like Clash config
pub fn is_clash_config(content: &str) -> bool {
    content.contains("proxies:") || content.contains("proxy-groups:")
}

fn convert_clash_proxy(proxy: &ClashProxy) -> Result<Node> {
    let udp = proxy.udp.unwrap_or(false);

    let protocol = match proxy.proxy_type.as_str() {
        "ss" => {
            let cipher = proxy
                .cipher
                .as_ref()
                .context("SS missing cipher")?
                .clone();
            let password = proxy
                .password
                .as_ref()
                .context("SS missing password")?
                .clone();
            ProxyProtocol::Shadowsocks {
                cipher,
                password,
                udp,
            }
        }
        "vmess" => {
            let uuid = proxy.uuid.as_ref().context("VMess missing uuid")?.clone();
            let alter_id = proxy.alter_id.unwrap_or(0);
            let cipher = proxy
                .cipher
                .clone()
                .unwrap_or_else(|| "auto".to_string());
            ProxyProtocol::VMess {
                uuid,
                alter_id,
                cipher,
                udp,
            }
        }
        "vless" => {
            let uuid = proxy.uuid.as_ref().context("VLESS missing uuid")?.clone();
            ProxyProtocol::VLess {
                uuid,
                flow: proxy.flow.clone(),
                udp,
            }
        }
        "tuic" => {
            let uuid = proxy.uuid.as_ref().context("TUIC missing uuid")?.clone();
            let password = proxy
                .password
                .as_ref()
                .context("TUIC missing password")?
                .clone();
            let congestion_control = proxy
                .congestion_controller
                .clone()
                .unwrap_or_else(|| "bbr".to_string());
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                udp,
            }
        }
        "trojan" => {
            let password = proxy
                .password
                .as_ref()
                .context("Trojan missing password")?
                .clone();
            ProxyProtocol::Trojan { password, udp }
        }
        "hysteria2" | "hy2" => {
            let password = proxy
                .password
                .as_ref()
                .context("Hysteria2 missing password")?
                .clone();
            ProxyProtocol::Hysteria2 { password, udp }
        }
        other => anyhow::bail!("Unsupported proxy type: {}", other),
    };

    let transport = build_transport(proxy);

    Ok(Node {
        name: proxy.name.clone(),
        server: proxy.server.clone(),
        port: proxy.port,
        protocol,
        transport,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

fn build_transport(proxy: &ClashProxy) -> Option<TransportConfig> {
    let network = proxy.network.as_deref().unwrap_or("tcp");

    // Reality transport
    if let Some(ref reality) = proxy.reality_opts {
        if let (Some(pk), Some(sid)) = (&reality.public_key, &reality.short_id) {
            return Some(TransportConfig {
                transport_type: TransportType::Reality,
                tls: None,
                ws: None,
                reality: Some(RealityConfig {
                    public_key: pk.clone(),
                    short_id: sid.clone(),
                    sni: proxy.servername.clone(),
                }),
            });
        }
    }

    let has_tls = proxy.tls;
    let has_ws = network == "ws";

    if !has_tls && !has_ws {
        return None;
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: proxy.servername.clone(),
            skip_cert_verify: proxy.skip_cert_verify,
            alpn: proxy.alpn.clone(),
            fingerprint: proxy.client_fingerprint.clone(),
        })
    } else {
        None
    };

    let ws = if has_ws {
        proxy.ws_opts.as_ref().map(|opts| {
            let host = opts
                .headers
                .as_ref()
                .and_then(|h| h.get("Host").cloned());
            WsConfig {
                path: opts.path.clone(),
                host,
                headers: opts.headers.clone(),
            }
        })
    } else {
        None
    };

    let transport_type = match (has_ws, has_tls) {
        (true, _) => TransportType::WebSocket,
        (false, true) => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Some(TransportConfig {
        transport_type,
        tls,
        ws,
        reality: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clash_ss() {
        let yaml = r#"
proxies:
  - name: "SS-HK"
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: "test123"
    udp: true
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "SS-HK");
        assert_eq!(nodes[0].server, "1.2.3.4");
        assert_eq!(nodes[0].port, 8388);
        match &nodes[0].protocol {
            ProxyProtocol::Shadowsocks {
                cipher, password, ..
            } => {
                assert_eq!(cipher, "aes-256-gcm");
                assert_eq!(password, "test123");
            }
            _ => panic!("Expected Shadowsocks"),
        }
    }

    #[test]
    fn test_parse_clash_vmess_ws_tls() {
        let yaml = r#"
proxies:
  - name: "VMess-JP"
    type: vmess
    server: 5.6.7.8
    port: 443
    uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4"
    alterId: 0
    cipher: auto
    tls: true
    servername: example.com
    network: ws
    ws-opts:
      path: /v2ray
      headers:
        Host: example.com
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(node.name, "VMess-JP");
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::WebSocket);
        assert!(transport.tls.is_some());
        assert!(transport.ws.is_some());
        assert_eq!(transport.ws.as_ref().unwrap().path.as_deref(), Some("/v2ray"));
    }

    #[test]
    fn test_parse_clash_vless_reality() {
        let yaml = r#"
proxies:
  - name: "VLESS-US"
    type: vless
    server: 9.10.11.12
    port: 443
    uuid: "b85798ef-e9dc-46a4-9a87-8da4499d36d0"
    flow: xtls-rprx-vision
    tls: true
    reality-opts:
      public-key: "abc123"
      short-id: "0123456789abcdef"
    servername: www.example.com
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::Reality);
        let reality = transport.reality.as_ref().unwrap();
        assert_eq!(reality.public_key, "abc123");
    }

    #[test]
    fn test_parse_clash_tuic() {
        let yaml = r#"
proxies:
  - name: "TUIC-SG"
    type: tuic
    server: 13.14.15.16
    port: 443
    uuid: "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa"
    password: "supersecret"
    congestion-controller: bbr
    udp: true
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        match &nodes[0].protocol {
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                ..
            } => {
                assert_eq!(uuid, "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa");
                assert_eq!(password, "supersecret");
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
    }

    #[test]
    fn test_parse_mixed_proxies() {
        let yaml = r#"
proxies:
  - name: "SS"
    type: ss
    server: 1.1.1.1
    port: 8388
    cipher: aes-256-gcm
    password: "pass1"
  - name: "VMess"
    type: vmess
    server: 2.2.2.2
    port: 443
    uuid: "uuid1"
  - name: "Trojan"
    type: trojan
    server: 3.3.3.3
    port: 443
    password: "pass2"
  - name: "Unknown"
    type: snell
    server: 4.4.4.4
    port: 1234
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        // snell is unsupported, should be skipped
        assert_eq!(nodes.len(), 3);
    }
}
