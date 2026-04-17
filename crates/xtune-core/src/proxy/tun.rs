use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use etherparse::Icmpv4Header;
use ipstack::{IpNumber, IpStackConfig, IpStackStream, TcpConfig, TcpOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tun::AbstractDevice;

use super::connector::SharedOutbound;
use super::relay::relay_bidirectional;
use crate::dns::DnsResolver;

/// TUN device IP configuration.
const TUN_IPV4: Ipv4Addr = Ipv4Addr::new(10, 10, 0, 2);
const TUN_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 10, 0, 1);
const TUN_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const TUN_MTU: u16 = 1500;

/// TUN-based transparent proxy.
///
/// Creates a TUN device, intercepts all TCP/UDP traffic via the `ipstack`
/// userspace TCP/IP stack, and forwards connections through the proxy outbound.
pub struct TunProxy {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    tun_name: String,
}

/// Information needed to set up system routes for TUN mode.
#[derive(Debug, Clone)]
pub struct TunRouteInfo {
    pub tun_name: String,
    pub tun_gateway: Ipv4Addr,
}

impl TunProxy {
    /// Start the TUN proxy.
    ///
    /// Creates a TUN device, configures `ipstack`, and spawns the accept loop.
    /// **Requires root/admin privileges.**
    ///
    /// After calling `start()`, the caller should set up system routes via
    /// [`setup_tun_routes`] to direct traffic through the TUN device.
    pub fn start(outbound: SharedOutbound) -> Result<Self> {
        validate_tun_environment()?;

        let mut tun_config = tun::Configuration::default();
        tun_config
            .address(TUN_IPV4)
            .netmask(TUN_NETMASK)
            .destination(TUN_GATEWAY)
            .mtu(TUN_MTU)
            .up();

        #[cfg(target_os = "linux")]
        tun_config.platform_config(|p_cfg| {
            p_cfg.ensure_root_privileges(true);
        });

        let tun_dev =
            tun::create_as_async(&tun_config).with_context(tun_creation_failure_message)?;

        let tun_name = tun_dev
            .tun_name()
            .unwrap_or_else(|_| default_tun_name().to_string());

        tracing::info!("TUN device created: {} ({})", tun_name, TUN_IPV4);

        let mut ipstack_config = IpStackConfig::default();
        ipstack_config.mtu(TUN_MTU).expect("valid MTU");

        let mut tcp_config = TcpConfig::default();
        tcp_config.timeout = std::time::Duration::from_secs(300);
        tcp_config.options = Some(vec![TcpOptions::MaximumSegmentSize(
            (TUN_MTU - 40) as u16, // IP header (20) + TCP header (20)
        )]);
        tcp_config.max_unacked_bytes = 256 * 1024;
        ipstack_config.with_tcp_config(tcp_config);
        ipstack_config.udp_timeout(std::time::Duration::from_secs(30));

        let mut ip_stack = ipstack::IpStack::new(ipstack_config, tun_dev);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = tokio::spawn(async move {
            // Create DNS resolver for intercepting TUN DNS queries
            let dns_resolver = Arc::new(DnsResolver::with_config(
                crate::dns::china_split_dns_config(),
            ));

            tracing::info!("TUN accept loop started (with DNS interception)");
            while running_clone.load(Ordering::Relaxed) {
                match ip_stack.accept().await {
                    Ok(stream) => {
                        let outbound = outbound.clone();
                        let resolver = dns_resolver.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_tun_stream(stream, outbound, resolver).await {
                                tracing::debug!("TUN stream error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        if running_clone.load(Ordering::Relaxed) {
                            tracing::warn!("TUN accept error: {}", e);
                        }
                        break;
                    }
                }
            }
            tracing::info!("TUN accept loop exited");
        });

        Ok(TunProxy {
            running,
            handle: Some(handle),
            tun_name,
        })
    }

    /// Get the TUN device name.
    pub fn tun_name(&self) -> &str {
        &self.tun_name
    }

    /// Get route info for setting up system routes.
    pub fn route_info(&self) -> TunRouteInfo {
        TunRouteInfo {
            tun_name: self.tun_name.clone(),
            tun_gateway: TUN_GATEWAY,
        }
    }

    /// Stop the TUN proxy and clean up.
    pub async fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.abort();
            let _ = h.await;
        }
        tracing::info!("TUN proxy stopped");
    }
}

impl Drop for TunProxy {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Handle a single TUN stream (TCP, UDP, or ICMP).
async fn handle_tun_stream(
    stream: IpStackStream,
    outbound: SharedOutbound,
    dns_resolver: Arc<DnsResolver>,
) -> Result<()> {
    match stream {
        IpStackStream::Tcp(mut tcp) => {
            let dst = tcp.peer_addr();
            let (host, port) = addr_to_host_port(&dst);
            tracing::debug!("TUN TCP: -> {}:{}", host, port);

            match outbound.connect(&host, port).await {
                Ok(mut remote) => {
                    let result = relay_bidirectional(&mut tcp, &mut remote).await;
                    let _ = remote.shutdown().await;
                    let _ = tcp.shutdown().await;
                    if let Err(e) = result {
                        tracing::debug!("TUN TCP relay error {}:{}: {}", host, port, e);
                    }
                }
                Err(e) => {
                    tracing::debug!("TUN TCP connect failed {}:{}: {}", host, port, e);
                    let _ = tcp.shutdown().await;
                }
            }
        }
        IpStackStream::Udp(mut udp) => {
            let dst = udp.peer_addr();
            let (host, port) = addr_to_host_port(&dst);

            // Intercept DNS queries (UDP port 53) and resolve locally
            if port == 53 {
                handle_tun_dns(&mut udp, &dns_resolver).await?;
                return Ok(());
            }

            tracing::debug!("TUN UDP: -> {}:{}", host, port);

            match outbound.connect(&host, port).await {
                Ok(mut remote) => {
                    let result = relay_bidirectional(&mut udp, &mut remote).await;
                    let _ = remote.shutdown().await;
                    let _ = udp.shutdown().await;
                    if let Err(e) = result {
                        tracing::debug!("TUN UDP relay error {}:{}: {}", host, port, e);
                    }
                }
                Err(e) => {
                    tracing::debug!("TUN UDP connect failed {}:{}: {}", host, port, e);
                    let _ = udp.shutdown().await;
                }
            }
        }
        IpStackStream::UnknownTransport(pkt) => {
            if pkt.src_addr().is_ipv4() && pkt.ip_protocol() == IpNumber::ICMP {
                if let Ok((icmp_header, req_payload)) = Icmpv4Header::from_slice(pkt.payload()) {
                    if let etherparse::Icmpv4Type::EchoRequest(echo) = icmp_header.icmp_type {
                        let mut resp = Icmpv4Header::new(etherparse::Icmpv4Type::EchoReply(echo));
                        resp.update_checksum(req_payload);
                        let mut payload = resp.to_bytes().to_vec();
                        payload.extend_from_slice(req_payload);
                        let _ = pkt.send(payload);
                    }
                }
            }
        }
        IpStackStream::UnknownNetwork(_) => {}
    }
    Ok(())
}

/// Handle a DNS query intercepted from TUN.
///
/// Reads the raw DNS packet, extracts the domain, resolves it using
/// the built-in DnsResolver (bypassing the tunnel), and writes back
/// a synthesized DNS response.
async fn handle_tun_dns<S>(udp: &mut S, resolver: &DnsResolver) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 1500];
    let n = udp.read(&mut buf).await?;
    if n < 12 {
        return Ok(());
    }
    let query = &buf[..n];

    match crate::dns::parse_dns_query(query) {
        Ok((domain, qtype)) => {
            tracing::trace!("TUN DNS: {} (type {})", domain, qtype);

            let timeout = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                resolver.resolve(&domain),
            );

            match timeout.await {
                Ok(Ok(addrs)) => {
                    // Filter by query type: A (1) → IPv4 only, AAAA (28) → IPv6 only
                    let filtered: Vec<IpAddr> = match qtype {
                        1 => addrs.into_iter().filter(|a| a.is_ipv4()).collect(),
                        28 => addrs.into_iter().filter(|a| a.is_ipv6()).collect(),
                        _ => addrs,
                    };
                    match crate::dns::build_dns_response(query, &filtered) {
                        Ok(resp) => {
                            udp.write_all(&resp).await?;
                        }
                        Err(e) => {
                            tracing::debug!("TUN DNS response build error for {}: {}", domain, e);
                            let err_resp = crate::dns::build_dns_error_response(query, 2);
                            let _ = udp.write_all(&err_resp).await;
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!("TUN DNS resolve error for {}: {}", domain, e);
                    // SERVFAIL
                    let err_resp = crate::dns::build_dns_error_response(query, 2);
                    let _ = udp.write_all(&err_resp).await;
                }
                Err(_) => {
                    tracing::debug!("TUN DNS timeout for {}", domain);
                    let err_resp = crate::dns::build_dns_error_response(query, 2);
                    let _ = udp.write_all(&err_resp).await;
                }
            }
        }
        Err(e) => {
            tracing::trace!("TUN DNS parse error: {}", e);
            let err_resp = crate::dns::build_dns_error_response(query, 1);
            let _ = udp.write_all(&err_resp).await;
        }
    }

    let _ = udp.shutdown().await;
    Ok(())
}

/// Convert a SocketAddr to (host_string, port).
fn addr_to_host_port(addr: &SocketAddr) -> (String, u16) {
    (addr.ip().to_string(), addr.port())
}

// ─── Platform default TUN device name ───

fn default_tun_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "tun0"
    }
    #[cfg(target_os = "macos")]
    {
        "utun3"
    }
    #[cfg(target_os = "windows")]
    {
        "xtune-tun"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "tun0"
    }
}

// ─── Cross-platform route setup ───

/// DNS server IPs that must bypass TUN to avoid a DNS resolution loop.
/// When TUN intercepts port-53 UDP and resolves via these servers,
/// the outbound DNS packets must NOT re-enter the TUN device.
const DNS_BYPASS_IPS: &[Ipv4Addr] = &[
    Ipv4Addr::new(8, 8, 8, 8),
    Ipv4Addr::new(1, 1, 1, 1),
    Ipv4Addr::new(223, 5, 5, 5),
    Ipv4Addr::new(119, 29, 29, 29),
];

/// Set up system routes to direct all traffic through the TUN device.
///
/// Returns a [`TunRouteGuard`] that restores routes on drop.
pub fn setup_tun_routes(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
) -> Result<TunRouteGuard> {
    // Combine proxy server IPs and DNS server IPs for loop avoidance.
    let mut bypass_ips: Vec<Ipv4Addr> = proxy_server_ips.to_vec();
    for &ip in DNS_BYPASS_IPS {
        if !bypass_ips.contains(&ip) {
            bypass_ips.push(ip);
        }
    }

    let (orig_gateway, orig_iface) =
        get_default_gateway().context("Failed to get default gateway")?;

    tracing::info!(
        "Original default route: {} via {}",
        orig_gateway,
        orig_iface
    );

    #[cfg(target_os = "linux")]
    setup_routes_linux(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "macos")]
    setup_routes_macos(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "windows")]
    setup_routes_windows(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    tracing::info!("Default route set to TUN device {}", route_info.tun_name);

    Ok(TunRouteGuard {
        orig_gateway,
        orig_iface,
        bypass_ips,
        restored: AtomicBool::new(false),
    })
}

/// Guard that restores system routes when dropped.
pub struct TunRouteGuard {
    orig_gateway: Ipv4Addr,
    orig_iface: String,
    bypass_ips: Vec<Ipv4Addr>,
    restored: AtomicBool,
}

impl TunRouteGuard {
    /// Explicitly restore routes.
    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::Relaxed) {
            return;
        }

        tracing::info!("Restoring original routes");

        #[cfg(target_os = "linux")]
        self.restore_linux();

        #[cfg(target_os = "macos")]
        self.restore_macos();

        #[cfg(target_os = "windows")]
        self.restore_windows();
    }
}

impl Drop for TunRouteGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

// ─── Linux implementation ───

#[cfg(target_os = "linux")]
fn setup_routes_linux(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    orig_iface: &str,
) -> Result<()> {
    // Enable IP forwarding
    run_cmd("sysctl", &["-w", "net.ipv4.ip_forward=1"])?;

    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_route = format!("{}/32", ip);
        let _ = run_cmd(
            "ip",
            &["route", "add", &ip_route, "via", &gw_str, "dev", orig_iface],
        );
        tracing::info!(
            "Route: {} -> {} via {} (loop avoidance)",
            ip,
            orig_gateway,
            orig_iface
        );
    }

    // Wait for TUN device to be fully ready before setting routes
    wait_for_tun_device(&route_info.tun_name)?;

    let tun_gw = route_info.tun_gateway.to_string();

    // Try atomic replace first (no window without default route)
    if run_cmd(
        "ip",
        &[
            "route",
            "replace",
            "default",
            "via",
            &tun_gw,
            "dev",
            &route_info.tun_name,
        ],
    )
    .is_ok()
    {
        return Ok(());
    }
    tracing::warn!("ip route replace via gateway failed, trying dev-only route");

    // Fallback: try dev-only (no via) – more compatible with some TUN configurations
    if run_cmd(
        "ip",
        &["route", "replace", "default", "dev", &route_info.tun_name],
    )
    .is_ok()
    {
        return Ok(());
    }
    tracing::warn!("ip route replace dev-only failed, trying del+add");

    // Last resort: delete + add
    let _ = run_cmd("ip", &["route", "del", "default"]);
    run_cmd(
        "ip",
        &["route", "add", "default", "dev", &route_info.tun_name],
    )
    .with_context(|| {
        format!(
            "Failed to set default route through TUN device {}. {}",
            route_info.tun_name,
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "linux")]
impl TunRouteGuard {
    fn restore_linux(&self) {
        let gw_str = self.orig_gateway.to_string();
        // Use replace for atomic route restoration
        let _ = run_cmd(
            "ip",
            &[
                "route",
                "replace",
                "default",
                "via",
                &gw_str,
                "dev",
                &self.orig_iface,
            ],
        );
        for ip in &self.bypass_ips {
            let ip_route = format!("{}/32", ip);
            let _ = run_cmd("ip", &["route", "del", &ip_route]);
        }
    }
}

// ─── macOS implementation ───

#[cfg(target_os = "macos")]
fn setup_routes_macos(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    _orig_iface: &str,
) -> Result<()> {
    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_str = ip.to_string();
        let _ = run_cmd("route", &["-n", "add", "-host", &ip_str, &gw_str]);
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Replace default route through TUN gateway
    let tun_gw = route_info.tun_gateway.to_string();

    // Try change first (atomic), then delete+add as fallback
    if run_cmd("route", &["-n", "change", "default", &tun_gw]).is_ok() {
        return Ok(());
    }
    tracing::warn!("route change failed, trying delete+add");

    let _ = run_cmd("route", &["-n", "delete", "default"]);
    run_cmd("route", &["-n", "add", "default", &tun_gw]).with_context(|| {
        format!(
            "Failed to set default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "macos")]
impl TunRouteGuard {
    fn restore_macos(&self) {
        let gw_str = self.orig_gateway.to_string();
        // Try change first, then delete+add
        if run_cmd("route", &["-n", "change", "default", &gw_str]).is_err() {
            let _ = run_cmd("route", &["-n", "delete", "default"]);
            let _ = run_cmd("route", &["-n", "add", "default", &gw_str]);
        }
        for ip in &self.bypass_ips {
            let _ = run_cmd("route", &["-n", "delete", "-host", &ip.to_string()]);
        }
    }
}

// ─── Windows implementation ───

#[cfg(target_os = "windows")]
fn setup_routes_windows(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    _orig_iface: &str,
) -> Result<()> {
    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_str = ip.to_string();
        let _ = run_cmd(
            "route",
            &[
                "add",
                &ip_str,
                "mask",
                "255.255.255.255",
                &gw_str,
                "metric",
                "5",
            ],
        );
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Replace default route through TUN gateway
    let tun_gw = route_info.tun_gateway.to_string();

    // Try change first (atomic), then delete+add as fallback
    if run_cmd(
        "route",
        &[
            "change", "0.0.0.0", "mask", "0.0.0.0", &tun_gw, "metric", "3",
        ],
    )
    .is_ok()
    {
        return Ok(());
    }
    tracing::warn!("route change failed, trying delete+add");

    let _ = run_cmd("route", &["delete", "0.0.0.0"]);
    run_cmd(
        "route",
        &["add", "0.0.0.0", "mask", "0.0.0.0", &tun_gw, "metric", "3"],
    )
    .with_context(|| {
        format!(
            "Failed to set default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "windows")]
impl TunRouteGuard {
    fn restore_windows(&self) {
        let gw_str = self.orig_gateway.to_string();
        // Try change first, then delete+add
        if run_cmd(
            "route",
            &[
                "change", "0.0.0.0", "mask", "0.0.0.0", &gw_str, "metric", "5",
            ],
        )
        .is_err()
        {
            let _ = run_cmd("route", &["delete", "0.0.0.0"]);
            let _ = run_cmd(
                "route",
                &["add", "0.0.0.0", "mask", "0.0.0.0", &gw_str, "metric", "5"],
            );
        }
        for ip in &self.bypass_ips {
            let _ = run_cmd("route", &["delete", &ip.to_string()]);
        }
    }
}

// ─── Cross-platform default gateway detection ───

fn get_default_gateway() -> Result<(Ipv4Addr, String)> {
    #[cfg(target_os = "linux")]
    {
        get_default_gateway_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_default_gateway_macos()
    }
    #[cfg(target_os = "windows")]
    {
        get_default_gateway_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        bail!("TUN mode is not supported on this platform")
    }
}

#[cfg(target_os = "linux")]
fn get_default_gateway_linux() -> Result<(Ipv4Addr, String)> {
    // Try /proc/net/route first (most reliable, no external command)
    if let Ok(content) = std::fs::read_to_string("/proc/net/route") {
        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 8 {
                continue;
            }
            let iface = fields[0].trim();
            let dest = fields[1].trim();
            let gw_hex = fields[2].trim();
            let flags: u32 = u32::from_str_radix(fields[3].trim(), 16).unwrap_or(0);
            let mask = fields[7].trim();

            if dest == "00000000" && mask == "00000000" && (flags & 0x0002) != 0 {
                let gw_u32 = u32::from_str_radix(gw_hex, 16)
                    .context("Invalid gateway hex in /proc/net/route")?;
                // /proc/net/route stores in little-endian on x86
                let gateway = Ipv4Addr::new(
                    (gw_u32 & 0xFF) as u8,
                    ((gw_u32 >> 8) & 0xFF) as u8,
                    ((gw_u32 >> 16) & 0xFF) as u8,
                    ((gw_u32 >> 24) & 0xFF) as u8,
                );
                return Ok((gateway, iface.to_string()));
            }
        }
    }

    // Fallback: parse `ip route show default`
    let output = run_cmd("ip", &["route", "show", "default"])?;
    // Example: "default via 192.168.1.1 dev eth0 proto dhcp metric 100"
    let parts: Vec<&str> = output.split_whitespace().collect();
    if parts.len() >= 5 && parts[0] == "default" && parts[1] == "via" {
        let gw: Ipv4Addr = parts[2].parse().context("Failed to parse gateway IP")?;
        let iface = if parts[3] == "dev" { parts[4] } else { "eth0" };
        return Ok((gw, iface.to_string()));
    }

    bail!("No default gateway found on Linux")
}

#[cfg(target_os = "macos")]
fn get_default_gateway_macos() -> Result<(Ipv4Addr, String)> {
    // Use `route -n get default` to obtain the gateway and interface
    let output = run_cmd("route", &["-n", "get", "default"])?;
    let mut gateway: Option<Ipv4Addr> = None;
    let mut iface: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(gw) = line.strip_prefix("gateway:") {
            gateway = gw.trim().parse().ok();
        }
        if let Some(if_name) = line.strip_prefix("interface:") {
            iface = Some(if_name.trim().to_string());
        }
    }

    match (gateway, iface) {
        (Some(gw), Some(ifc)) => Ok((gw, ifc)),
        (Some(gw), None) => Ok((gw, "en0".to_string())),
        _ => {
            // Fallback: parse `netstat -rn`
            let ns_output = run_cmd("netstat", &["-rn"])?;
            for line in ns_output.lines() {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() >= 4 && fields[0] == "default" {
                    if let Ok(gw) = fields[1].parse::<Ipv4Addr>() {
                        let if_name = fields.last().unwrap_or(&"en0");
                        return Ok((gw, if_name.to_string()));
                    }
                }
            }
            bail!("No default gateway found on macOS")
        }
    }
}

#[cfg(target_os = "windows")]
fn get_default_gateway_windows() -> Result<(Ipv4Addr, String)> {
    // Parse `route print 0.0.0.0` for the default route
    let output = run_cmd("route", &["print", "0.0.0.0"])?;
    // Look for lines like:
    //   0.0.0.0          0.0.0.0      192.168.1.1    192.168.1.100     25
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4 && fields[0] == "0.0.0.0" && fields[1] == "0.0.0.0" {
            if let Ok(gw) = fields[2].parse::<Ipv4Addr>() {
                // Interface is the local IP (fields[3]), use it as the "iface" identifier
                let iface = fields.get(3).unwrap_or(&"0.0.0.0");
                return Ok((gw, iface.to_string()));
            }
        }
    }

    // Fallback: try PowerShell
    let ps_output = run_cmd(
        "powershell",
        &[
            "-Command",
            "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -First 1).NextHop",
        ],
    );
    if let Ok(ps_out) = ps_output {
        let gw_str = ps_out.trim();
        if let Ok(gw) = gw_str.parse::<Ipv4Addr>() {
            return Ok((gw, "0.0.0.0".to_string()));
        }
    }

    bail!("No default gateway found on Windows")
}

// ─── TUN device readiness check ───

/// Wait for a TUN device to appear and be operational.
#[cfg(target_os = "linux")]
fn wait_for_tun_device(tun_name: &str) -> Result<()> {
    let sys_path = format!("/sys/class/net/{}", tun_name);
    let operstate_path = format!("{}/operstate", sys_path);

    for i in 0..20 {
        if std::path::Path::new(&sys_path).exists() {
            // Check if device is up (operstate is "up" or "unknown" for TUN)
            if let Ok(state) = std::fs::read_to_string(&operstate_path) {
                let state = state.trim();
                if state == "up" || state == "unknown" {
                    tracing::debug!(
                        "TUN device {} ready (state: {}, attempt {})",
                        tun_name,
                        state,
                        i
                    );
                    return Ok(());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // If the device exists but operstate never became up, still try
    if std::path::Path::new(&sys_path).exists() {
        tracing::warn!(
            "TUN device {} exists but may not be fully ready, proceeding anyway",
            tun_name
        );
        return Ok(());
    }

    bail!("TUN device {} did not appear within 2 seconds", tun_name)
}

// ─── Utility functions ───

/// Run an external command with a timeout and return its output.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    run_cmd_timeout(cmd, args, std::time::Duration::from_secs(10))
}

/// Run an external command with a specific timeout.
fn run_cmd_timeout(cmd: &str, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    let cmd_path = find_cmd(cmd).unwrap_or_else(|| cmd.to_string());
    let cmd_path_clone = cmd_path.clone();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let args_display = args.join(" ");

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = std::process::Command::new(&cmd_path_clone)
            .args(&args_owned)
            .output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            let _ = handle.join();
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("{} {} failed: {}", cmd, args_display, stderr.trim());
            }
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Err(e)) => {
            let _ = handle.join();
            bail!("Failed to run {} {}: {}", cmd, args_display, e);
        }
        Err(_) => {
            // Timeout - the thread may still be running but we proceed
            bail!(
                "{} {} timed out after {}s",
                cmd,
                args_display,
                timeout.as_secs()
            );
        }
    }
}

/// Find a command in PATH or common system directories.
fn find_cmd(cmd: &str) -> Option<String> {
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(output) = std::process::Command::new("which").arg(cmd).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
        for dir in &["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
            let path = format!("{}/{}", dir, cmd);
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = std::process::Command::new("where").arg(cmd).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Resolve a hostname to IPv4 addresses for route setup.
pub fn resolve_to_ipv4(host: &str) -> Vec<Ipv4Addr> {
    use std::net::ToSocketAddrs;
    format!("{}:0", host)
        .to_socket_addrs()
        .unwrap_or_else(|_| vec![].into_iter())
        .filter_map(|addr| match addr.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            _ => None,
        })
        .collect()
}

/// Check if TUN mode is supported on the current platform.
pub fn tun_supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// Return a human-readable description of TUN requirements for the current OS.
pub fn tun_requirements() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "Requires root or CAP_NET_ADMIN capability"
    }
    #[cfg(target_os = "macos")]
    {
        "Requires root privileges (sudo)"
    }
    #[cfg(target_os = "windows")]
    {
        "Requires Administrator privileges"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "TUN mode is not supported on this platform"
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivilegeStatus {
    Privileged,
    Missing,
    Unknown,
}

fn validate_tun_environment() -> Result<()> {
    if !tun_supported() {
        bail!("{}", tun_requirements());
    }

    #[cfg(target_os = "linux")]
    {
        if !std::path::Path::new("/dev/net/tun").exists() {
            bail!(
                "TUN device node /dev/net/tun is missing. Load the tun kernel module and ensure the device node exists."
            );
        }

        // Check that required routing commands are available
        let mut missing = Vec::new();
        if find_cmd("ip").is_none() {
            missing.push("ip (install: apt install iproute2 / dnf install iproute)");
        }
        if find_cmd("sysctl").is_none() {
            missing.push("sysctl (install: apt install procps / dnf install procps-ng)");
        }
        if !missing.is_empty() {
            bail!(
                "TUN mode requires the following commands: {}",
                missing.join(", ")
            );
        }
    }

    #[cfg(target_os = "windows")]
    {
        if find_cmd("route").is_none() {
            bail!("TUN mode requires the 'route' command (should be available by default on Windows)");
        }
    }

    match current_privilege_status() {
        PrivilegeStatus::Missing if cfg!(any(target_os = "macos", target_os = "windows")) => {
            bail!("{}", tun_requirements());
        }
        _ => Ok(()),
    }
}

fn tun_creation_failure_message() -> String {
    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/dev/net/tun").exists() {
        return "Failed to create TUN device: /dev/net/tun is missing. Load the tun kernel module and ensure the device node exists.".to_string();
    }

    match current_privilege_status() {
        PrivilegeStatus::Missing => format!("Failed to create TUN device. {}", tun_requirements()),
        PrivilegeStatus::Privileged => {
            #[cfg(target_os = "windows")]
            {
                "Failed to create TUN device. The WinTun driver may not be installed.\n\
                 Download wintun.dll from https://www.wintun.net/ and place it in the same folder as the application."
                    .to_string()
            }
            #[cfg(not(target_os = "windows"))]
            {
                "Failed to create TUN device. The TUN driver or adapter may be unavailable on this system.".to_string()
            }
        }
        PrivilegeStatus::Unknown => format!(
            "Failed to create TUN device. Ensure the TUN driver is installed and {}",
            tun_requirements().to_lowercase()
        ),
    }
}

fn tun_route_failure_hint() -> String {
    match current_privilege_status() {
        PrivilegeStatus::Missing => tun_requirements().to_string(),
        PrivilegeStatus::Privileged => {
            "The current process already has elevated privileges, so the TUN interface or routing command likely failed to initialize correctly.".to_string()
        }
        PrivilegeStatus::Unknown => format!(
            "Ensure the required routing tools are available and {}",
            tun_requirements().to_lowercase()
        ),
    }
}

fn current_privilege_status() -> PrivilegeStatus {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if unix_is_root() {
            return PrivilegeStatus::Privileged;
        }
        return PrivilegeStatus::Missing;
    }

    #[cfg(target_os = "windows")]
    {
        return match windows_is_elevated() {
            Some(true) => PrivilegeStatus::Privileged,
            Some(false) => PrivilegeStatus::Missing,
            None => PrivilegeStatus::Unknown,
        };
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PrivilegeStatus::Unknown
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
unsafe extern "C" {
    fn geteuid() -> u32;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn unix_is_root() -> bool {
    unsafe { geteuid() == 0 }
}

#[cfg(target_os = "windows")]
fn windows_is_elevated() -> Option<bool> {
    // Use Win32 API — most reliable way to check admin privileges
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    Some(unsafe { IsUserAnAdmin() != 0 })
}

// === WinTun DLL auto-install (Windows only) ===

const WINTUN_DLL: &str = "wintun.dll";

// Embed the architecture-specific wintun.dll at compile time.
// The DLL files are from https://www.wintun.net/builds/wintun-0.14.1.zip
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_amd64.dll");

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_arm64.dll");

#[cfg(all(target_os = "windows", target_arch = "x86"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_x86.dll");

/// Check if wintun.dll is available (exists next to the executable or in PATH).
pub fn wintun_dll_available() -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        true // not needed on non-Windows
    }
    #[cfg(target_os = "windows")]
    {
        wintun_dll_path().map(|p| p.exists()).unwrap_or(false)
    }
}

/// Path where wintun.dll should be placed (next to the executable).
fn wintun_dll_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join(WINTUN_DLL)))
}

/// Ensure wintun.dll is available. Extracts the embedded DLL if missing.
#[cfg(target_os = "windows")]
pub async fn ensure_wintun_dll() -> Result<std::path::PathBuf> {
    let dll_path = wintun_dll_path()
        .ok_or_else(|| anyhow::anyhow!("cannot determine executable directory"))?;

    if dll_path.exists() {
        tracing::info!("wintun.dll found at {}", dll_path.display());
        return Ok(dll_path);
    }

    tracing::info!(
        "wintun.dll not found, extracting embedded DLL ({} bytes) to {}",
        WINTUN_DLL_BYTES.len(),
        dll_path.display()
    );

    std::fs::write(&dll_path, WINTUN_DLL_BYTES).with_context(|| {
        format!("failed to write wintun.dll to {}", dll_path.display())
    })?;

    tracing::info!("wintun.dll installed at {}", dll_path.display());
    Ok(dll_path)
}

/// On non-Windows platforms, this is a no-op.
#[cfg(not(target_os = "windows"))]
pub async fn ensure_wintun_dll() -> Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from("/dev/net/tun"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tun_supported() {
        // On Linux/macOS/Windows, TUN should be reported as supported
        assert!(tun_supported());
    }

    #[test]
    fn test_tun_requirements_not_empty() {
        let req = tun_requirements();
        assert!(!req.is_empty());
    }

    #[test]
    fn test_resolve_to_ipv4() {
        let addrs = resolve_to_ipv4("localhost");
        assert!(
            addrs.contains(&Ipv4Addr::LOCALHOST),
            "localhost should resolve to 127.0.0.1"
        );
    }

    #[test]
    fn test_default_gateway_detection() {
        // This test verifies gateway detection works on the current system
        match get_default_gateway() {
            Ok((gw, iface)) => {
                assert!(!iface.is_empty(), "interface name should not be empty");
                // Gateway should not be 0.0.0.0 (that indicates no gateway)
                tracing::info!("Detected gateway: {} via {}", gw, iface);
            }
            Err(e) => {
                // Acceptable in some CI environments with no default route
                tracing::warn!("No default gateway (may be expected in CI): {}", e);
            }
        }
    }

    #[test]
    fn test_validate_tun_environment() {
        // Just verify it doesn't panic; result depends on the runtime environment
        let result = validate_tun_environment();
        tracing::info!("validate_tun_environment: {:?}", result);
    }

    #[test]
    fn test_addr_to_host_port() {
        let addr: SocketAddr = "1.2.3.4:443".parse().unwrap();
        let (host, port) = addr_to_host_port(&addr);
        assert_eq!(host, "1.2.3.4");
        assert_eq!(port, 443);
    }

    #[test]
    fn test_find_cmd_known() {
        // `ls` should be findable on any Unix system
        #[cfg(not(target_os = "windows"))]
        {
            let result = find_cmd("ls");
            assert!(result.is_some(), "ls should be found on Unix");
        }
    }

    /// Smoke test: create a TUN device, verify name, then stop.
    /// Requires root and /dev/net/tun — skipped automatically if unavailable.
    #[tokio::test]
    async fn test_tun_create_and_stop() {
        if validate_tun_environment().is_err() {
            eprintln!("Skipping TUN smoke test (environment not suitable)");
            return;
        }

        let outbound = SharedOutbound::direct();
        let tun = TunProxy::start(outbound).expect("TUN creation should succeed");
        let name = tun.tun_name().to_string();
        assert!(!name.is_empty(), "TUN device name should not be empty");

        let info = tun.route_info();
        assert_eq!(info.tun_gateway, TUN_GATEWAY);

        tun.stop().await;
    }
}
