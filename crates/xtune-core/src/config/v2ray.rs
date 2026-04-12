use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose;
use serde::Deserialize;
use std::collections::HashMap;

use super::model::*;

/// V2Ray JSON outbound config (simplified)
#[derive(Debug, Deserialize)]
struct V2RayConfig {
    #[serde(default)]
    outbounds: Vec<V2RayOutbound>,
}

#[derive(Debug, Deserialize)]
struct V2RayOutbound {
    protocol: String,
    #[serde(default)]
    settings: serde_json::Value,
    #[serde(rename = "streamSettings")]
    stream_settings: Option<V2RayStreamSettings>,
    tag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct V2RayStreamSettings {
    network: Option<String>,
    security: Option<String>,
    #[serde(rename = "tlsSettings")]
    tls_settings: Option<V2RayTlsSettings>,
    #[serde(rename = "wsSettings")]
    ws_settings: Option<V2RayWsSettings>,
    #[serde(rename = "realitySettings")]
    reality_settings: Option<V2RayRealitySettings>,
}

#[derive(Debug, Deserialize)]
struct V2RayTlsSettings {
    #[serde(rename = "serverName")]
    server_name: Option<String>,
    #[serde(default)]
    alpn: Vec<String>,
    fingerprint: Option<String>,
    #[serde(rename = "allowInsecure", default)]
    allow_insecure: bool,
}

#[derive(Debug, Deserialize)]
struct V2RayWsSettings {
    path: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct V2RayRealitySettings {
    #[serde(rename = "publicKey")]
    public_key: Option<String>,
    #[serde(rename = "shortId")]
    short_id: Option<String>,
    #[serde(rename = "serverName")]
    server_name: Option<String>,
}

/// VMess share link JSON format
#[derive(Debug, Deserialize)]
struct VMessShareLink {
    #[serde(default = "default_v")]
    v: String,
    ps: Option<String>,
    add: String,
    port: serde_json::Value,
    id: String,
    #[serde(default)]
    aid: serde_json::Value,
    #[serde(default = "default_cipher")]
    scy: String,
    net: Option<String>,
    #[serde(rename = "type")]
    header_type: Option<String>,
    host: Option<String>,
    path: Option<String>,
    tls: Option<String>,
    sni: Option<String>,
    alpn: Option<String>,
}

fn default_v() -> String {
    "2".to_string()
}

fn default_cipher() -> String {
    "auto".to_string()
}

/// Check if content looks like V2Ray JSON config
pub fn is_v2ray_config(content: &str) -> bool {
    content.contains("\"outbounds\"") || content.contains("\"inbounds\"")
}

/// Parse V2Ray JSON config into nodes
pub fn parse_v2ray_config(json_str: &str) -> Result<Vec<Node>> {
    let config: V2RayConfig =
        serde_json::from_str(json_str).context("Failed to parse V2Ray JSON")?;

    let mut nodes = Vec::new();
    for outbound in &config.outbounds {
        match convert_v2ray_outbound(outbound) {
            Ok(Some(node)) => nodes.push(node),
            Ok(None) => {} // skip non-proxy outbounds (direct, block, etc.)
            Err(e) => {
                tracing::warn!(
                    "Skipping outbound '{}': {}",
                    outbound.tag.as_deref().unwrap_or("unknown"),
                    e
                );
            }
        }
    }

    Ok(nodes)
}

/// Parse a vmess:// share link
pub fn parse_vmess_uri(uri: &str) -> Result<Node> {
    let encoded = uri
        .strip_prefix("vmess://")
        .context("Not a vmess:// URI")?;

    // Remove fragment (name after #)
    let encoded = encoded.split('#').next().unwrap_or(encoded);

    let decoded = general_purpose::STANDARD
        .decode(encoded.trim())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded.trim()))
        .context("Failed to Base64 decode vmess link")?;

    let json_str = String::from_utf8(decoded).context("Invalid UTF-8 in vmess link")?;
    let link: VMessShareLink =
        serde_json::from_str(&json_str).context("Failed to parse vmess JSON")?;

    let port: u16 = match &link.port {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(443) as u16,
        serde_json::Value::String(s) => s.parse().unwrap_or(443),
        _ => 443,
    };

    let alter_id: u32 = match &link.aid {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u32,
        serde_json::Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    };

    let name = link
        .ps
        .clone()
        .unwrap_or_else(|| format!("VMess-{}", &link.add));

    let has_tls = link.tls.as_deref() == Some("tls");
    let network = link.net.as_deref().unwrap_or("tcp");

    let transport = build_vmess_transport(network, has_tls, &link);

    Ok(Node {
        name,
        server: link.add,
        port,
        protocol: ProxyProtocol::VMess {
            uuid: link.id,
            alter_id,
            cipher: link.scy,
            udp: false,
        },
        transport,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

/// Parse a vless:// share link
pub fn parse_vless_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri
        .strip_prefix("vless://")
        .context("Not a vless:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse VLESS URI")?;

    let uuid = parsed.username().to_string();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name = url::form_urlencoded::parse(
        parsed.fragment().unwrap_or("").as_bytes(),
    )
    .next()
    .map(|(k, _)| k.to_string())
    .unwrap_or_else(|| {
        parsed.fragment().unwrap_or(&format!("VLESS-{}", server)).to_string()
    });

    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

    let flow = params.get("flow").cloned();
    let security = params.get("security").map(|s| s.as_str()).unwrap_or("none");
    let transport_type_str = params.get("type").map(|s| s.as_str()).unwrap_or("tcp");

    let transport = build_uri_transport(security, transport_type_str, &params);

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::VLess {
            uuid,
            flow,
            udp: true,
        },
        transport,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

/// Parse a ss:// share link
pub fn parse_ss_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri.strip_prefix("ss://").context("Not a ss:// URI")?;

    // Extract name from fragment
    let (main, fragment) = match without_scheme.split_once('#') {
        Some((m, f)) => (m, Some(urlencoding::decode(f).unwrap_or_default().to_string())),
        None => (without_scheme, None),
    };

    // Format 1: ss://base64(method:password)@host:port
    // Format 2: ss://method:password@host:port (SIP002)
    if let Some((userinfo, hostport)) = main.split_once('@') {
        let decoded = general_purpose::STANDARD
            .decode(userinfo.trim())
            .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(userinfo.trim()))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_else(|_| userinfo.to_string());

        let (cipher, password) = decoded
            .split_once(':')
            .context("Invalid SS userinfo format")?;

        let parsed = url::Url::parse(&format!("scheme://user@{}", hostport))
            .context("Failed to parse SS host:port")?;
        let server = parsed
            .host_str()
            .context("Missing server host")?
            .to_string();
        let port = parsed.port().unwrap_or(8388);
        let name = fragment.unwrap_or_else(|| format!("SS-{}", server));

        return Ok(Node {
            name,
            server,
            port,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: cipher.to_string(),
                password: password.to_string(),
                udp: true,
            },
            transport: None,
            latency_ms: None,
            extra: HashMap::new(),
        });
    }

    // Format 3: ss://base64(method:password@host:port)
    let decoded = general_purpose::STANDARD
        .decode(main.trim())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(main.trim()))
        .context("Failed to decode SS base64")?;
    let decoded_str = String::from_utf8(decoded).context("Invalid UTF-8")?;

    let (method_pass, hostport) = decoded_str
        .split_once('@')
        .context("Invalid SS format")?;
    let (cipher, password) = method_pass
        .split_once(':')
        .context("Invalid SS method:password")?;

    let parsed = url::Url::parse(&format!("scheme://user@{}", hostport))
        .context("Failed to parse SS host:port")?;
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(8388);
    let name = fragment.unwrap_or_else(|| format!("SS-{}", server));

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Shadowsocks {
            cipher: cipher.to_string(),
            password: password.to_string(),
            udp: true,
        },
        transport: None,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

/// Parse a tuic:// share link
pub fn parse_tuic_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri
        .strip_prefix("tuic://")
        .context("Not a tuic:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse TUIC URI")?;

    let uuid = parsed.username().to_string();
    let password = parsed
        .password()
        .unwrap_or("")
        .to_string();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name = parsed
        .fragment()
        .unwrap_or(&format!("TUIC-{}", server))
        .to_string();

    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    let congestion_control = params
        .get("congestion_control")
        .cloned()
        .unwrap_or_else(|| "bbr".to_string());

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            udp: true,
        },
        transport: None,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

/// Parse a trojan:// share link
pub fn parse_trojan_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri
        .strip_prefix("trojan://")
        .context("Not a trojan:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse Trojan URI")?;

    let password = parsed.username().to_string();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name = parsed
        .fragment()
        .unwrap_or(&format!("Trojan-{}", server))
        .to_string();

    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    let security = params.get("security").map(|s| s.as_str()).unwrap_or("tls");
    let transport_type_str = params.get("type").map(|s| s.as_str()).unwrap_or("tcp");
    let transport = build_uri_transport(security, transport_type_str, &params);

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Trojan {
            password,
            udp: true,
        },
        transport,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

/// Parse any supported proxy URI
pub fn parse_proxy_uri(uri: &str) -> Result<Node> {
    let uri = uri.trim();
    if uri.starts_with("vmess://") {
        parse_vmess_uri(uri)
    } else if uri.starts_with("vless://") {
        parse_vless_uri(uri)
    } else if uri.starts_with("ss://") {
        parse_ss_uri(uri)
    } else if uri.starts_with("tuic://") {
        parse_tuic_uri(uri)
    } else if uri.starts_with("trojan://") {
        parse_trojan_uri(uri)
    } else if uri.starts_with("hysteria2://") || uri.starts_with("hy2://") {
        parse_hysteria2_uri(uri)
    } else {
        anyhow::bail!("Unsupported URI scheme: {}", uri.split("://").next().unwrap_or("unknown"))
    }
}

/// Parse a hysteria2:// or hy2:// share link
pub fn parse_hysteria2_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri
        .strip_prefix("hysteria2://")
        .or_else(|| uri.strip_prefix("hy2://"))
        .context("Not a hysteria2/hy2 URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse Hysteria2 URI")?;

    let password = parsed.username().to_string();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name = parsed
        .fragment()
        .unwrap_or(&format!("Hy2-{}", server))
        .to_string();

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Hysteria2 {
            password,
            udp: true,
        },
        transport: None,
        latency_ms: None,
        extra: HashMap::new(),
    })
}

fn convert_v2ray_outbound(outbound: &V2RayOutbound) -> Result<Option<Node>> {
    match outbound.protocol.as_str() {
        "vmess" => {
            let vnext = outbound
                .settings
                .get("vnext")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .context("Missing vnext")?;

            let server = vnext
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = vnext
                .get("port")
                .and_then(|v| v.as_u64())
                .unwrap_or(443) as u16;

            let user = vnext
                .get("users")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .context("Missing users")?;

            let uuid = user
                .get("id")
                .and_then(|v| v.as_str())
                .context("Missing user id")?
                .to_string();
            let alter_id = user
                .get("alterId")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let cipher = user
                .get("security")
                .and_then(|v| v.as_str())
                .unwrap_or("auto")
                .to_string();

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("VMess-{}", server));

            let transport = outbound
                .stream_settings
                .as_ref()
                .and_then(|ss| build_v2ray_transport(ss));

            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::VMess {
                    uuid,
                    alter_id,
                    cipher,
                    udp: true,
                },
                transport,
                latency_ms: None,
                extra: HashMap::new(),
            }))
        }
        "vless" => {
            let vnext = outbound
                .settings
                .get("vnext")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .context("Missing vnext")?;

            let server = vnext
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = vnext
                .get("port")
                .and_then(|v| v.as_u64())
                .unwrap_or(443) as u16;

            let user = vnext
                .get("users")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .context("Missing users")?;

            let uuid = user
                .get("id")
                .and_then(|v| v.as_str())
                .context("Missing user id")?
                .to_string();
            let flow = user
                .get("flow")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("VLESS-{}", server));

            let transport = outbound
                .stream_settings
                .as_ref()
                .and_then(|ss| build_v2ray_transport(ss));

            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::VLess {
                    uuid,
                    flow,
                    udp: true,
                },
                transport,
                latency_ms: None,
                extra: HashMap::new(),
            }))
        }
        "shadowsocks" => {
            let servers = outbound
                .settings
                .get("servers")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .context("Missing servers")?;

            let server = servers
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = servers
                .get("port")
                .and_then(|v| v.as_u64())
                .unwrap_or(8388) as u16;
            let cipher = servers
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("aes-256-gcm")
                .to_string();
            let password = servers
                .get("password")
                .and_then(|v| v.as_str())
                .context("Missing password")?
                .to_string();

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("SS-{}", server));

            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::Shadowsocks {
                    cipher,
                    password,
                    udp: true,
                },
                transport: None,
                latency_ms: None,
                extra: HashMap::new(),
            }))
        }
        // Skip non-proxy protocols
        "freedom" | "blackhole" | "dns" | "loopback" => Ok(None),
        other => {
            tracing::debug!("Unsupported V2Ray protocol: {}", other);
            Ok(None)
        }
    }
}

fn build_v2ray_transport(ss: &V2RayStreamSettings) -> Option<TransportConfig> {
    let security = ss.security.as_deref().unwrap_or("none");
    let network = ss.network.as_deref().unwrap_or("tcp");

    if security == "none" && network == "tcp" {
        return None;
    }

    if security == "reality" {
        if let Some(ref rs) = ss.reality_settings {
            return Some(TransportConfig {
                transport_type: TransportType::Reality,
                tls: None,
                ws: None,
                reality: Some(RealityConfig {
                    public_key: rs.public_key.clone().unwrap_or_default(),
                    short_id: rs.short_id.clone().unwrap_or_default(),
                    sni: rs.server_name.clone(),
                }),
            });
        }
    }

    let tls = if security == "tls" {
        Some(TlsConfig {
            sni: ss
                .tls_settings
                .as_ref()
                .and_then(|t| t.server_name.clone()),
            skip_cert_verify: ss
                .tls_settings
                .as_ref()
                .map(|t| t.allow_insecure)
                .unwrap_or(false),
            alpn: ss.tls_settings.as_ref().map(|t| t.alpn.clone()),
            fingerprint: ss
                .tls_settings
                .as_ref()
                .and_then(|t| t.fingerprint.clone()),
        })
    } else {
        None
    };

    let ws = if network == "ws" {
        ss.ws_settings.as_ref().map(|w| {
            let host = w.headers.get("Host").cloned();
            WsConfig {
                path: w.path.clone(),
                host,
                headers: Some(w.headers.clone()),
            }
        })
    } else {
        None
    };

    let transport_type = match network {
        "ws" => TransportType::WebSocket,
        _ if security == "tls" => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Some(TransportConfig {
        transport_type,
        tls,
        ws,
        reality: None,
    })
}

fn build_vmess_transport(
    network: &str,
    has_tls: bool,
    link: &VMessShareLink,
) -> Option<TransportConfig> {
    if !has_tls && network == "tcp" {
        return None;
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: link.sni.clone().or_else(|| link.host.clone()),
            skip_cert_verify: false,
            alpn: link.alpn.as_ref().map(|a| a.split(',').map(|s| s.trim().to_string()).collect()),
            fingerprint: None,
        })
    } else {
        None
    };

    let ws = if network == "ws" {
        Some(WsConfig {
            path: link.path.clone(),
            host: link.host.clone(),
            headers: None,
        })
    } else {
        None
    };

    let transport_type = match network {
        "ws" => TransportType::WebSocket,
        _ if has_tls => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Some(TransportConfig {
        transport_type,
        tls,
        ws,
        reality: None,
    })
}

fn build_uri_transport(
    security: &str,
    transport_type_str: &str,
    params: &HashMap<String, String>,
) -> Option<TransportConfig> {
    if security == "none" && transport_type_str == "tcp" {
        return None;
    }

    if security == "reality" {
        return Some(TransportConfig {
            transport_type: TransportType::Reality,
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key: params.get("pbk").cloned().unwrap_or_default(),
                short_id: params.get("sid").cloned().unwrap_or_default(),
                sni: params.get("sni").cloned(),
            }),
        });
    }

    let tls = if security == "tls" {
        Some(TlsConfig {
            sni: params.get("sni").cloned(),
            skip_cert_verify: params.get("allowInsecure").map(|v| v == "1").unwrap_or(false),
            alpn: params.get("alpn").map(|a| a.split(',').map(|s| s.to_string()).collect()),
            fingerprint: params.get("fp").cloned(),
        })
    } else {
        None
    };

    let ws = if transport_type_str == "ws" {
        Some(WsConfig {
            path: params.get("path").cloned(),
            host: params.get("host").cloned(),
            headers: None,
        })
    } else {
        None
    };

    let transport_type = match transport_type_str {
        "ws" => TransportType::WebSocket,
        _ if security == "tls" => TransportType::Tls,
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
    fn test_parse_vmess_uri() {
        let json = serde_json::json!({
            "v": "2",
            "ps": "Test-VMess",
            "add": "example.com",
            "port": "443",
            "id": "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4",
            "aid": "0",
            "scy": "auto",
            "net": "ws",
            "host": "example.com",
            "path": "/v2ray",
            "tls": "tls",
            "sni": "example.com"
        });
        let encoded = general_purpose::STANDARD.encode(json.to_string());
        let uri = format!("vmess://{}", encoded);

        let node = parse_vmess_uri(&uri).unwrap();
        assert_eq!(node.name, "Test-VMess");
        assert_eq!(node.server, "example.com");
        assert_eq!(node.port, 443);
        match &node.protocol {
            ProxyProtocol::VMess { uuid, .. } => {
                assert_eq!(uuid, "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4");
            }
            _ => panic!("Expected VMess"),
        }
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::WebSocket);
    }

    #[test]
    fn test_parse_vless_uri() {
        let uri = "vless://uuid-here@example.com:443?encryption=none&security=tls&sni=example.com&type=ws&path=/v2ray#My-VLESS";
        let node = parse_vless_uri(uri).unwrap();
        assert_eq!(node.name, "My-VLESS");
        assert_eq!(node.server, "example.com");
        assert_eq!(node.port, 443);
    }

    #[test]
    fn test_parse_ss_uri_sip002() {
        let userinfo = general_purpose::STANDARD.encode("aes-256-gcm:password123");
        let uri = format!("ss://{}@1.2.3.4:8388#My-SS", userinfo);
        let node = parse_ss_uri(&uri).unwrap();
        assert_eq!(node.name, "My-SS");
        assert_eq!(node.server, "1.2.3.4");
        assert_eq!(node.port, 8388);
        match &node.protocol {
            ProxyProtocol::Shadowsocks { cipher, password, .. } => {
                assert_eq!(cipher, "aes-256-gcm");
                assert_eq!(password, "password123");
            }
            _ => panic!("Expected SS"),
        }
    }

    #[test]
    fn test_parse_tuic_uri() {
        let uri = "tuic://uuid-here:password@example.com:443?congestion_control=bbr#TUIC-SG";
        let node = parse_tuic_uri(uri).unwrap();
        assert_eq!(node.name, "TUIC-SG");
        assert_eq!(node.server, "example.com");
        match &node.protocol {
            ProxyProtocol::Tuic { congestion_control, .. } => {
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
    }

    #[test]
    fn test_parse_trojan_uri() {
        let uri = "trojan://password@example.com:443?sni=example.com#Trojan-JP";
        let node = parse_trojan_uri(uri).unwrap();
        assert_eq!(node.name, "Trojan-JP");
        assert_eq!(node.server, "example.com");
    }

    #[test]
    fn test_parse_v2ray_config() {
        let json = r#"{
            "outbounds": [
                {
                    "protocol": "vmess",
                    "tag": "proxy",
                    "settings": {
                        "vnext": [{
                            "address": "example.com",
                            "port": 443,
                            "users": [{
                                "id": "test-uuid",
                                "alterId": 0,
                                "security": "auto"
                            }]
                        }]
                    },
                    "streamSettings": {
                        "network": "ws",
                        "security": "tls",
                        "tlsSettings": {
                            "serverName": "example.com"
                        },
                        "wsSettings": {
                            "path": "/path",
                            "headers": {"Host": "example.com"}
                        }
                    }
                },
                {
                    "protocol": "freedom",
                    "tag": "direct"
                }
            ]
        }"#;

        let nodes = parse_v2ray_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "proxy");
    }
}
