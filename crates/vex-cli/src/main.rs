use std::env;
use std::fs;
use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use vex_core::{
    AppConfig, ProxyProtocol, ProxyService, Router, RoutingOutbound, RuleSet, SharedOutbound,
    TunProxy, create_outbound, fetch_subscription, normalize_node_names, resolve_to_ipv4,
    setup_tun_routes, tun_requirements, tun_supported,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();

    // Handle --init: generate a default config and exit
    if args.iter().any(|a| a == "--init") {
        let path = args
            .iter()
            .position(|a| a == "--init")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config.yaml");
        return init_config(path);
    }

    // Feature flags
    let enable_tun = args.iter().any(|a| a == "--tun");
    let setup_routes = args.iter().any(|a| a == "--tun-routes");

    let config_path = parse_config_path()?;
    let config = load_config(&config_path)
        .await
        .with_context(|| format!("failed to load config from {}", config_path))?;

    let active_index = resolve_active_index(&config)?;
    let outbound = build_outbound(&config, active_index)?;
    let selected = active_index.and_then(|index| config.nodes.get(index));

    // Start HTTP/SOCKS5 proxy service
    let mut service = ProxyService::with_outbound(outbound.clone());
    service
        .start(&config.listen_addr, config.socks_port, config.http_port)
        .await?;

    if let Some(node) = selected {
        tracing::info!(
            "Vex CLI started with node '{}' ({})",
            node.name,
            protocol_name(&node.protocol)
        );
    } else {
        tracing::warn!("Vex CLI started without proxy node, outbound=direct");
    }

    tracing::info!(
        "Listening: SOCKS5={}:{}, HTTP={}:{}",
        config.listen_addr,
        config.socks_port,
        config.listen_addr,
        config.http_port
    );

    // Optionally start TUN proxy
    let tun_proxy = if enable_tun {
        if !tun_supported() {
            tracing::warn!(
                "TUN mode not supported on this system: {}",
                tun_requirements()
            );
            None
        } else {
            match TunProxy::start(outbound.clone()) {
                Ok(tun) => {
                    let route_info = tun.route_info();
                    tracing::info!(
                        "TUN device started: {} — TCP+UDP+ICMP proxy active",
                        route_info.tun_name
                    );

                    if setup_routes {
                        // Collect proxy server IPs to bypass routing loop
                        let bypass_ips = collect_proxy_ips(&config, active_index).await;
                        match setup_tun_routes(&route_info, &bypass_ips) {
                            Ok(_guard) => {
                                tracing::info!(
                                    "System default route → TUN (desktop mode, {} IPs bypassed)",
                                    bypass_ips.len()
                                );
                                // _guard is intentionally dropped at shutdown via emergency_restore_routes
                            }
                            Err(e) => tracing::warn!("Route setup failed: {}", e),
                        }
                    } else {
                        tracing::info!(
                            "TUN device: {} (router mode — use iptables/TPROXY to route traffic in)",
                            route_info.tun_name
                        );
                        tracing::info!(
                            "  TUN IP: 198.18.0.1  Add: ip route add <LAN> dev {}",
                            route_info.tun_name
                        );
                    }

                    Some(tun)
                }
                Err(e) => {
                    tracing::error!("Failed to start TUN proxy: {}", e);
                    None
                }
            }
        }
    } else {
        None
    };

    tracing::info!("Press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received");

    // Stop TUN first, then proxy
    if let Some(tun) = tun_proxy {
        tun.stop().await;
    }
    service.stop().await;

    // Clean up system proxy and TUN routes
    let _ = vex_core::clear_system_proxy();
    vex_core::emergency_restore_routes();

    Ok(())
}

fn parse_config_path() -> Result<String> {
    // Skip flag arguments (--tun, --tun-routes, etc.)
    let path = env::args().skip(1).find(|a| !a.starts_with("--"));
    match path {
        Some(p) => Ok(p),
        None => {
            // Try default path
            let default_path = "config.yaml";
            if std::path::Path::new(default_path).exists() {
                tracing::info!("Using default config: {}", default_path);
                Ok(default_path.to_string())
            } else {
                bail!(
                    "usage: vex-cli <config.yaml> [--tun] [--tun-routes]\n\
                     flags:\n\
                     \x20 --tun           enable TUN proxy (TCP+UDP transparent proxy)\n\
                     \x20 --tun-routes    also set system default route through TUN (desktop mode)\n\
                     \x20 --init [path]   generate default config file"
                )
            }
        }
    }
}

/// Generate a default config file at the given path.
fn init_config(path: &str) -> Result<()> {
    if std::path::Path::new(path).exists() {
        bail!("config file already exists: {}", path);
    }
    let config = AppConfig::default();
    let content = serde_yaml::to_string(&config)?;
    fs::write(path, &content)?;
    println!("✓ Default config written to: {}", path);
    println!("  Edit the file to add subscriptions or nodes, then run:");
    println!("  vex-cli {}", path);
    Ok(())
}

async fn load_config(path: &str) -> Result<AppConfig> {
    let content = fs::read_to_string(path)?;
    let mut config: AppConfig = serde_yaml::from_str(&content)?;

    if !config.subscriptions.is_empty() {
        tracing::info!(
            "Fetching {} subscription(s) from config",
            config.subscriptions.len()
        );

        let mut merged_nodes = config.nodes.clone();
        for subscription in &config.subscriptions {
            let mut nodes = fetch_subscription(subscription)
                .await
                .with_context(|| format!("failed to fetch subscription '{}'", subscription.name))?;
            tracing::info!(
                "Fetched {} node(s) from subscription '{}'",
                nodes.len(),
                subscription.name
            );
            merged_nodes.append(&mut nodes);
        }
        config.nodes = merged_nodes;
    }

    // Normalize node names (URL decode)
    normalize_node_names(&mut config.nodes);

    Ok(config)
}

fn resolve_active_index(config: &AppConfig) -> Result<Option<usize>> {
    if config.nodes.is_empty() {
        return Ok(None);
    }

    match config.active_node {
        Some(index) if index < config.nodes.len() => Ok(Some(index)),
        Some(index) => bail!(
            "active_node {} is out of range for {} configured node(s)",
            index,
            config.nodes.len()
        ),
        None => {
            tracing::warn!("active_node is not set, using the first configured node");
            Ok(Some(0))
        }
    }
}

fn build_outbound(config: &AppConfig, active_index: Option<usize>) -> Result<SharedOutbound> {
    let base_outbound = match active_index {
        Some(index) => create_outbound(&config.nodes[index])?,
        None => SharedOutbound::direct(),
    };

    if config.rules.is_empty() {
        return Ok(base_outbound);
    }

    let router = Arc::new(Router::new(RuleSet::from_config(&config.rules)));
    Ok(SharedOutbound(Arc::new(RoutingOutbound::new(
        router,
        base_outbound,
    ))))
}

/// Resolve proxy server hostnames to IPv4 addresses for TUN route bypass.
async fn collect_proxy_ips(config: &AppConfig, active_index: Option<usize>) -> Vec<Ipv4Addr> {
    let mut ips = Vec::new();
    if let Some(idx) = active_index {
        if let Some(node) = config.nodes.get(idx) {
            let resolved = resolve_to_ipv4(&node.server);
            if resolved.is_empty() {
                tracing::warn!("Could not resolve proxy server: {}", node.server);
            }
            ips.extend(resolved);
        }
    }
    ips
}

fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "shadowsocks",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hysteria2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_core::{Node, TransportConfig, TransportType};

    fn sample_node(name: &str) -> Node {
        Node {
            name: name.to_string(),
            server: "127.0.0.1".to_string(),
            port: 1080,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "secret".to_string(),
                udp: false,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Tcp,
                tls: None,
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn resolve_active_index_allows_empty_nodes() {
        let config = AppConfig::default();
        assert_eq!(resolve_active_index(&config).unwrap(), None);
    }

    #[test]
    fn resolve_active_index_uses_explicit_index() {
        let config = AppConfig {
            nodes: vec![sample_node("a"), sample_node("b")],
            active_node: Some(1),
            ..AppConfig::default()
        };

        assert_eq!(resolve_active_index(&config).unwrap(), Some(1));
    }

    #[test]
    fn resolve_active_index_defaults_to_first_node() {
        let config = AppConfig {
            nodes: vec![sample_node("a"), sample_node("b")],
            active_node: None,
            ..AppConfig::default()
        };

        assert_eq!(resolve_active_index(&config).unwrap(), Some(0));
    }

    #[test]
    fn resolve_active_index_rejects_out_of_range_index() {
        let config = AppConfig {
            nodes: vec![sample_node("a")],
            active_node: Some(2),
            ..AppConfig::default()
        };

        assert!(resolve_active_index(&config).is_err());
    }
}
