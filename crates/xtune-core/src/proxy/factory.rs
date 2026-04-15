use std::sync::Arc;

use anyhow::Result;

use crate::config::model::{Node, ProxyProtocol};

use super::connector::{DirectOutbound, SharedOutbound};
use super::hysteria2::Hysteria2Outbound;
use super::ss::{SsOutbound, normalize_ss_cipher};
use super::trojan::TrojanOutbound;
use super::tuic::TuicOutbound;
use super::vless::VlessOutbound;
use super::vmess::VMessOutbound;

/// Create an outbound connector from a Node configuration.
pub fn create_outbound(node: &Node) -> Result<SharedOutbound> {
    match &node.protocol {
        ProxyProtocol::VLess { uuid, .. } => {
            let outbound =
                VlessOutbound::new(&node.server, node.port, uuid, node.transport.as_ref())?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Trojan { password, .. } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let outbound = TrojanOutbound::new(&node.server, node.port, password, tls_config);
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Shadowsocks {
            cipher, password, ..
        } => {
            let normalized_cipher = normalize_ss_cipher(cipher);
            let outbound = SsOutbound::new(&node.server, node.port, normalized_cipher, password)?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::VMess { uuid, cipher, .. } => {
            let outbound = VMessOutbound::new(
                &node.server,
                node.port,
                uuid,
                cipher,
                node.transport.as_ref(),
            )?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            ..
        } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let outbound = TuicOutbound::new(
                &node.server,
                node.port,
                uuid,
                password,
                congestion_control,
                tls_config,
            )?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Hysteria2 { password, .. } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let outbound = Hysteria2Outbound::new(
                &node.server,
                node.port,
                password,
                tls_config,
            )?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{TlsConfig, TransportConfig, TransportType};

    #[test]
    fn test_create_vless_outbound() {
        let node = Node {
            name: "test-vless".to_string(),
            server: "server.com".to_string(),
            port: 443,
            protocol: ProxyProtocol::VLess {
                uuid: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                flow: None,
                udp: false,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Tls,
                tls: Some(TlsConfig {
                    sni: Some("server.com".to_string()),
                    skip_cert_verify: false,
                    alpn: None,
                    fingerprint: None,
                }),
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            extra: Default::default(),
        };

        let outbound = create_outbound(&node).unwrap();
        assert_eq!(outbound.0.name(), "vless");
    }

    #[test]
    fn test_create_trojan_outbound() {
        let node = Node {
            name: "test-trojan".to_string(),
            server: "trojan.server.com".to_string(),
            port: 443,
            protocol: ProxyProtocol::Trojan {
                password: "my_password".to_string(),
                udp: false,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Tls,
                tls: Some(TlsConfig {
                    sni: Some("trojan.server.com".to_string()),
                    skip_cert_verify: false,
                    alpn: None,
                    fingerprint: None,
                }),
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            extra: Default::default(),
        };

        let outbound = create_outbound(&node).unwrap();
        assert_eq!(outbound.0.name(), "trojan");
    }

    #[test]
    fn test_create_ss_outbound() {
        let node = Node {
            name: "test-ss".to_string(),
            server: "ss.server.com".to_string(),
            port: 8388,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "password123".to_string(),
                udp: false,
            },
            transport: None,
            latency_ms: None,
            extra: Default::default(),
        };

        let outbound = create_outbound(&node).unwrap();
        assert_eq!(outbound.0.name(), "shadowsocks");
    }
}
