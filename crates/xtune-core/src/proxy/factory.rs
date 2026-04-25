use std::sync::Arc;

use anyhow::Result;

use crate::config::model::{Node, ProxyProtocol};

use super::connector::SharedOutbound;
use super::hysteria2::Hysteria2Outbound;
use super::ss::{SsOutbound, normalize_ss_cipher};
use super::trojan::TrojanOutbound;
use super::tuic::TuicOutbound;
use super::vless::VlessOutbound;
use super::vmess::VMessOutbound;

/// Number of retry attempts for transient connection failures (TCP-based protocols).
const DEFAULT_RETRY_ATTEMPTS: u32 = 3;

/// Create an outbound connector from a Node configuration.
///
/// TCP-based outbounds (VLess, VMess, Trojan, Shadowsocks) are wrapped with
/// retry logic to handle transient network blips that self-resolve quickly.
///
/// QUIC-based outbounds (TUIC, Hysteria2) are NOT wrapped with outer retry:
/// they maintain a persistent connection internally via `get_connection()`,
/// which already handles reconnection. Adding outer retry would only multiply
/// the already-long QUIC handshake timeout on truly unreachable servers.
pub fn create_outbound(node: &Node) -> Result<SharedOutbound> {
    match &node.protocol {
        ProxyProtocol::VLess { uuid, .. } => {
            let out = VlessOutbound::new(&node.server, node.port, uuid, node.transport.as_ref())?;
            Ok(SharedOutbound(Arc::new(out)).with_retry(DEFAULT_RETRY_ATTEMPTS))
        }

        ProxyProtocol::Trojan { password, .. } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let out = TrojanOutbound::new(&node.server, node.port, password, tls_config);
            Ok(SharedOutbound(Arc::new(out)).with_retry(DEFAULT_RETRY_ATTEMPTS))
        }

        ProxyProtocol::Shadowsocks {
            cipher, password, ..
        } => {
            let normalized_cipher = normalize_ss_cipher(cipher);
            let out = SsOutbound::new(&node.server, node.port, normalized_cipher, password)?;
            Ok(SharedOutbound(Arc::new(out)).with_retry(DEFAULT_RETRY_ATTEMPTS))
        }

        ProxyProtocol::VMess { uuid, cipher, .. } => {
            let out = VMessOutbound::new(
                &node.server,
                node.port,
                uuid,
                cipher,
                node.transport.as_ref(),
            )?;
            Ok(SharedOutbound(Arc::new(out)).with_retry(DEFAULT_RETRY_ATTEMPTS))
        }

        // QUIC-based protocols: no outer retry wrapper.
        // TuicOutbound::get_connection() caches the QUIC connection and reconnects
        // automatically when it detects the connection is closed. Outer retry would
        // only add multiples of the QUIC handshake timeout on dead servers.
        ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            ..
        } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let out = TuicOutbound::new(
                &node.server,
                node.port,
                uuid,
                password,
                congestion_control,
                tls_config,
            )?;
            Ok(SharedOutbound(Arc::new(out)))
        }

        ProxyProtocol::Hysteria2 { password, .. } => {
            let tls_config = node.transport.as_ref().and_then(|t| t.tls.as_ref());
            let out = Hysteria2Outbound::new(&node.server, node.port, password, tls_config)?;
            Ok(SharedOutbound(Arc::new(out)))
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
