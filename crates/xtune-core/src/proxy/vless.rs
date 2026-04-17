use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::model::{TlsConfig, TransportConfig, TransportType};

use super::connector::{BoxProxyStream, Outbound, ProxyStream};
use super::pool::{ConnFactory, ConnPool};
use super::transport::{build_tls_connector, connect_with_connector};

/// Factory that creates TCP+TLS connections to the VLESS proxy server.
struct VlessConnFactory {
    server: String,
    port: u16,
    sni: String,
    use_tls: bool,
    connector: craft_tls::TlsConnector,
}

impl ConnFactory for VlessConnFactory {
    fn create(&self) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        Box::pin(async {
            connect_with_connector(
                &self.server,
                self.port,
                &self.connector,
                &self.sni,
                self.use_tls,
                std::time::Duration::from_secs(15),
            )
            .await
        })
    }
}

/// VLESS outbound connector.
pub struct VlessOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    pool: ConnPool,
}

impl VlessOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        let uuid = uuid::Uuid::parse_str(uuid_str)?;

        let (tls_config, use_tls) = match transport {
            Some(t) => {
                let needs_tls = matches!(
                    t.transport_type,
                    TransportType::Tls | TransportType::Reality
                );
                // For Reality transport, build a TLS config from the Reality settings
                // since the normal `tls` field is typically None.
                let tls = if t.transport_type == TransportType::Reality && t.tls.is_none() {
                    if let Some(ref reality) = t.reality {
                        tracing::info!(
                            "VLESS Reality: using SNI '{}' with skip_cert_verify (no uTLS fingerprinting)",
                            reality.sni.as_deref().unwrap_or(server)
                        );
                        Some(crate::config::model::TlsConfig {
                            sni: reality.sni.clone(),
                            skip_cert_verify: true,
                            alpn: Some(vec!["h2".to_string(), "http/1.1".to_string()]),
                            fingerprint: None,
                        })
                    } else {
                        t.tls.clone()
                    }
                } else {
                    t.tls.clone()
                };
                (tls, needs_tls)
            }
            None => (None, false),
        };

        let sni = tls_config
            .as_ref()
            .and_then(|c| c.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let connector = build_tls_connector(tls_config.as_ref());

        let factory = Arc::new(VlessConnFactory {
            server: server.to_string(),
            port,
            sni,
            use_tls,
            connector,
        });
        let pool = ConnPool::new(factory);

        Ok(Self {
            server: server.to_string(),
            port,
            uuid: *uuid.as_bytes(),
            pool,
        })
    }
}

impl Outbound for VlessOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let target_host = host.to_string();
        Box::pin(async move {
            let mut stream = self.pool.get().await.with_context(|| {
                format!(
                    "VLESS: failed to connect to {}:{} (target: {}:{})",
                    self.server, self.port, target_host, port
                )
            })?;

            let header = build_vless_request(&self.uuid, &target_host, port);
            stream.write_all(&header).await?;

            read_vless_response(&mut stream).await?;

            Ok(stream)
        })
    }

    fn name(&self) -> &str {
        "vless"
    }
}

/// Build VLESS request header.
///
/// Format:
/// [version(1)] [UUID(16)] [addon_len(1)] [addons...]
/// [command(1)] [port(2, big-endian)] [addr_type(1)] [addr_data...]
fn build_vless_request(uuid: &[u8; 16], host: &str, port: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);

    // Version
    buf.push(0x00);
    // UUID
    buf.extend_from_slice(uuid);
    // Addon length (no addons)
    buf.push(0x00);
    // Command: TCP (0x01)
    buf.push(0x01);
    // Port (big-endian)
    buf.extend_from_slice(&port.to_be_bytes());

    // Address
    if let Ok(ipv4) = host.parse::<std::net::Ipv4Addr>() {
        buf.push(0x01); // IPv4
        buf.extend_from_slice(&ipv4.octets());
    } else if let Ok(ipv6) = host.parse::<std::net::Ipv6Addr>() {
        buf.push(0x03); // IPv6
        buf.extend_from_slice(&ipv6.octets());
    } else {
        // Domain name
        buf.push(0x02); // Domain
        let domain_bytes = host.as_bytes();
        buf.push(domain_bytes.len() as u8);
        buf.extend_from_slice(domain_bytes);
    }

    buf
}

/// Read VLESS response header.
///
/// Format: [version(1)] [addon_len(1)] [addons...]
async fn read_vless_response(stream: &mut Box<dyn ProxyStream>) -> Result<()> {
    let version = stream.read_u8().await?;
    if version != 0x00 {
        bail!("Unexpected VLESS response version: {}", version);
    }
    let addon_len = stream.read_u8().await?;
    if addon_len > 0 {
        let mut addon = vec![0u8; addon_len as usize];
        stream.read_exact(&mut addon).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_vless_request_domain() {
        let uuid = [0x01u8; 16];
        let buf = build_vless_request(&uuid, "example.com", 443);

        assert_eq!(buf[0], 0x00); // version
        assert_eq!(&buf[1..17], &[0x01u8; 16]); // UUID
        assert_eq!(buf[17], 0x00); // no addons
        assert_eq!(buf[18], 0x01); // TCP command
        assert_eq!(u16::from_be_bytes([buf[19], buf[20]]), 443); // port
        assert_eq!(buf[21], 0x02); // domain type
        assert_eq!(buf[22], 11); // "example.com" length
        assert_eq!(&buf[23..34], b"example.com");
    }

    #[test]
    fn test_build_vless_request_ipv4() {
        let uuid = [0xAA; 16];
        let buf = build_vless_request(&uuid, "1.2.3.4", 80);

        assert_eq!(buf[21], 0x01); // IPv4 type
        assert_eq!(&buf[22..26], &[1, 2, 3, 4]); // IPv4 addr
    }

    #[test]
    fn test_build_vless_request_ipv6() {
        let uuid = [0xBB; 16];
        let buf = build_vless_request(&uuid, "::1", 8080);

        assert_eq!(buf[21], 0x03); // IPv6 type
        let expected_ipv6 = std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1).octets();
        assert_eq!(&buf[22..38], &expected_ipv6);
    }
}
