//! Sing-box configuration parser.
//!
//! Parses sing-box JSON outbound format into vex Node structs.
//! Supports: TUIC, Hysteria2, VMess, VLESS, Trojan, Shadowsocks.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::model::*;

// --- Sing-box config structures ---

#[derive(Debug, Deserialize)]
struct SingBoxConfig {
    #[serde(default)]
    outbounds: Vec<SingBoxOutbound>,
}

#[derive(Debug, Deserialize)]
struct SingBoxOutbound {
    #[serde(rename = "type")]
    outbound_type: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    server_port: u16,

    // Auth fields
    uuid: Option<String>,
    password: Option<String>,
    method: Option<String>, // SS cipher
    flow: Option<String>,   // VLESS

    // TUIC
    congestion_control: Option<String>,
    udp_relay_mode: Option<String>,
    #[serde(default)]
    zero_rtt_handshake: bool,
    heartbeat: Option<String>,

    // TLS
    tls: Option<SingBoxTls>,

    // Transport
    transport: Option<SingBoxTransport>,

    // UDP
    #[serde(default)]
    udp: Option<bool>,

    // Multiplex
    multiplex: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SingBoxTls {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    insecure: bool,
    server_name: Option<String>,
    alpn: Option<Vec<String>>,
    #[serde(default)]
    disable_sni: bool,
    utls: Option<SingBoxUtls>,
}

#[derive(Debug, Deserialize)]
struct SingBoxUtls {
    fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SingBoxTransport {
    #[serde(rename = "type")]
    transport_type: Option<String>,
    path: Option<String>,
    headers: Option<HashMap<String, String>>,
}

/// Check if JSON content looks like a sing-box config.
pub fn is_singbox_config(content: &str) -> bool {
    content.contains("\"outbounds\"") && content.contains("\"server_port\"")
}

/// Parse sing-box JSON config into Node list.
pub fn parse_singbox_config(json_str: &str) -> Result<Vec<Node>> {
    let config: SingBoxConfig =
        serde_json::from_str(json_str).context("Failed to parse sing-box JSON")?;

    let mut nodes = Vec::new();
    for outbound in &config.outbounds {
        match convert_singbox_outbound(outbound) {
            Ok(Some(node)) => nodes.push(node),
            Ok(None) => {} // skip non-proxy outbounds (direct, dns, etc.)
            Err(e) => {
                tracing::warn!("Skipping sing-box outbound '{}': {}", outbound.tag, e);
            }
        }
    }
    Ok(nodes)
}

/// Parse a single sing-box outbound JSON object.
pub fn parse_singbox_outbound(json_str: &str) -> Result<Option<Node>> {
    let outbound: SingBoxOutbound =
        serde_json::from_str(json_str).context("Failed to parse sing-box outbound")?;
    convert_singbox_outbound(&outbound)
}

fn convert_singbox_outbound(ob: &SingBoxOutbound) -> Result<Option<Node>> {
    let udp = ob.udp.unwrap_or(false);

    let protocol = match ob.outbound_type.as_str() {
        "tuic" => {
            let uuid = ob.uuid.as_ref().context("TUIC missing uuid")?.clone();
            let password = ob
                .password
                .as_ref()
                .context("TUIC missing password")?
                .clone();
            let congestion_control = ob
                .congestion_control
                .clone()
                .unwrap_or_else(|| "bbr".to_string());
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                udp,
            }
        }
        "hysteria2" | "hy2" => {
            let password = ob
                .password
                .as_ref()
                .context("Hysteria2 missing password")?
                .clone();
            ProxyProtocol::Hysteria2 { password, udp }
        }
        "vless" => {
            let uuid = ob.uuid.as_ref().context("VLESS missing uuid")?.clone();
            ProxyProtocol::VLess {
                uuid,
                flow: ob.flow.clone(),
                udp,
            }
        }
        "vmess" => {
            let uuid = ob.uuid.as_ref().context("VMess missing uuid")?.clone();
            ProxyProtocol::VMess {
                uuid,
                alter_id: 0,
                cipher: "auto".to_string(),
                udp,
            }
        }
        "trojan" => {
            let password = ob
                .password
                .as_ref()
                .context("Trojan missing password")?
                .clone();
            ProxyProtocol::Trojan { password, udp }
        }
        "shadowsocks" | "ss" => {
            let cipher = ob
                .method
                .as_ref()
                .context("Shadowsocks missing method")?
                .clone();
            let password = ob
                .password
                .as_ref()
                .context("Shadowsocks missing password")?
                .clone();
            ProxyProtocol::Shadowsocks {
                cipher,
                password,
                udp,
            }
        }
        // Non-proxy types: skip
        "direct" | "block" | "dns" | "selector" | "urltest" => return Ok(None),
        other => anyhow::bail!("Unsupported sing-box outbound type: {}", other),
    };

    let transport = build_singbox_transport(ob);

    let name = if ob.tag.is_empty() {
        format!("{}-{}:{}", ob.outbound_type, ob.server, ob.server_port)
    } else {
        decode_display_name(&ob.tag)
    };

    Ok(Some(Node {
        name,
        server: ob.server.clone(),
        port: ob.server_port,
        protocol,
        transport,
        latency_ms: None,
        tags: vec![],
        extra: HashMap::new(),
    }))
}

fn build_singbox_transport(ob: &SingBoxOutbound) -> Option<TransportConfig> {
    let tls_cfg = ob.tls.as_ref();
    let proxy_type = ob.outbound_type.as_str();

    // TUIC and Hysteria2 always need TLS even if not explicitly enabled
    let force_tls = matches!(proxy_type, "tuic" | "hysteria2" | "hy2");
    let has_tls = tls_cfg.map(|t| t.enabled).unwrap_or(false) || force_tls;

    let has_ws = ob
        .transport
        .as_ref()
        .and_then(|t| t.transport_type.as_deref())
        .map(|t| t == "ws")
        .unwrap_or(false);

    if !has_tls && !has_ws {
        return None;
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: tls_cfg.and_then(|t| t.server_name.clone()),
            skip_cert_verify: tls_cfg.map(|t| t.insecure).unwrap_or(false),
            alpn: tls_cfg.and_then(|t| t.alpn.clone()),
            fingerprint: tls_cfg.and_then(|t| t.utls.as_ref().and_then(|u| u.fingerprint.clone())),
        })
    } else {
        None
    };

    let ws = if has_ws {
        ob.transport.as_ref().map(|t| WsConfig {
            path: t.path.clone(),
            host: t.headers.as_ref().and_then(|h| h.get("Host").cloned()),
            headers: t.headers.clone(),
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
    fn test_parse_singbox_tuic() {
        let json = r#"{
            "outbounds": [{
                "type": "tuic",
                "tag": "MY-TUIC",
                "server": "202.155.10.21",
                "server_port": 60000,
                "congestion_control": "bbr",
                "udp_relay_mode": "native",
                "zero_rtt_handshake": true,
                "heartbeat": "10s",
                "tls": {
                    "enabled": true,
                    "insecure": true,
                    "alpn": ["h3"],
                    "server_name": "www.python.org"
                },
                "uuid": "47c58a02-a50b-4478-8e68-632d6ceecad8",
                "password": "47c58a02-a50b-4478-8e68-632d6ceecad8"
            }]
        }"#;

        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "MY-TUIC");
        assert_eq!(nodes[0].server, "202.155.10.21");
        assert_eq!(nodes[0].port, 60000);

        // Check TLS config preserved
        let transport = nodes[0].transport.as_ref().unwrap();
        let tls = transport.tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("www.python.org"));
        assert!(tls.skip_cert_verify);
        assert_eq!(tls.alpn.as_ref().unwrap(), &vec!["h3".to_string()]);

        match &nodes[0].protocol {
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                ..
            } => {
                assert_eq!(uuid, "47c58a02-a50b-4478-8e68-632d6ceecad8");
                assert_eq!(password, "47c58a02-a50b-4478-8e68-632d6ceecad8");
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
    }

    #[test]
    fn test_parse_singbox_hysteria2() {
        let json = r#"{
            "outbounds": [{
                "type": "hysteria2",
                "tag": "HY2-SG",
                "server": "1.2.3.4",
                "server_port": 443,
                "password": "secret",
                "tls": {
                    "enabled": true,
                    "insecure": false,
                    "server_name": "example.com",
                    "alpn": ["h3"]
                }
            }]
        }"#;

        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "HY2-SG");
        let tls = nodes[0].transport.as_ref().unwrap().tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("example.com"));
        assert!(!tls.skip_cert_verify);
    }

    #[test]
    fn test_skip_non_proxy_outbounds() {
        let json = r#"{
            "outbounds": [
                {"type": "direct", "tag": "direct"},
                {"type": "block", "tag": "block"},
                {"type": "dns", "tag": "dns"},
                {"type": "selector", "tag": "select"}
            ]
        }"#;
        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_is_singbox_config() {
        assert!(is_singbox_config(r#"{"outbounds":[{"server_port":443}]}"#));
        assert!(!is_singbox_config(r#"{"inbounds":[]}"#));
        assert!(!is_singbox_config("proxies:"));
    }

    #[test]
    fn test_parse_single_outbound() {
        let json = r#"{
            "type": "tuic",
            "tag": "MY-1",
            "server": "1.2.3.4",
            "server_port": 443,
            "uuid": "550e8400-e29b-41d4-a716-446655440000",
            "password": "pass",
            "tls": {"enabled": true, "insecure": true, "server_name": "sni.test.com"}
        }"#;
        let node = parse_singbox_outbound(json).unwrap().unwrap();
        assert_eq!(node.name, "MY-1");
        let tls = node.transport.as_ref().unwrap().tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("sni.test.com"));
    }
}
