use std::sync::Arc;

use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::config::model::TlsConfig;

/// Connect to a remote server with TLS.
pub async fn connect_tls(
    tcp: TcpStream,
    sni: &str,
    config: Option<&TlsConfig>,
) -> Result<TlsStream<TcpStream>> {
    let skip_verify = config.map(|c| c.skip_cert_verify).unwrap_or(false);
    let alpn = config.and_then(|c| c.alpn.as_ref());

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let mut tls_config = if skip_verify {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    if let Some(alpn) = alpn {
        tls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = ServerName::try_from(sni.to_string())?;
    let stream = connector.connect(server_name, tcp).await?;
    Ok(stream)
}

/// Connect to a remote server over TCP, optionally wrapping with TLS.
/// Returns a boxed ProxyStream.
pub async fn connect_with_tls(
    server: &str,
    port: u16,
    tls_config: Option<&TlsConfig>,
    use_tls: bool,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    let tcp = TcpStream::connect(format!("{}:{}", server, port)).await?;
    if use_tls {
        let sni = tls_config.and_then(|c| c.sni.as_deref()).unwrap_or(server);
        let tls = connect_tls(tcp, sni, tls_config).await?;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

/// Certificate verifier that accepts all certificates (for skip_cert_verify).
#[derive(Debug)]
struct InsecureVerifier;

impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}
