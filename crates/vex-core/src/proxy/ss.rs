use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use anyhow::{Result, bail};
use shadowsocks::ProxyClientStream;
use shadowsocks::config::{ServerAddr, ServerConfig, ServerType};
use shadowsocks::context::Context;
use shadowsocks::crypto::CipherKind;

use super::connector::{BoxProxyStream, Outbound};

/// Shadowsocks outbound connector using the shadowsocks crate.
pub struct SsOutbound {
    server_config: ServerConfig,
    context: shadowsocks::context::SharedContext,
}

impl SsOutbound {
    pub fn new(server: &str, port: u16, cipher: &str, password: &str) -> Result<Self> {
        let method = CipherKind::from_str(cipher)
            .map_err(|_| anyhow::anyhow!("Unknown SS cipher: {}", cipher))?;

        let server_addr: ServerAddr = (server.to_string(), port).into();
        let server_config = ServerConfig::new(server_addr, password, method)
            .map_err(|e| anyhow::anyhow!("Invalid SS server config: {}", e))?;

        let context = Context::new_shared(ServerType::Local);

        Ok(Self {
            server_config,
            context,
        })
    }
}

impl Outbound for SsOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let addr = shadowsocks::relay::Address::DomainNameAddress(host.to_string(), port);
        Box::pin(async move {
            let stream =
                ProxyClientStream::connect(self.context.clone(), &self.server_config, addr).await?;

            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "shadowsocks"
    }
}

/// Map a common cipher name (from Clash/V2Ray configs) to shadowsocks-crypto format.
pub fn normalize_ss_cipher(cipher: &str) -> &str {
    match cipher {
        "aes-128-gcm" => "aes-128-gcm",
        "aes-256-gcm" => "aes-256-gcm",
        "chacha20-ietf-poly1305" | "chacha20-poly1305" => "chacha20-ietf-poly1305",
        "2022-blake3-aes-128-gcm" => "2022-blake3-aes-128-gcm",
        "2022-blake3-aes-256-gcm" => "2022-blake3-aes-256-gcm",
        "2022-blake3-chacha20-poly1305" => "2022-blake3-chacha20-poly1305",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ss_outbound_creation() {
        let outbound = SsOutbound::new("127.0.0.1", 8388, "aes-256-gcm", "password123");
        assert!(outbound.is_ok());
        assert_eq!(outbound.unwrap().name(), "shadowsocks");
    }

    #[test]
    fn test_ss_outbound_aead_2022() {
        // AEAD 2022 ciphers require base64-encoded keys of specific length
        let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &[0u8; 32]);
        let outbound = SsOutbound::new("127.0.0.1", 8388, "2022-blake3-aes-256-gcm", &key);
        assert!(outbound.is_ok());
    }

    #[test]
    fn test_ss_outbound_invalid_cipher() {
        let outbound = SsOutbound::new("127.0.0.1", 8388, "invalid-cipher", "password123");
        assert!(outbound.is_err());
    }

    #[test]
    fn test_normalize_cipher() {
        assert_eq!(
            normalize_ss_cipher("chacha20-poly1305"),
            "chacha20-ietf-poly1305"
        );
        assert_eq!(normalize_ss_cipher("aes-256-gcm"), "aes-256-gcm");
    }
}
