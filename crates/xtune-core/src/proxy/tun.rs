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
    /// **Requires root privileges or CAP_NET_ADMIN capability.**
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
            .context("Failed to create TUN device (root required)")?;

        // Get TUN device name
        let tun_name = get_tun_name().unwrap_or_else(|| "tun0".to_string());

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

            // For UDP, connect through TCP outbound as a best-effort fallback.
            // Full UDP relay requires protocol-level support (TUIC supports it).
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
            // Handle ICMP echo requests (ping)
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

/// Get the TUN device name by scanning network interfaces.
fn get_tun_name() -> Option<String> {
    // On Linux, the tun crate typically creates "tun0", "tun1", etc.
    // We look for newly created tun device.
    for i in 0..10 {
        let name = format!("tun{}", i);
        let path = format!("/sys/class/net/{}", name);
        if std::path::Path::new(&path).exists() {
            return Some(name);
        }
    }
    None
}

/// Set up system routes to direct all traffic through the TUN device.
///
/// This function:
/// 1. Enables IP forwarding
/// 2. Saves the current default gateway
/// 3. Adds a direct route for proxy server IPs (loop avoidance)
/// 4. Replaces the default route to go through TUN
///
/// Returns a [`TunRouteGuard`] that restores routes on drop.
pub fn setup_tun_routes(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
) -> Result<TunRouteGuard> {
    // Get current default gateway
    let (orig_gateway, orig_iface) = get_default_gateway()
        .context("Failed to get default gateway")?;

    tracing::info!(
        "Original default route: {} via {}",
        orig_gateway,
        orig_iface
    );

    // Enable IP forwarding
    run_cmd("sysctl", &["-w", "net.ipv4.ip_forward=1"])?;

    // Add direct routes for proxy server IPs (so proxy traffic doesn't loop)
    for ip in proxy_server_ips {
        let ip_str = ip.to_string();
        let _ = run_cmd(
            "ip",
            &["route", "add", &format!("{}/32", ip_str), "via", &orig_gateway.to_string(), "dev", &orig_iface],
        );
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Add default route via TUN
    let _ = run_cmd(
        "ip",
        &["route", "del", "default"],
    );
    run_cmd(
        "ip",
        &["route", "add", "default", "via", &route_info.tun_gateway.to_string(), "dev", &route_info.tun_name],
    )?;

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

        // Restore default route
        let _ = run_cmd("ip", &["route", "del", "default"]);
        let _ = run_cmd(
            "ip",
            &["route", "add", "default", "via", &self.orig_gateway.to_string(), "dev", &self.orig_iface],
        );

        // Remove proxy server direct routes
        for ip in &self.proxy_server_ips {
            let _ = run_cmd(
                "ip",
                &["route", "del", &format!("{}/32", ip)],
            );
        }
    }
}

impl Drop for TunRouteGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Get the current default gateway IP and interface name.
fn get_default_gateway() -> Result<(Ipv4Addr, String)> {
    let output = std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .context("Failed to run 'ip route show default'")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Example: "default via 172.28.80.1 dev eth0 proto kernel"
    let parts: Vec<&str> = stdout.split_whitespace().collect();

    let via_idx = parts
        .iter()
        .position(|&p| p == "via")
        .context("No 'via' in default route")?;
    let dev_idx = parts
        .iter()
        .position(|&p| p == "dev")
        .context("No 'dev' in default route")?;

    let gateway: Ipv4Addr = parts
        .get(via_idx + 1)
        .context("Missing gateway IP")?
        .parse()
        .context("Invalid gateway IP")?;

    let iface = parts
        .get(dev_idx + 1)
        .context("Missing interface name")?
        .to_string();

    Ok((gateway, iface))
}

/// Run an external command and return its output.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run: {} {}", cmd, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} {} failed: {}", cmd, args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
