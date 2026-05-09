//! Shared QUIC/TLS infrastructure for TUIC and Hysteria2.
//!
//! Centralises the common QUIC connection management, TLS configuration,
//! and DNS resolution logic shared across QUIC-based proxy protocols.

use std::net::SocketAddr;
use std::sync::{Arc, Once};

use anyhow::Result;

/// Cached QUIC connection state with double-checked-locking reconnect logic.
pub struct QuicConnectionState {
    state: tokio::sync::RwLock<Option<(quinn::Endpoint, quinn::Connection)>>,
}

impl QuicConnectionState {
    pub fn new() -> Self {
        Self {
            state: tokio::sync::RwLock::new(None),
        }
    }

    /// Fast path: returns a live connection if cached, else `None`.
    pub async fn get_existing(&self) -> Option<quinn::Connection> {
        let guard = self.state.read().await;
        if let Some((_, ref conn)) = *guard
            && conn.close_reason().is_none()
        {
            return Some(conn.clone());
        }
        None
    }

    /// Slow path: store a newly created and authenticated connection under write lock.
    ///
    /// Implements double-check: if another task stored a live connection while we
    /// were creating ours, that connection is returned and our arguments are dropped.
    pub async fn store_if_dead(
        &self,
        new_endpoint: quinn::Endpoint,
        new_conn: quinn::Connection,
    ) -> quinn::Connection {
        let mut guard = self.state.write().await;
        if let Some((_, ref conn)) = *guard
            && conn.close_reason().is_none()
        {
            return conn.clone();
        }
        *guard = Some((new_endpoint, new_conn.clone()));
        new_conn
    }
}

impl Default for QuicConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Ensure the rustls ring crypto provider is installed (idempotent).
pub fn ensure_quic_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Globally cached root cert store for QUIC TLS (built once).
static QUIC_ROOT_CERTS: std::sync::OnceLock<rustls::RootCertStore> = std::sync::OnceLock::new();

pub fn quic_root_certs() -> &'static rustls::RootCertStore {
    QUIC_ROOT_CERTS.get_or_init(|| {
        let mut store = rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        store
    })
}

/// Build a `quinn::ClientConfig` with TLS settings (reusable across reconnections).
pub fn build_quic_client_config(
    skip_cert_verify: bool,
    alpn: &[String],
) -> Result<quinn::ClientConfig> {
    let mut rustls_config = if skip_cert_verify {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureQuicVerifier))
            .with_no_client_auth()
    } else {
        rustls::ClientConfig::builder()
            .with_root_certificates(quic_root_certs().clone())
            .with_no_client_auth()
    };

    if !alpn.is_empty() {
        rustls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
        .map_err(|e| anyhow::anyhow!("QUIC TLS config error: {}", e))?;

    Ok(quinn::ClientConfig::new(Arc::new(quic_config)))
}

pub(crate) fn prefer_socket_addrs(mut addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
    addrs.dedup();
    addrs
}

/// Resolve server hostname to `SocketAddr` candidates, preferring IPv4 first.
pub async fn resolve_server_addrs(server: &str, port: u16) -> Result<Vec<SocketAddr>> {
    use tokio::net::lookup_host;
    let addrs: Vec<_> = lookup_host(format!("{}:{}", server, port)).await?.collect();
    let addrs = prefer_socket_addrs(addrs);
    if addrs.is_empty() {
        anyhow::bail!("Failed to resolve {}", server);
    }
    Ok(addrs)
}

/// Insecure certificate verifier for the `skip_cert_verify` option.
#[derive(Debug)]
pub struct InsecureQuicVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureQuicVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefer_socket_addrs_prefers_ipv4_and_dedups() {
        let ordered = prefer_socket_addrs(vec![
            "[2001:db8::1]:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "[2001:db8::2]:443".parse().unwrap(),
        ]);

        assert_eq!(ordered.len(), 3);
        assert!(ordered[0].is_ipv4());
        assert!(ordered[1].is_ipv6());
        assert!(ordered[2].is_ipv6());
    }
}
