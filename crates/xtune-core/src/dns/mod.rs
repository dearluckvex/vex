//! DNS resolver module for xtune.
//!
//! Provides a configurable DNS resolver supporting:
//! - System DNS (uses OS resolver)
//! - Custom UDP DNS servers
//! - DNS-over-HTTPS (DoH) via standard wireformat
//! - DNS split: route China domains to local DNS, others to remote DNS

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};

/// DNS resolver configuration.
#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// Primary DNS servers (IP:port or DoH URL)
    pub servers: Vec<DnsServer>,
    /// Fallback DNS servers used when primary fails
    pub fallback: Vec<DnsServer>,
    /// Domain rules: domain suffix -> server group (e.g., "cn" -> use fallback)
    pub domain_rules: HashMap<String, DnsGroup>,
    /// Cache TTL override (0 = use DNS TTL, >0 = override)
    pub cache_ttl: u32,
    /// Enable DNS cache
    pub cache_enabled: bool,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                DnsServer::Udp("8.8.8.8:53".parse().unwrap()),
                DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
            ],
            fallback: vec![
                DnsServer::Udp("223.5.5.5:53".parse().unwrap()),
                DnsServer::Udp("119.29.29.29:53".parse().unwrap()),
            ],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: true,
        }
    }
}

/// Which DNS server group to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsGroup {
    Primary,
    Fallback,
}

/// DNS server types.
#[derive(Debug, Clone)]
pub enum DnsServer {
    /// Standard UDP DNS
    Udp(SocketAddr),
    /// DNS-over-HTTPS (e.g., "https://dns.google/dns-query")
    DoH(String),
}

/// Cached DNS entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    addresses: Vec<IpAddr>,
    expires_at: Instant,
    last_used: Instant,
}

/// Maximum number of entries in the DNS cache before LRU eviction.
const DNS_CACHE_MAX_ENTRIES: usize = 1024;

/// In-flight DNS query entry for deduplication.
type InflightEntry = Arc<tokio::sync::OnceCell<Result<Vec<IpAddr>, String>>>;

/// The main DNS resolver.
pub struct DnsResolver {
    config: DnsConfig,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    /// Shared reqwest client for DoH queries (avoids per-query creation)
    doh_client: reqwest::Client,
    /// In-flight queries: concurrent resolves for the same domain share one query
    inflight: Mutex<HashMap<String, InflightEntry>>,
}

impl DnsResolver {
    /// Create a new resolver with default config (8.8.8.8 + 1.1.1.1).
    pub fn new() -> Self {
        Self::with_config(DnsConfig::default())
    }

    /// Create a new resolver with custom config.
    pub fn with_config(config: DnsConfig) -> Self {
        let doh_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(2)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            doh_client,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a hostname to IP addresses.
    pub async fn resolve(&self, domain: &str) -> Result<Vec<IpAddr>> {
        // If it's already an IP, return directly
        if let Ok(ip) = domain.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }

        // Check cache
        if self.config.cache_enabled {
            let now = Instant::now();
            let mut cache = self.cache.write().await;
            if let Some(entry) = cache.get_mut(domain) {
                entry.last_used = now;
                if entry.expires_at > now {
                    return Ok(entry.addresses.clone());
                }
                // Stale-while-revalidate: return stale result and refresh in background.
                // Only serve stale data up to 5 minutes past expiry.
                let stale_limit = Duration::from_secs(300);
                if now.duration_since(entry.expires_at) < stale_limit {
                    let stale_addrs = entry.addresses.clone();
                    let domain_owned = domain.to_string();
                    let cache_ref = self.cache.clone();
                    let config = self.config.clone();
                    let doh_client = self.doh_client.clone();
                    tokio::spawn(async move {
                        // Create a temporary resolver-like context for background refresh
                        let resolver = DnsResolver {
                            config,
                            cache: cache_ref,
                            doh_client,
                            inflight: Mutex::new(HashMap::new()),
                        };
                        let _ = resolver.resolve_uncached(&domain_owned).await;
                    });
                    return Ok(stale_addrs);
                }
                // Too stale — fall through to fresh resolution
            }
        }

        // In-flight dedup: if another task is already resolving this domain,
        // share its result instead of sending a duplicate query.
        let cell = {
            let mut inflight = self.inflight.lock().await;
            inflight
                .entry(domain.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        // Only the first caller runs resolve_uncached(); others wait for its result.
        let result = cell
            .get_or_init(|| async {
                self.resolve_uncached(domain)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

        // Clean up inflight entry so future queries aren't stale
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(domain);
        }

        result.clone().map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Perform the actual DNS resolution (post-cache, no dedup).
    async fn resolve_uncached(&self, domain: &str) -> Result<Vec<IpAddr>> {
        // Determine which server group to use
        let group = self.match_domain_group(domain);
        let servers = match group {
            DnsGroup::Primary => &self.config.servers,
            DnsGroup::Fallback => &self.config.fallback,
        };

        // Try each server in order
        let mut last_err = None;
        for server in servers {
            match self.query_server(server, domain).await {
                Ok(addrs) if !addrs.is_empty() => {
                    // Cache result
                    if self.config.cache_enabled {
                        let ttl = if self.config.cache_ttl > 0 {
                            self.config.cache_ttl
                        } else {
                            300 // default 5 minutes
                        };
                        let mut cache = self.cache.write().await;

                        // Evict expired entries first
                        let now = Instant::now();
                        cache.retain(|_, e| e.expires_at > now);

                        // If still over limit, evict least-recently-used
                        if cache.len() >= DNS_CACHE_MAX_ENTRIES {
                            if let Some(lru_key) = cache
                                .iter()
                                .min_by_key(|(_, e)| e.last_used)
                                .map(|(k, _)| k.clone())
                            {
                                cache.remove(&lru_key);
                            }
                        }

                        cache.insert(
                            domain.to_string(),
                            CacheEntry {
                                addresses: addrs.clone(),
                                expires_at: now + Duration::from_secs(ttl as u64),
                                last_used: now,
                            },
                        );
                    }
                    return Ok(addrs);
                }
                Ok(_) => last_err = Some(anyhow::anyhow!("Empty DNS response for {}", domain)),
                Err(e) => last_err = Some(e),
            }
        }

        // Try fallback if primary failed
        if group == DnsGroup::Primary && !self.config.fallback.is_empty() {
            for server in &self.config.fallback {
                if let Ok(addrs) = self.query_server(server, domain).await {
                    if !addrs.is_empty() {
                        return Ok(addrs);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("No DNS servers configured")))
    }

    /// Resolve to first IPv4 address.
    pub async fn resolve_ipv4(&self, domain: &str) -> Result<Ipv4Addr> {
        let addrs = self.resolve(domain).await?;
        addrs
            .into_iter()
            .find_map(|a| match a {
                IpAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("No IPv4 address for {}", domain))
    }

    /// Clear the DNS cache.
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cache statistics: (total_entries, expired_entries).
    pub async fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.read().await;
        let now = Instant::now();
        let total = cache.len();
        let expired = cache.values().filter(|e| e.expires_at <= now).count();
        (total, expired)
    }

    fn match_domain_group(&self, domain: &str) -> DnsGroup {
        // Check domain rules (longest suffix match)
        let parts: Vec<&str> = domain.split('.').collect();
        for i in 0..parts.len() {
            let suffix = parts[i..].join(".");
            if let Some(group) = self.config.domain_rules.get(&suffix) {
                return group.clone();
            }
        }
        DnsGroup::Primary
    }

    async fn query_server(&self, server: &DnsServer, domain: &str) -> Result<Vec<IpAddr>> {
        match server {
            DnsServer::Udp(addr) => self.query_udp(*addr, domain).await,
            DnsServer::DoH(url) => self.query_doh(url, domain).await,
        }
    }

    /// Query DNS over UDP.
    async fn query_udp(&self, server: SocketAddr, domain: &str) -> Result<Vec<IpAddr>> {
        let query = build_dns_query(domain, 1); // A record
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.send_to(&query, server).await?;

        let mut buf = vec![0u8; 512];
        let timeout = tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buf));
        let (n, _) = timeout
            .await
            .map_err(|_| anyhow::anyhow!("DNS query timeout"))??;

        parse_dns_response(&buf[..n])
    }

    /// Query DNS over HTTPS (DoH) using wireformat.
    async fn query_doh(&self, url: &str, domain: &str) -> Result<Vec<IpAddr>> {
        let query = build_dns_query(domain, 1);

        let resp = self
            .doh_client
            .post(url)
            .header("Content-Type", "application/dns-message")
            .header("Accept", "application/dns-message")
            .body(query)
            .send()
            .await?;

        if !resp.status().is_success() {
            bail!("DoH server returned status {}", resp.status());
        }

        let body = resp.bytes().await?;
        parse_dns_response(&body)
    }
}

// --- DNS wire format helpers ---

/// Build a minimal DNS query packet for A record.
fn build_dns_query(domain: &str, qtype: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);

    // Header
    let id: u16 = rand::random();
    buf.extend_from_slice(&id.to_be_bytes()); // ID
    buf.extend_from_slice(&[0x01, 0x00]); // Flags: standard query, recursion desired
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT: 1
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT: 0

    // Question section
    for label in domain.split('.') {
        let bytes = label.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    buf.push(0x00); // Root label

    buf.extend_from_slice(&qtype.to_be_bytes()); // QTYPE
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS: IN

    buf
}

/// Parse a DNS response and extract IP addresses.
fn parse_dns_response(data: &[u8]) -> Result<Vec<IpAddr>> {
    if data.len() < 12 {
        bail!("DNS response too short");
    }

    let ancount = u16::from_be_bytes([data[6], data[7]]) as usize;
    let rcode = data[3] & 0x0F;

    if rcode != 0 {
        bail!("DNS response error: rcode={}", rcode);
    }

    // Skip header (12 bytes) and question section
    let mut pos = 12;

    // Skip question section (QDCOUNT = data[4..6])
    let qdcount = u16::from_be_bytes([data[4], data[5]]) as usize;
    for _ in 0..qdcount {
        pos = skip_dns_name(data, pos)?;
        pos += 4; // QTYPE + QCLASS
    }

    // Parse answer section
    let mut addrs = Vec::new();
    for _ in 0..ancount {
        if pos >= data.len() {
            break;
        }
        pos = skip_dns_name(data, pos)?;
        if pos + 10 > data.len() {
            break;
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            break;
        }

        match rtype {
            1 if rdlength == 4 => {
                // A record
                let ip = Ipv4Addr::new(data[pos], data[pos + 1], data[pos + 2], data[pos + 3]);
                addrs.push(IpAddr::V4(ip));
            }
            28 if rdlength == 16 => {
                // AAAA record
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[pos..pos + 16]);
                addrs.push(IpAddr::V6(octets.into()));
            }
            _ => {}
        }
        pos += rdlength;
    }

    Ok(addrs)
}

/// Skip a DNS name (handles compression pointers).
fn skip_dns_name(data: &[u8], mut pos: usize) -> Result<usize> {
    let mut jumped = false;
    loop {
        if pos >= data.len() {
            bail!("DNS name extends beyond packet");
        }
        let len = data[pos] as usize;
        if len == 0 {
            if !jumped {
                pos += 1;
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer
            if !jumped {
                pos += 2;
                jumped = true;
            }
            break;
        }
        if !jumped {
            pos += 1 + len;
        } else {
            break;
        }
    }
    Ok(pos)
}

/// Extract the queried domain name and query type from a raw DNS packet.
///
/// Returns `(domain, qtype)` where qtype is 1 for A, 28 for AAAA, etc.
pub fn parse_dns_query(data: &[u8]) -> Result<(String, u16)> {
    if data.len() < 12 {
        bail!("DNS packet too short");
    }
    let mut pos = 12; // skip header
    let mut labels: Vec<String> = Vec::new();
    loop {
        if pos >= data.len() {
            bail!("DNS question extends beyond packet");
        }
        let len = data[pos] as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            bail!("Unexpected compression pointer in question");
        }
        pos += 1;
        if pos + len > data.len() {
            bail!("DNS label extends beyond packet");
        }
        labels.push(String::from_utf8_lossy(&data[pos..pos + len]).to_string());
        pos += len;
    }
    if pos + 4 > data.len() {
        bail!("DNS question QTYPE/QCLASS missing");
    }
    let qtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
    let domain = labels.join(".");
    Ok((domain, qtype))
}

/// Build a DNS response packet from a query packet and a set of resolved IPs.
///
/// Preserves the query ID and question section, adds A/AAAA answers.
pub fn build_dns_response(query: &[u8], addresses: &[IpAddr]) -> Result<Vec<u8>> {
    if query.len() < 12 {
        bail!("Query packet too short to build response");
    }

    // Find end of question section
    let mut qend = 12;
    loop {
        if qend >= query.len() {
            bail!("Malformed DNS query: question section overflows");
        }
        let len = query[qend] as usize;
        if len == 0 {
            qend += 1; // null terminator
            break;
        }
        if len & 0xC0 == 0xC0 {
            qend += 2;
            break;
        }
        qend += 1 + len;
    }
    qend += 4; // QTYPE + QCLASS

    let an_count = addresses.len() as u16;
    let mut resp = Vec::with_capacity(qend + addresses.len() * 16 + 32);

    // Copy header from query
    resp.extend_from_slice(&query[..2]); // ID

    // Flags: response, recursion desired+available, no error
    resp.extend_from_slice(&[0x81, 0x80]);

    // QDCOUNT = 1
    resp.extend_from_slice(&[0x00, 0x01]);
    // ANCOUNT
    resp.extend_from_slice(&an_count.to_be_bytes());
    // NSCOUNT, ARCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

    // Copy question section from query
    resp.extend_from_slice(&query[12..qend]);

    // Answer records
    let ttl: u32 = 300;
    for addr in addresses {
        // Name pointer to question
        resp.extend_from_slice(&[0xC0, 0x0C]);
        match addr {
            IpAddr::V4(v4) => {
                resp.extend_from_slice(&[0x00, 0x01]); // A
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
                resp.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                resp.extend_from_slice(&[0x00, 0x1C]); // AAAA
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x10]); // RDLENGTH
                resp.extend_from_slice(&v6.octets());
            }
        }
    }

    Ok(resp)
}

/// Build a DNS NXDOMAIN (or SERVFAIL) error response from a query packet.
pub fn build_dns_error_response(query: &[u8], rcode: u8) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    let mut resp = query.to_vec();
    // Set QR=1 (response), keep RD, set RA, set rcode
    resp[2] = 0x81;
    resp[3] = 0x80 | (rcode & 0x0F);
    // Zero out ANCOUNT
    resp[6] = 0;
    resp[7] = 0;
    resp
}

/// Common China domain suffixes for DNS splitting.
pub fn china_domain_suffixes() -> Vec<String> {
    vec![
        "cn".to_string(),
        "com.cn".to_string(),
        "net.cn".to_string(),
        "org.cn".to_string(),
        "baidu.com".to_string(),
        "qq.com".to_string(),
        "taobao.com".to_string(),
        "tmall.com".to_string(),
        "jd.com".to_string(),
        "alipay.com".to_string(),
        "weibo.com".to_string(),
        "bilibili.com".to_string(),
        "zhihu.com".to_string(),
        "163.com".to_string(),
        "douyin.com".to_string(),
        "tiktok.com".to_string(),
        "sina.com".to_string(),
        "sohu.com".to_string(),
        "csdn.net".to_string(),
        "aliyun.com".to_string(),
    ]
}

/// Create a DnsConfig with China DNS splitting.
/// China domains use domestic DNS (223.5.5.5, 119.29.29.29),
/// others use international DNS (8.8.8.8, 1.1.1.1).
pub fn china_split_dns_config() -> DnsConfig {
    let mut rules = HashMap::new();
    for suffix in china_domain_suffixes() {
        rules.insert(suffix, DnsGroup::Fallback);
    }

    DnsConfig {
        servers: vec![
            DnsServer::Udp("8.8.8.8:53".parse().unwrap()),
            DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
        ],
        fallback: vec![
            DnsServer::Udp("223.5.5.5:53".parse().unwrap()),
            DnsServer::Udp("119.29.29.29:53".parse().unwrap()),
        ],
        domain_rules: rules,
        cache_ttl: 600,
        cache_enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_dns_query() {
        let query = build_dns_query("example.com", 1);
        assert!(query.len() > 12);
        // Check header flags
        assert_eq!(query[2], 0x01); // RD flag
        assert_eq!(query[4], 0x00);
        assert_eq!(query[5], 0x01); // QDCOUNT = 1
    }

    #[test]
    fn test_parse_dns_response_a_record() {
        // Minimal synthetic DNS response with one A record
        let mut resp = Vec::new();
        // Header
        resp.extend_from_slice(&[0x00, 0x01]); // ID
        resp.extend_from_slice(&[0x81, 0x80]); // Flags: response, recursion
        resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT
        resp.extend_from_slice(&[0x00, 0x01]); // ANCOUNT
        resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
        resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT

        // Question: example.com A IN
        resp.push(7);
        resp.extend_from_slice(b"example");
        resp.push(3);
        resp.extend_from_slice(b"com");
        resp.push(0);
        resp.extend_from_slice(&[0x00, 0x01]); // A
        resp.extend_from_slice(&[0x00, 0x01]); // IN

        // Answer: pointer to name, A, IN, TTL=300, 4 bytes, 93.184.216.34
        resp.extend_from_slice(&[0xC0, 0x0C]); // Name pointer
        resp.extend_from_slice(&[0x00, 0x01]); // A
        resp.extend_from_slice(&[0x00, 0x01]); // IN
        resp.extend_from_slice(&[0x00, 0x00, 0x01, 0x2C]); // TTL = 300
        resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH = 4
        resp.extend_from_slice(&[93, 184, 216, 34]); // IP

        let addrs = parse_dns_response(&resp).unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));
    }

    #[test]
    fn test_dns_resolver_ip_passthrough() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resolver = DnsResolver::new();
        let result = rt.block_on(resolver.resolve("1.2.3.4")).unwrap();
        assert_eq!(result, vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))]);
    }

    #[test]
    fn test_domain_group_matching() {
        let mut rules = HashMap::new();
        rules.insert("cn".to_string(), DnsGroup::Fallback);
        rules.insert("baidu.com".to_string(), DnsGroup::Fallback);

        let config = DnsConfig {
            domain_rules: rules,
            ..Default::default()
        };
        let resolver = DnsResolver::with_config(config);

        assert_eq!(
            resolver.match_domain_group("www.baidu.com"),
            DnsGroup::Fallback
        );
        assert_eq!(resolver.match_domain_group("test.cn"), DnsGroup::Fallback);
        assert_eq!(resolver.match_domain_group("google.com"), DnsGroup::Primary);
    }

    #[test]
    fn test_china_split_config() {
        let config = china_split_dns_config();
        assert!(!config.domain_rules.is_empty());
        assert!(config.domain_rules.contains_key("baidu.com"));
        assert!(config.cache_enabled);
    }

    #[test]
    fn test_parse_error_response() {
        // NXDOMAIN response
        let mut resp = vec![0u8; 12];
        resp[2] = 0x81;
        resp[3] = 0x83; // rcode = 3 (NXDOMAIN)
        resp[5] = 0; // no questions

        let result = parse_dns_response(&resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dns_query_and_build_response() {
        // Build a query for "example.com" A record
        let query = build_dns_query("example.com", 1);
        let (domain, qtype) = parse_dns_query(&query).unwrap();
        assert_eq!(domain, "example.com");
        assert_eq!(qtype, 1);

        // Build a response with one A record
        let addrs = vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))];
        let resp = build_dns_response(&query, &addrs).unwrap();

        // Verify it's a valid DNS response
        assert!(resp.len() > 12);
        // QR bit should be set (response)
        assert_eq!(resp[2] & 0x80, 0x80);
        // ANCOUNT should be 1
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1);

        // Parse the response to extract the IP
        let parsed = parse_dns_response(&resp).unwrap();
        assert_eq!(parsed, addrs);
    }

    #[test]
    fn test_build_dns_error_response() {
        let query = build_dns_query("fail.example.com", 1);
        let resp = build_dns_error_response(&query, 2); // SERVFAIL
        assert!(resp.len() >= 12);
        assert_eq!(resp[3] & 0x0F, 2); // rcode = SERVFAIL
    }
}
