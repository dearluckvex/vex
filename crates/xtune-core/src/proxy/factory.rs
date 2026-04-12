use std::sync::Arc;

use anyhow::{bail, Result};

use crate::config::model::{Node, ProxyProtocol};

use super::connector::{DirectOutbound, SharedOutbound};
use super::trojan::TrojanOutbound;
use super::vless::VlessOutbound;

/// Create an outbound connector from a Node configuration.
pub fn create_outbound(node: &Node) -> Result<SharedOutbound> {
    match &node.protocol {
        ProxyProtocol::VLess { uuid, .. } => {
            let outbound = VlessOutbound::new(
                &node.server,
                node.port,
                uuid,
                node.transport.as_ref(),
            )?;
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Trojan { password, .. } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let outbound = TrojanOutbound::new(
                &node.server,
                node.port,
                password,
                tls_config,
            );
            Ok(SharedOutbound(Arc::new(outbound)))
        }

        ProxyProtocol::Shadowsocks { cipher, password, .. } => {
            // TODO: Phase 4b - implement Shadowsocks outbound
            tracing::warn!(
                "Shadowsocks outbound not yet implemented (cipher={}), using direct",
                cipher
            );
            let _ = password;
            Ok(SharedOutbound(Arc::new(DirectOutbound)))
        }

        ProxyProtocol::VMess { uuid, cipher, .. } => {
            // TODO: Phase 4b - implement VMess AEAD outbound
            tracing::warn!(
                "VMess outbound not yet implemented (uuid={}, cipher={}), using direct",
                uuid,
                cipher
            );
            Ok(SharedOutbound(Arc::new(DirectOutbound)))
        }

        ProxyProtocol::Tuic { uuid, .. } => {
            // TODO: Phase 4b - implement TUIC v5 outbound
            tracing::warn!(
                "TUIC v5 outbound not yet implemented (uuid={}), using direct",
                uuid
            );
            Ok(SharedOutbound(Arc::new(DirectOutbound)))
        }

        ProxyProtocol::Hysteria2 { password, .. } => {
            // TODO: Phase 4b - implement Hysteria2 outbound
            tracing::warn!(
                "Hysteria2 outbound not yet implemented (password={}...)",
                &password[..password.len().min(4)]
            );
            Ok(SharedOutbound(Arc::new(DirectOutbound)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{TransportConfig, TransportType, TlsConfig};

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
    fn test_create_ss_outbound_fallback() {
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

        // Falls back to direct until SS is implemented
        let outbound = create_outbound(&node).unwrap();
        assert_eq!(outbound.0.name(), "direct");
    }
}
