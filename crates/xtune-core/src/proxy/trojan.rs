use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use sha2::{Digest, Sha224};
use tokio::io::AsyncWriteExt;

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::transport::connect_with_tls;

/// Trojan outbound connector.
/// Trojan always uses TLS - the protocol is designed to look like normal HTTPS traffic.
pub struct TrojanOutbound {
    server: String,
    port: u16,
    password_hash: String, // hex(SHA224(password)), 56 chars
    tls_config: Option<TlsConfig>,
}

impl TrojanOutbound {
    pub fn new(server: &str, port: u16, password: &str, tls_config: Option<&TlsConfig>) -> Self {
        let hash = Sha224::digest(password.as_bytes());
        let password_hash = hex::encode(hash);

        Self {
            server: server.to_string(),
            port,
            password_hash,
            tls_config: tls_config.cloned(),
        }
    }
}

impl Outbound for TrojanOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let target_host = host.to_string();
        Box::pin(async move {
            // Trojan always uses TLS
            let mut stream = connect_with_tls(
                &self.server,
                self.port,
                self.tls_config.as_ref(),
                true, // always TLS
            )
            .await?;

            // Send Trojan request
            let header = build_trojan_request(&self.password_hash, &target_host, port);
            stream.write_all(&header).await?;

            // No response header in Trojan - data flows immediately after request
            Ok(stream)
        })
    }

    fn name(&self) -> &str {
        "trojan"
    }
}

/// Build Trojan request header.
///
/// Format:
/// [hex(SHA224(password))] [CRLF]
/// [command(1)] [addr_type(1)] [addr_data...] [port(2, big-endian)] [CRLF]
/// [payload...]
fn build_trojan_request(password_hash: &str, host: &str, port: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);

    // Password hash (56 hex characters)
    buf.extend_from_slice(password_hash.as_bytes());
    // CRLF
    buf.extend_from_slice(b"\r\n");

    // Command: CONNECT (0x01)
    buf.push(0x01);

    // Address (same format as SOCKS5)
    if let Ok(ipv4) = host.parse::<std::net::Ipv4Addr>() {
        buf.push(0x01); // IPv4
        buf.extend_from_slice(&ipv4.octets());
    } else if let Ok(ipv6) = host.parse::<std::net::Ipv6Addr>() {
        buf.push(0x04); // IPv6
        buf.extend_from_slice(&ipv6.octets());
    } else {
        // Domain name
        buf.push(0x03); // Domain
        let domain_bytes = host.as_bytes();
        buf.push(domain_bytes.len() as u8);
        buf.extend_from_slice(domain_bytes);
    }

    // Port (big-endian)
    buf.extend_from_slice(&port.to_be_bytes());

    // CRLF
    buf.extend_from_slice(b"\r\n");

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trojan_password_hash() {
        let hash = Sha224::digest(b"test_password");
        let hex_hash = hex::encode(hash);
        assert_eq!(hex_hash.len(), 56);
    }

    #[test]
    fn test_build_trojan_request_domain() {
        let hash = hex::encode(Sha224::digest(b"pass"));
        let buf = build_trojan_request(&hash, "example.com", 443);

        // Password hash (56 bytes) + CRLF (2) = 58
        assert_eq!(&buf[56..58], b"\r\n");
        // Command
        assert_eq!(buf[58], 0x01); // CONNECT
        // Addr type
        assert_eq!(buf[59], 0x03); // Domain
        assert_eq!(buf[60], 11); // "example.com" length
        assert_eq!(&buf[61..72], b"example.com");
        // Port
        assert_eq!(u16::from_be_bytes([buf[72], buf[73]]), 443);
        // CRLF
        assert_eq!(&buf[74..76], b"\r\n");
    }

    #[test]
    fn test_build_trojan_request_ipv4() {
        let hash = hex::encode(Sha224::digest(b"pass"));
        let buf = build_trojan_request(&hash, "192.168.1.1", 8080);

        assert_eq!(buf[59], 0x01); // IPv4
        assert_eq!(&buf[60..64], &[192, 168, 1, 1]);
        assert_eq!(u16::from_be_bytes([buf[64], buf[65]]), 8080);
    }

    #[test]
    fn test_trojan_outbound_new() {
        let outbound = TrojanOutbound::new("server.com", 443, "password123", None);
        assert_eq!(outbound.server, "server.com");
        assert_eq!(outbound.port, 443);
        assert_eq!(outbound.password_hash.len(), 56);
        assert_eq!(outbound.name(), "trojan");
    }
}
