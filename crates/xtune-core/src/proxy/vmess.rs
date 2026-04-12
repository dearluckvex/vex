use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit as AesKeyInit};
use aes_gcm::Aes128Gcm;
use anyhow::{bail, Result};
use hmac::{Hmac, Mac};
use md5::{Digest as _, Md5};
use rand::Rng;
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::{TlsConfig, TransportConfig, TransportType};

use super::connector::{BoxProxyStream, Outbound, ProxyStream};
use super::transport::connect_with_tls;

type HmacSha256 = Hmac<Sha256>;

/// VMess AEAD security types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VMessSecurity {
    Aes128Gcm,
    Chacha20Poly1305,
    Auto,
    None,
    Zero,
}

impl VMessSecurity {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "aes-128-gcm" => Self::Aes128Gcm,
            "chacha20-poly1305" | "chacha20-ietf-poly1305" => Self::Chacha20Poly1305,
            "auto" => Self::Aes128Gcm,
            "none" => Self::None,
            "zero" => Self::Zero,
            _ => Self::Aes128Gcm,
        }
    }

    fn byte(&self) -> u8 {
        match self {
            Self::Aes128Gcm => 0x03,
            Self::Chacha20Poly1305 => 0x04,
            Self::Auto => 0x03,
            Self::None => 0x05,
            Self::Zero => 0x06,
        }
    }
}

/// VMess AEAD outbound connector.
pub struct VMessOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    security: VMessSecurity,
    tls_config: Option<TlsConfig>,
    use_tls: bool,
}

impl VMessOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        cipher: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        let parsed_uuid = uuid::Uuid::parse_str(uuid_str)?;

        let (tls_config, use_tls) = match transport {
            Some(t) => {
                let needs_tls = matches!(
                    t.transport_type,
                    TransportType::Tls | TransportType::Reality
                );
                (t.tls.clone(), needs_tls)
            }
            None => (None, false),
        };

        Ok(Self {
            server: server.to_string(),
            port,
            uuid: *parsed_uuid.as_bytes(),
            security: VMessSecurity::from_str(cipher),
            tls_config,
            use_tls,
        })
    }
}

impl Outbound for VMessOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let target_host = host.to_string();
        Box::pin(async move {
            let stream = connect_with_tls(
                &self.server,
                self.port,
                self.tls_config.as_ref(),
                self.use_tls,
            )
            .await?;

            // Generate session keys (in a block so rng drops before await)
            let (req_body_key, req_body_iv, resp_auth, header) = {
                let mut rng = rand::rng();
                let req_body_key: [u8; 16] = rng.random();
                let req_body_iv: [u8; 16] = rng.random();
                let resp_auth: u8 = rng.random();

                let header = build_vmess_header(
                    &self.uuid,
                    &req_body_key,
                    &req_body_iv,
                    resp_auth,
                    self.security,
                    &target_host,
                    port,
                )?;

                (req_body_key, req_body_iv, resp_auth, header)
            };

            // Derive response keys
            let resp_body_key_full = sha256_bytes(&req_body_key);
            let resp_body_iv_full = sha256_bytes(&req_body_iv);

            // Wrap in VMess AEAD stream
            let vmess_stream = VMessStream::new(
                stream,
                header,
                req_body_key,
                req_body_iv,
                resp_body_key_full[..16].try_into().unwrap(),
                resp_body_iv_full[..16].try_into().unwrap(),
                resp_auth,
                self.security,
            )
            .await?;

            Ok(Box::new(vmess_stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "vmess"
    }
}

/// Derive the VMess user command key from UUID using MD5.
fn vmess_cmd_key(uuid: &[u8; 16]) -> [u8; 16] {
    let magic = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";
    let mut hasher = Md5::new();
    hasher.update(uuid);
    hasher.update(magic);
    let result = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&result);
    key
}

/// Create authenticated length for AEAD header.
fn create_auth_id(cmd_key: &[u8; 16], timestamp: u64) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(cmd_key);
    hasher.update(timestamp.to_be_bytes());
    hasher.update(timestamp.to_be_bytes());
    hasher.update(timestamp.to_be_bytes());
    hasher.update(timestamp.to_be_bytes());
    let result = hasher.finalize();
    let mut auth_id = [0u8; 16];
    auth_id.copy_from_slice(&result);
    auth_id
}

/// KDF for VMess AEAD key derivation.
fn kdf(key: &[u8], paths: &[&[u8]]) -> Vec<u8> {
    const KDF_SALT: &[u8] = b"VMess AEAD KDF";
    let mut current =
        <HmacSha256 as Mac>::new_from_slice(KDF_SALT).expect("HMAC accepts any key length");
    current.update(key);

    for path in paths {
        let result = current.finalize().into_bytes();
        current =
            <HmacSha256 as Mac>::new_from_slice(&result).expect("HMAC accepts any key length");
        current.update(path);
    }

    current.finalize().into_bytes().to_vec()
}

/// Build the full VMess AEAD request header bytes.
fn build_vmess_header(
    uuid: &[u8; 16],
    body_key: &[u8; 16],
    body_iv: &[u8; 16],
    resp_auth: u8,
    security: VMessSecurity,
    host: &str,
    port: u16,
) -> Result<Vec<u8>> {
    let cmd_key = vmess_cmd_key(uuid);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let auth_id = create_auth_id(&cmd_key, timestamp);

    let mut rng = rand::rng();
    let nonce: [u8; 8] = rng.random();

    // Build the inner header (to be encrypted)
    let mut header = Vec::with_capacity(128);

    // Version
    header.push(1);
    // Body IV
    header.extend_from_slice(body_iv);
    // Body Key
    header.extend_from_slice(body_key);
    // Response Auth
    header.push(resp_auth);
    // Option: standard chunk stream (0x01) + chunk masking (0x04) + global padding (0x08)
    header.push(0x01 | 0x04);
    // Padding length (4 bits) + Security (4 bits)
    let padding_len: u8 = rng.random::<u8>() % 16;
    header.push((padding_len << 4) | security.byte());
    // Reserved
    header.push(0);
    // Command: TCP
    header.push(0x01);

    // Port (big-endian)
    header.extend_from_slice(&port.to_be_bytes());

    // Address
    if let Ok(ipv4) = host.parse::<std::net::Ipv4Addr>() {
        header.push(0x01); // IPv4
        header.extend_from_slice(&ipv4.octets());
    } else if let Ok(ipv6) = host.parse::<std::net::Ipv6Addr>() {
        header.push(0x03); // IPv6
        header.extend_from_slice(&ipv6.octets());
    } else {
        header.push(0x02); // Domain
        header.push(host.len() as u8);
        header.extend_from_slice(host.as_bytes());
    }

    // Random padding
    if padding_len > 0 {
        let padding: Vec<u8> = (0..padding_len).map(|_| rng.random()).collect();
        header.extend_from_slice(&padding);
    }

    // FNV1a hash of header for integrity
    let check = fnv1a32(&header);
    header.extend_from_slice(&check.to_be_bytes());

    // AEAD encrypt the header

    // Step 1: Derive header length encryption key and nonce
    let header_length_key_material = kdf(&cmd_key, &[b"VMess Header AEAD Key Length", &auth_id, &nonce]);
    let header_length_nonce_material = kdf(&cmd_key, &[b"VMess Header AEAD Nonce Length", &auth_id, &nonce]);

    let header_length_key: [u8; 16] = header_length_key_material[..16].try_into().unwrap();
    let header_length_nonce: [u8; 12] = header_length_nonce_material[..12].try_into().unwrap();

    let header_len = header.len() as u16;
    let cipher = Aes128Gcm::new_from_slice(&header_length_key)?;
    let encrypted_length = cipher
        .encrypt(
            (&header_length_nonce).into(),
            aes_gcm::aead::Payload {
                msg: &header_len.to_be_bytes(),
                aad: &auth_id,
            },
        )
        .map_err(|e| anyhow::anyhow!("AEAD encrypt length failed: {}", e))?;

    // Step 2: Derive header payload encryption key and nonce
    let header_key_material = kdf(&cmd_key, &[b"VMess Header AEAD Key", &auth_id, &nonce]);
    let header_nonce_material = kdf(&cmd_key, &[b"VMess Header AEAD Nonce", &auth_id, &nonce]);

    let header_key: [u8; 16] = header_key_material[..16].try_into().unwrap();
    let header_nonce: [u8; 12] = header_nonce_material[..12].try_into().unwrap();

    let cipher = Aes128Gcm::new_from_slice(&header_key)?;
    let encrypted_header = cipher
        .encrypt(
            (&header_nonce).into(),
            aes_gcm::aead::Payload {
                msg: &header,
                aad: &auth_id,
            },
        )
        .map_err(|e| anyhow::anyhow!("AEAD encrypt header failed: {}", e))?;

    // Assemble: auth_id(16) + encrypted_length(2+16) + nonce(8) + encrypted_header(N+16)
    let mut out = Vec::with_capacity(16 + 18 + 8 + encrypted_header.len());
    out.extend_from_slice(&auth_id);
    out.extend_from_slice(&encrypted_length);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&encrypted_header);

    Ok(out)
}

fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// VMess AEAD encrypted stream.
///
/// Uses a DuplexStream internally: two background tasks handle
/// encryption (write path) and decryption (read path) of chunks,
/// while the returned stream provides a clean AsyncRead + AsyncWrite.
struct VMessStream {
    stream: tokio::io::DuplexStream,
    _read_task: tokio::task::JoinHandle<()>,
    _write_task: tokio::task::JoinHandle<()>,
}

enum VMessChunkCipher {
    Aes128Gcm {
        cipher: Aes128Gcm,
        iv: [u8; 16],
    },
    Chacha20Poly1305 {
        cipher: chacha20poly1305::ChaCha20Poly1305,
        iv: [u8; 16],
    },
    None,
}

impl VMessChunkCipher {
    fn new(security: VMessSecurity, key: &[u8; 16], iv: &[u8; 16]) -> Self {
        match security {
            VMessSecurity::Aes128Gcm | VMessSecurity::Auto => {
                let cipher = Aes128Gcm::new_from_slice(key).unwrap();
                VMessChunkCipher::Aes128Gcm {
                    cipher,
                    iv: *iv,
                }
            }
            VMessSecurity::Chacha20Poly1305 => {
                use chacha20poly1305::KeyInit;
                let key32 = generate_chacha_key(key);
                let cipher =
                    chacha20poly1305::ChaCha20Poly1305::new_from_slice(&key32).unwrap();
                VMessChunkCipher::Chacha20Poly1305 {
                    cipher,
                    iv: *iv,
                }
            }
            VMessSecurity::None | VMessSecurity::Zero => VMessChunkCipher::None,
        }
    }

    fn encrypt(&self, count: u16, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            VMessChunkCipher::Aes128Gcm { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .encrypt((&nonce).into(), data)
                    .map_err(|e| anyhow::anyhow!("AES-GCM encrypt: {}", e))
            }
            VMessChunkCipher::Chacha20Poly1305 { cipher, iv } => {
                use chacha20poly1305::aead::Aead;
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .encrypt((&nonce).into(), data)
                    .map_err(|e| anyhow::anyhow!("ChaCha20 encrypt: {}", e))
            }
            VMessChunkCipher::None => Ok(data.to_vec()),
        }
    }

    fn decrypt(&self, count: u16, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            VMessChunkCipher::Aes128Gcm { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .decrypt((&nonce).into(), data)
                    .map_err(|e| anyhow::anyhow!("AES-GCM decrypt: {}", e))
            }
            VMessChunkCipher::Chacha20Poly1305 { cipher, iv } => {
                use chacha20poly1305::aead::Aead;
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .decrypt((&nonce).into(), data)
                    .map_err(|e| anyhow::anyhow!("ChaCha20 decrypt: {}", e))
            }
            VMessChunkCipher::None => Ok(data.to_vec()),
        }
    }

    fn overhead(&self) -> usize {
        match self {
            VMessChunkCipher::Aes128Gcm { .. } => 16,
            VMessChunkCipher::Chacha20Poly1305 { .. } => 16,
            VMessChunkCipher::None => 0,
        }
    }
}

// ChunkCipher is Send because both aes_gcm and chacha20poly1305 types are Send
unsafe impl Send for VMessChunkCipher {}

fn make_aead_nonce(iv: &[u8; 16], count: u16) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..2].copy_from_slice(&count.to_be_bytes());
    nonce[2..12].copy_from_slice(&iv[2..12]);
    nonce
}

fn generate_chacha_key(key: &[u8; 16]) -> [u8; 32] {
    let mut hasher1 = Md5::new();
    hasher1.update(key);
    let h1 = hasher1.finalize();

    let mut hasher2 = Md5::new();
    hasher2.update(&h1);
    let h2 = hasher2.finalize();

    let mut key32 = [0u8; 32];
    key32[..16].copy_from_slice(&h1);
    key32[16..].copy_from_slice(&h2);
    key32
}

impl VMessStream {
    async fn new(
        inner: BoxProxyStream,
        header: Vec<u8>,
        req_key: [u8; 16],
        req_iv: [u8; 16],
        resp_key: [u8; 16],
        resp_iv: [u8; 16],
        resp_auth: u8,
        security: VMessSecurity,
    ) -> Result<Self> {
        let (mut inner_read, mut inner_write) = tokio::io::split(inner);

        // Send header on the write half
        inner_write.write_all(&header).await?;

        // Create duplex streams for the caller
        let (caller_stream, our_stream) = tokio::io::duplex(65536);
        let (mut our_read, mut our_write) = tokio::io::split(our_stream);

        // Read task: reads encrypted chunks from server → decrypts → writes to caller
        let read_cipher = VMessChunkCipher::new(security, &resp_key, &resp_iv);
        let read_task = tokio::spawn(async move {
            // First, verify response header
            if let Err(e) = verify_response_header(&mut inner_read, resp_auth).await {
                tracing::error!("VMess response verification failed: {}", e);
                return;
            }

            let mut count: u16 = 0;
            loop {
                // Read chunk length
                let mut len_buf = [0u8; 2];
                match inner_read.read_exact(&mut len_buf).await {
                    Ok(_) => {}
                    Err(_) => break,
                }
                let chunk_len = u16::from_be_bytes(len_buf) as usize;
                if chunk_len == 0 {
                    break;
                }

                // Read encrypted chunk
                let mut encrypted = vec![0u8; chunk_len];
                if inner_read.read_exact(&mut encrypted).await.is_err() {
                    break;
                }

                // Decrypt
                match read_cipher.decrypt(count, &encrypted) {
                    Ok(decrypted) => {
                        if our_write.write_all(&decrypted).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("VMess decrypt error: {}", e);
                        break;
                    }
                }
                count = count.wrapping_add(1);
            }
        });

        // Write task: reads from caller → encrypts → writes chunks to server
        let write_cipher = VMessChunkCipher::new(security, &req_key, &req_iv);
        let write_task = tokio::spawn(async move {
            let max_chunk = 16384 - write_cipher.overhead();
            let mut count: u16 = 0;
            let mut buf = vec![0u8; max_chunk];

            loop {
                let n = match our_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };

                match write_cipher.encrypt(count, &buf[..n]) {
                    Ok(encrypted) => {
                        let chunk_len = encrypted.len() as u16;
                        if inner_write
                            .write_all(&chunk_len.to_be_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if inner_write.write_all(&encrypted).await.is_err() {
                            break;
                        }
                        if inner_write.flush().await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("VMess encrypt error: {}", e);
                        break;
                    }
                }
                count = count.wrapping_add(1);
            }
        });

        Ok(Self {
            stream: caller_stream,
            _read_task: read_task,
            _write_task: write_task,
        })
    }
}

/// Verify the VMess response header.
async fn verify_response_header(
    reader: &mut tokio::io::ReadHalf<BoxProxyStream>,
    expected_auth: u8,
) -> Result<()> {
    let resp_auth = reader.read_u8().await?;
    if resp_auth != expected_auth {
        bail!(
            "VMess response auth mismatch: expected {}, got {}",
            expected_auth,
            resp_auth
        );
    }
    let _opt = reader.read_u8().await?;
    let cmd_len = reader.read_u8().await?;
    if cmd_len > 0 {
        let mut cmd = vec![0u8; cmd_len as usize];
        reader.read_exact(&mut cmd).await?;
    }
    Ok(())
}

impl AsyncRead for VMessStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for VMessStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

impl Unpin for VMessStream {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vmess_cmd_key() {
        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811")
            .unwrap();
        let key = vmess_cmd_key(uuid.as_bytes());
        // Just verify it produces a deterministic 16-byte key
        assert_eq!(key.len(), 16);
        let key2 = vmess_cmd_key(uuid.as_bytes());
        assert_eq!(key, key2);
    }

    #[test]
    fn test_create_auth_id() {
        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811")
            .unwrap();
        let cmd_key = vmess_cmd_key(uuid.as_bytes());
        let auth_id = create_auth_id(&cmd_key, 1000000);
        assert_eq!(auth_id.len(), 16);
        // Same inputs should produce same output
        let auth_id2 = create_auth_id(&cmd_key, 1000000);
        assert_eq!(auth_id, auth_id2);
        // Different timestamp should produce different output
        let auth_id3 = create_auth_id(&cmd_key, 1000001);
        assert_ne!(auth_id, auth_id3);
    }

    #[test]
    fn test_kdf() {
        let key = b"test key material";
        let result = kdf(key, &[b"path1", b"path2"]);
        assert_eq!(result.len(), 32); // SHA256 output
        // Deterministic
        let result2 = kdf(key, &[b"path1", b"path2"]);
        assert_eq!(result, result2);
    }

    #[test]
    fn test_fnv1a32() {
        assert_eq!(fnv1a32(b""), 0x811c9dc5);
        assert_eq!(fnv1a32(b"hello"), 0x4f9f2cab);
    }

    #[test]
    fn test_build_header() {
        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811")
            .unwrap();
        let body_key = [1u8; 16];
        let body_iv = [2u8; 16];

        let header = build_vmess_header(
            uuid.as_bytes(),
            &body_key,
            &body_iv,
            0x42,
            VMessSecurity::Aes128Gcm,
            "example.com",
            443,
        )
        .unwrap();

        // Header should be: auth_id(16) + enc_length(18) + nonce(8) + enc_header(N+16)
        assert!(header.len() > 16 + 18 + 8);
    }

    #[test]
    fn test_chunk_cipher_roundtrip_aes() {
        let key = [0xAA; 16];
        let iv = [0xBB; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::Aes128Gcm, &key, &iv);

        let plaintext = b"Hello, VMess AEAD!";
        let encrypted = cipher.encrypt(0, plaintext).unwrap();
        let decrypted = cipher.decrypt(0, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_chunk_cipher_roundtrip_chacha() {
        let key = [0xCC; 16];
        let iv = [0xDD; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::Chacha20Poly1305, &key, &iv);

        let plaintext = b"Hello, ChaCha20!";
        let encrypted = cipher.encrypt(0, plaintext).unwrap();
        let decrypted = cipher.decrypt(0, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_chunk_cipher_none() {
        let key = [0; 16];
        let iv = [0; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::None, &key, &iv);

        let plaintext = b"No encryption";
        let encrypted = cipher.encrypt(0, plaintext).unwrap();
        assert_eq!(encrypted, plaintext);
    }

    #[test]
    fn test_make_aead_nonce() {
        let iv = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10];
        let nonce = make_aead_nonce(&iv, 42);
        assert_eq!(nonce[0], 0); // 42 >> 8
        assert_eq!(nonce[1], 42); // 42 & 0xFF
        assert_eq!(&nonce[2..], &iv[2..12]);
    }

    #[test]
    fn test_vmess_outbound_new() {
        let outbound = VMessOutbound::new(
            "server.com",
            443,
            "b831381d-6324-4d53-ad4f-8cda48b30811",
            "aes-128-gcm",
            None,
        )
        .unwrap();
        assert_eq!(outbound.name(), "vmess");
    }

    #[test]
    fn test_security_from_str() {
        assert_eq!(
            VMessSecurity::from_str("aes-128-gcm"),
            VMessSecurity::Aes128Gcm
        );
        assert_eq!(
            VMessSecurity::from_str("chacha20-poly1305"),
            VMessSecurity::Chacha20Poly1305
        );
        // "auto" resolves to Aes128Gcm at parse time
        assert_eq!(VMessSecurity::from_str("auto"), VMessSecurity::Aes128Gcm);
        assert_eq!(VMessSecurity::from_str("none"), VMessSecurity::None);
        assert_eq!(VMessSecurity::from_str("zero"), VMessSecurity::Zero);
        // Auto resolves to AES-128-GCM byte
        assert_eq!(VMessSecurity::Auto.byte(), VMessSecurity::Aes128Gcm.byte());
    }
}
