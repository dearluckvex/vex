use std::env;
use std::fs;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use xtune_core::{
    AppConfig, ProxyProtocol, ProxyService, Router, RoutingOutbound, RuleSet, SharedOutbound,
    create_outbound, fetch_subscription,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = parse_config_path()?;
    let config = load_config(&config_path)
        .await
        .with_context(|| format!("failed to load config from {}", config_path))?;

    let active_index = resolve_active_index(&config)?;
    let outbound = build_outbound(&config, active_index)?;
    let selected = active_index.and_then(|index| config.nodes.get(index));

    let mut service = ProxyService::with_outbound(outbound);
    service
        .start(&config.listen_addr, config.socks_port, config.http_port)
        .await?;

    if let Some(node) = selected {
        tracing::info!(
            "XTune CLI started with node '{}' ({})",
            node.name,
            protocol_name(&node.protocol)
        );
    } else {
        tracing::warn!("XTune CLI started without proxy node, outbound=direct");
    }

    tracing::info!(
        "Listening: SOCKS5={}:{}, HTTP={}:{}",
        config.listen_addr,
        config.socks_port,
        config.listen_addr,
        config.http_port
    );
    tracing::info!("Press Ctrl+C to stop");

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received");
    service.stop().await;

    Ok(())
}

fn parse_config_path() -> Result<String> {
    match env::args().nth(1) {
        Some(path) => Ok(path),
        None => bail!("usage: xtune-cli <config.yaml>"),
    }
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
    use xtune_core::{Node, TransportConfig, TransportType};

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
