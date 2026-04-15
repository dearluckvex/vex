use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use etherparse::Icmpv4Header;
use ipstack::{IpNumber, IpStackConfig, IpStackStream, TcpConfig, TcpOptions};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use super::connector::SharedOutbound;

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

        let tun_dev = tun::create_as_async(&tun_config)
            .context("Failed to create TUN device (root/admin required)")?;

        let tun_name = get_tun_name().unwrap_or_else(|| default_tun_name().to_string());

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
            tracing::info!("TUN accept loop started");
            while running_clone.load(Ordering::Relaxed) {
                match ip_stack.accept().await {
                    Ok(stream) => {
                        let outbound = outbound.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_tun_stream(stream, outbound).await {
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
async fn handle_tun_stream(stream: IpStackStream, outbound: SharedOutbound) -> Result<()> {
    match stream {
        IpStackStream::Tcp(mut tcp) => {
            let dst = tcp.peer_addr();
            let (host, port) = addr_to_host_port(&dst);
            tracing::debug!("TUN TCP: -> {}:{}", host, port);

            match outbound.connect(&host, port).await {
                Ok(mut remote) => {
                    let result = tokio::io::copy_bidirectional(&mut tcp, &mut remote).await;
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
            tracing::debug!("TUN UDP: -> {}:{}", host, port);

            match outbound.connect(&host, port).await {
                Ok(mut remote) => {
                    let result = tokio::io::copy_bidirectional(&mut udp, &mut remote).await;
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
                        let mut resp =
                            Icmpv4Header::new(etherparse::Icmpv4Type::EchoReply(echo));
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

/// Convert a SocketAddr to (host_string, port).
fn addr_to_host_port(addr: &SocketAddr) -> (String, u16) {
    (addr.ip().to_string(), addr.port())
}

// ─── Platform default TUN device name ───

fn default_tun_name() -> &'static str {
    #[cfg(target_os = "linux")]
    { "tun0" }
    #[cfg(target_os = "macos")]
    { "utun3" }
    #[cfg(target_os = "windows")]
    { "xtune-tun" }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { "tun0" }
}

// ─── Cross-platform TUN device name detection ───

/// Get the TUN device name by scanning network interfaces.
fn get_tun_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    { get_tun_name_linux() }
    #[cfg(target_os = "macos")]
    { get_tun_name_macos() }
    #[cfg(target_os = "windows")]
    { get_tun_name_windows() }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { None }
}

#[cfg(target_os = "linux")]
fn get_tun_name_linux() -> Option<String> {
    for i in 0..10 {
        let name = format!("tun{}", i);
        let path = format!("/sys/class/net/{}", name);
        if std::path::Path::new(&path).exists() {
            return Some(name);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn get_tun_name_macos() -> Option<String> {
    // macOS uses utunN interfaces; scan for the highest-numbered one
    let output = std::process::Command::new("ifconfig")
        .arg("-l")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ifaces = String::from_utf8_lossy(&output.stdout);
    ifaces
        .split_whitespace()
        .filter(|name| name.starts_with("utun"))
        .last()
        .map(|s| s.to_string())
}

#[cfg(target_os = "windows")]
fn get_tun_name_windows() -> Option<String> {
    // On Windows the tun crate (wintun backend) returns a fixed name.
    // We probe for a network adapter whose IP matches our TUN_IPV4.
    let output = std::process::Command::new("netsh")
        .args(["interface", "ip", "show", "addresses"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let tun_ip_str = TUN_IPV4.to_string();
    let mut current_iface: Option<String> = None;
    for line in text.lines() {
        // Interface lines look like: Configuration for interface "Ethernet"
        if let Some(pos) = line.find('"') {
            if let Some(end) = line[pos + 1..].find('"') {
                current_iface = Some(line[pos + 1..pos + 1 + end].to_string());
            }
        }
        if line.contains(&tun_ip_str) {
            if let Some(ref name) = current_iface {
                return Some(name.clone());
            }
        }
    }
    Some("xtune-tun".to_string())
}

// ─── Cross-platform route setup ───

/// Set up system routes to direct all traffic through the TUN device.
///
/// Returns a [`TunRouteGuard`] that restores routes on drop.
pub fn setup_tun_routes(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
) -> Result<TunRouteGuard> {
    let (orig_gateway, orig_iface) = get_default_gateway()
        .context("Failed to get default gateway")?;

    tracing::info!(
        "Original default route: {} via {}",
        orig_gateway,
        orig_iface
    );

    #[cfg(target_os = "linux")]
    setup_routes_linux(route_info, proxy_server_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "macos")]
    setup_routes_macos(route_info, proxy_server_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "windows")]
    setup_routes_windows(route_info, proxy_server_ips, &orig_gateway, &orig_iface)?;

    tracing::info!("Default route set to TUN device {}", route_info.tun_name);

    Ok(TunRouteGuard {
        orig_gateway,
        orig_iface,
        proxy_server_ips: proxy_server_ips.to_vec(),
    })
}

/// Guard that restores system routes when dropped.
pub struct TunRouteGuard {
    orig_gateway: Ipv4Addr,
    orig_iface: String,
    proxy_server_ips: Vec<Ipv4Addr>,
}

impl TunRouteGuard {
    /// Explicitly restore routes.
    pub fn restore(&self) {
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
        tracing::info!("Route: {} -> {} via {} (loop avoidance)", ip, orig_gateway, orig_iface);
    }

    // Replace default route to go through TUN
    let tun_gw = route_info.tun_gateway.to_string();
    let _ = run_cmd("ip", &["route", "del", "default"]);
    run_cmd(
        "ip",
        &["route", "add", "default", "via", &tun_gw, "dev", &route_info.tun_name],
    )?;

    Ok(())
}

#[cfg(target_os = "linux")]
impl TunRouteGuard {
    fn restore_linux(&self) {
        let gw_str = self.orig_gateway.to_string();
        let _ = run_cmd("ip", &["route", "del", "default"]);
        let _ = run_cmd(
            "ip",
            &["route", "add", "default", "via", &gw_str, "dev", &self.orig_iface],
        );
        for ip in &self.proxy_server_ips {
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
    let _ = run_cmd("route", &["-n", "delete", "default"]);
    run_cmd("route", &["-n", "add", "default", &tun_gw])?;

    Ok(())
}

#[cfg(target_os = "macos")]
impl TunRouteGuard {
    fn restore_macos(&self) {
        let gw_str = self.orig_gateway.to_string();
        let _ = run_cmd("route", &["-n", "delete", "default"]);
        let _ = run_cmd("route", &["-n", "add", "default", &gw_str]);
        for ip in &self.proxy_server_ips {
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
            &["add", &ip_str, "mask", "255.255.255.255", &gw_str, "metric", "5"],
        );
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Replace default route through TUN gateway
    let tun_gw = route_info.tun_gateway.to_string();
    let _ = run_cmd("route", &["delete", "0.0.0.0"]);
    run_cmd(
        "route",
        &["add", "0.0.0.0", "mask", "0.0.0.0", &tun_gw, "metric", "3"],
    )?;

    Ok(())
}

#[cfg(target_os = "windows")]
impl TunRouteGuard {
    fn restore_windows(&self) {
        let gw_str = self.orig_gateway.to_string();
        // Remove TUN default route and restore original
        let _ = run_cmd("route", &["delete", "0.0.0.0"]);
        let _ = run_cmd(
            "route",
            &["add", "0.0.0.0", "mask", "0.0.0.0", &gw_str, "metric", "5"],
        );
        for ip in &self.proxy_server_ips {
            let _ = run_cmd("route", &["delete", &ip.to_string()]);
        }
    }
}

// ─── Cross-platform default gateway detection ───

fn get_default_gateway() -> Result<(Ipv4Addr, String)> {
    #[cfg(target_os = "linux")]
    { get_default_gateway_linux() }
    #[cfg(target_os = "macos")]
    { get_default_gateway_macos() }
    #[cfg(target_os = "windows")]
    { get_default_gateway_windows() }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { bail!("TUN mode is not supported on this platform") }
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
        &["-Command", "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -First 1).NextHop"],
    );
    if let Ok(ps_out) = ps_output {
        let gw_str = ps_out.trim();
        if let Ok(gw) = gw_str.parse::<Ipv4Addr>() {
            return Ok((gw, "0.0.0.0".to_string()));
        }
    }

    bail!("No default gateway found on Windows")
}

// ─── Utility functions ───

/// Run an external command and return its output.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    let cmd_path = find_cmd(cmd).unwrap_or_else(|| cmd.to_string());
    let output = std::process::Command::new(&cmd_path)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run: {} {}", cmd_path, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} {} failed: {}", cmd, args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
    cfg!(any(target_os = "linux", target_os = "macos", target_os = "windows"))
}

/// Return a human-readable description of TUN requirements for the current OS.
pub fn tun_requirements() -> &'static str {
    #[cfg(target_os = "linux")]
    { "Requires root or CAP_NET_ADMIN capability" }
    #[cfg(target_os = "macos")]
    { "Requires root privileges (sudo)" }
    #[cfg(target_os = "windows")]
    { "Requires Administrator privileges" }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { "TUN mode is not supported on this platform" }
}
