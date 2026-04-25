use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::InputState;
use gpui_component::tag::{Tag, TagVariant};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::log_buffer::SharedLogBuffer;

// Keyboard shortcut actions
actions!(
    xtune,
    [
        ToggleProxy,
        SwitchToHome,
        SwitchToNodes,
        SwitchToConfig,
        SwitchToRules,
        SwitchToLogs,
        SwitchToSettings,
        TestAllLatency,
    ]
);
use xtune_core::config::model::{
    AppConfig, Node, ProxyProtocol, RoutingRule, Subscription, TransportConfig, TransportType,
};
use xtune_core::proxy::ProxyStats;
use xtune_core::{
    ProxyMode, ProxyService, Router, RoutingOutbound, RuleSet, SharedOutbound, TunProxy,
    china_direct_ruleset, clear_system_proxy as clear_os_proxy, create_outbound,
    fetch_subscription, get_system_proxy as get_os_proxy, normalize_node_names, parse_proxy_uri,
    resolve_to_ipv4, set_system_proxy as set_os_proxy, setup_tun_routes, system_proxy_supported,
    tun_requirements, tun_supported,
};

// Color palette — refined dark theme with soft contrast
const BG_PRIMARY: u32 = 0x0f1117; // Near-black — main background
const BG_SIDEBAR: u32 = 0x161921; // Slightly lighter — sidebar
const BG_CARD: u32 = 0x1a1e2a; // Subtle card surface
const BG_CARD_HOVER: u32 = 0x222838; // Hover lift
const BORDER_COLOR: u32 = 0x2b3040; // Soft border, barely visible
const ACCENT: u32 = 0x6c8cff; // Calm blue accent
const ACCENT_DIM: u32 = 0x4a6adf; // Dimmer accent for borders/indicators
const TEXT_PRIMARY: u32 = 0xeaedf3; // Off-white — comfortable reading
const TEXT_SECONDARY: u32 = 0x9ba3b5; // Mid-gray — good contrast (AA)
const TEXT_MUTED: u32 = 0x626a7e; // Muted labels
const SUCCESS_COLOR: u32 = 0x5ee6a0; // Soft green
const WARNING_COLOR: u32 = 0xf0c74f; // Warm amber
const DANGER_COLOR: u32 = 0xf07070; // Soft red
const BG_ACCENT_SUBTLE: u32 = 0x1c2236; // Subtle tinted bg for active states

/// Main application state
pub struct AppState {
    active_view: ActiveView,

    // Proxy state
    proxy_running: bool,
    proxy_status: String,
    proxy_validation_status: String,
    proxy_session_id: u64,

    // Proxy routing mode
    proxy_mode: ProxyMode,

    // Node management
    nodes: Vec<Node>,
    selected_node: Option<usize>,
    active_proxy_node: Option<usize>,

    // Input states
    import_url_input: Entity<InputState>,
    listen_addr_input: Entity<InputState>,
    socks_port_input: Entity<InputState>,
    http_port_input: Entity<InputState>,

    // Import status
    import_status: String,
    settings_status: String,
    rules_status: String,

    // Settings (applied values)
    listen_addr: String,
    socks_port: u16,
    http_port: u16,
    system_proxy_enabled: bool,
    system_proxy_status: String,
    system_proxy_managed_by_app: bool,

    // Proxy stats
    proxy_stats: Option<ProxyStats>,

    // Tokio runtime handle
    tokio_handle: tokio::runtime::Handle,

    // Proxy shutdown
    proxy_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // TUN mode
    tun_enabled: bool,
    tun_status: String,
    tun_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // Rules management
    rules: Vec<RoutingRule>,
    rule_type_input: Entity<InputState>,
    rule_pattern_input: Entity<InputState>,
    rule_target_input: Entity<InputState>,

    // Node search filter
    node_filter: String,
    node_filter_input: Entity<InputState>,

    // Nodes currently being latency-tested
    latency_testing: std::collections::HashSet<usize>,
    // Semaphore to cap concurrent latency tests and avoid network saturation
    latency_semaphore: std::sync::Arc<tokio::sync::Semaphore>,

    // Manual node URI input
    node_uri_input: Entity<InputState>,

    // Rule editing
    editing_rule_index: Option<usize>,

    // Log viewer
    log_buffer: SharedLogBuffer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActiveView {
    Home,
    Nodes,
    Config,
    Rules,
    Settings,
    Logs,
}

fn current_system_proxy_state() -> (bool, String) {
    if !system_proxy_supported() {
        return (false, "Unsupported on this platform".to_string());
    }

    match get_os_proxy() {
        Ok(proxy) if proxy.enabled => (
            true,
            format!(
                "Enabled: {}:{} (bypass: {})",
                proxy.host, proxy.port, proxy.bypass
            ),
        ),
        Ok(_) => (false, "Disabled".to_string()),
        Err(err) => (false, format!("Error: {:#}", err)),
    }
}

fn rule_mode_ruleset(rules: &[RoutingRule]) -> RuleSet {
    if rules.is_empty() {
        china_direct_ruleset()
    } else {
        RuleSet::from_config(rules)
    }
}

fn rule_mode_summary(rules: &[RoutingRule]) -> String {
    if rules.is_empty() {
        "Built-in China direct rules".to_string()
    } else {
        format!("{} custom rule(s)", rules.len())
    }
}

fn status_text_color(message: &str) -> u32 {
    if message.starts_with('✓') {
        SUCCESS_COLOR
    } else if message.starts_with('✗') || message.starts_with('❌') {
        DANGER_COLOR
    } else if message.starts_with('⚠') {
        WARNING_COLOR
    } else {
        TEXT_SECONDARY
    }
}

impl AppState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        tokio_handle: tokio::runtime::Handle,
        log_buffer: SharedLogBuffer,
    ) -> Self {
        let mut persisted = load_gui_state().unwrap_or_else(|| {
            let default_config = AppConfig::default();
            // Create default config on first launch
            let _ = save_gui_state(&default_config);
            default_config
        });
        if normalize_node_names(&mut persisted.nodes) {
            let _ = save_gui_state(&persisted);
        }
        let selected_node = persisted
            .active_node
            .filter(|index| *index < persisted.nodes.len());
        let import_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://example.com/subscribe?token=...")
        });
        let listen_addr_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("127.0.0.1")
                .default_value(persisted.listen_addr.clone())
        });
        let socks_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1080")
                .default_value(persisted.socks_port.to_string())
        });
        let http_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1087")
                .default_value(persisted.http_port.to_string())
        });
        let (system_proxy_enabled, system_proxy_status) = current_system_proxy_state();
        let rule_type_input = cx.new(|cx| InputState::new(window, cx).placeholder("domain-suffix"));
        let rule_pattern_input = cx.new(|cx| InputState::new(window, cx).placeholder("google.com"));
        let rule_target_input = cx.new(|cx| InputState::new(window, cx).placeholder("proxy"));
        let node_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search nodes..."));
        let node_uri_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("vless://... or vmess://... or ss://... or trojan://...")
        });

        Self {
            active_view: ActiveView::Home,
            proxy_running: false,
            proxy_status: "Disconnected".to_string(),
            proxy_validation_status: "Not validated".to_string(),
            proxy_session_id: 0,
            proxy_mode: persisted.proxy_mode.clone(),
            nodes: persisted.nodes.clone(),
            selected_node,
            active_proxy_node: None,
            import_url_input,
            listen_addr_input,
            socks_port_input,
            http_port_input,
            import_status: if persisted.nodes.is_empty() {
                String::new()
            } else {
                format!("✓ Loaded {} saved nodes", persisted.nodes.len())
            },
            settings_status: String::new(),
            rules_status: String::new(),
            listen_addr: persisted.listen_addr,
            socks_port: persisted.socks_port,
            http_port: persisted.http_port,
            system_proxy_enabled,
            system_proxy_status,
            system_proxy_managed_by_app: false,
            proxy_stats: None,
            tokio_handle,
            proxy_stop_tx: None,
            tun_enabled: false,
            tun_status: "Disabled".to_string(),
            tun_stop_tx: None,
            rules: persisted.rules.clone(),
            rule_type_input,
            rule_pattern_input,
            rule_target_input,
            node_filter: String::new(),
            node_filter_input,
            latency_testing: std::collections::HashSet::new(),
            latency_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(5)),
            node_uri_input,
            editing_rule_index: None,
            log_buffer,
        }
    }

    fn set_view(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        self.active_view = view;
        cx.notify();
    }

    // === Proxy Control ===

    fn toggle_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            self.stop_proxy(cx);
        } else {
            self.start_proxy(cx);
        }
    }

    fn start_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            return;
        }

        if !self.nodes.is_empty() && self.selected_node.is_none() {
            self.proxy_status = "Please select a node first".to_string();
            cx.notify();
            return;
        }

        let selected_index = self.selected_node;
        let node = selected_index.and_then(|i| self.nodes.get(i).cloned());
        let node_name = node
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "Direct".to_string());

        let outbound = match &node {
            Some(n) => match create_outbound(n) {
                Ok(o) => {
                    tracing::info!("Proxy outbound: {} (node: {})", o.0.name(), n.name);
                    o
                }
                Err(e) => {
                    self.proxy_status = format!("✗ Outbound error: {:#}", e);
                    cx.notify();
                    return;
                }
            },
            None => {
                tracing::info!("Proxy outbound: direct (no node selected)");
                SharedOutbound::direct()
            }
        };

        // Wrap outbound based on proxy mode
        let final_outbound = match self.proxy_mode {
            ProxyMode::Global => {
                tracing::info!("Proxy mode: Global");
                outbound
            }
            ProxyMode::Rule => {
                tracing::info!("Proxy mode: Rule ({})", rule_mode_summary(&self.rules));
                let ruleset = rule_mode_ruleset(&self.rules);
                let router = std::sync::Arc::new(Router::new(ruleset));
                let routing = RoutingOutbound::new(router, outbound);
                SharedOutbound(std::sync::Arc::new(routing))
            }
            ProxyMode::Direct => {
                tracing::info!("Proxy mode: Direct (no proxy)");
                SharedOutbound::direct()
            }
        };

        let service = ProxyService::with_outbound(final_outbound);
        let stats = service.stats().clone();

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        self.proxy_session_id += 1;
        let session_id = self.proxy_session_id;
        self.proxy_stop_tx = Some(stop_tx);
        self.proxy_stats = Some(stats);
        self.proxy_running = true;
        self.proxy_status = "Connecting...".to_string();
        self.proxy_validation_status = "Waiting for local proxy startup".to_string();
        self.active_proxy_node = None;
        cx.notify();

        let listen_addr = self.listen_addr.clone();
        let socks_port = self.socks_port;
        let http_port = self.http_port;
        let handle = self.tokio_handle.clone();

        cx.spawn(async move |weak, cx| {
            let service_listen_addr = listen_addr.clone();
            let join = handle.spawn(async move {
                let mut service = service;
                if let Err(err) = service
                    .start(&service_listen_addr, socks_port, http_port)
                    .await
                {
                    let _ = ready_tx.send(Err(err.to_string()));
                    return Err(err);
                }
                let _ = ready_tx.send(Ok(()));
                // Wait for stop signal
                let _ = stop_rx.await;
                service.stop().await;
                Ok::<_, anyhow::Error>(())
            });

            let ready_ok = match ready_rx.await {
                Ok(Ok(())) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        if this.proxy_session_id != session_id {
                            return;
                        }
                        this.active_proxy_node = selected_index;
                        this.proxy_status = format!("Connected — {}", node_name);
                        this.proxy_validation_status =
                            "Verifying proxy reachability...".to_string();
                        match set_os_proxy(&this.listen_addr, this.http_port) {
                            Ok(()) => {
                                this.system_proxy_managed_by_app = true;
                                this.refresh_system_proxy_status(None, cx);
                            }
                            Err(err) => {
                                this.system_proxy_managed_by_app = false;
                                this.refresh_system_proxy_status(Some(err.to_string()), cx);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                    true
                }
                Ok(Err(err)) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        if this.proxy_session_id != session_id {
                            return;
                        }
                        this.proxy_running = false;
                        this.proxy_stop_tx = None;
                        this.proxy_stats = None;
                        this.active_proxy_node = None;
                        this.proxy_status = format!("✗ Start failed: {:#}", err);
                        this.proxy_validation_status = "Not validated".to_string();
                        cx.notify();
                    })
                    .ok();
                    false
                }
                Err(_) => false,
            };

            let mut validation_task = if ready_ok {
                let addr_for_check = listen_addr.clone();
                Some(handle.spawn(async move {
                    verify_local_http_proxy(
                        &addr_for_check,
                        http_port,
                        std::time::Duration::from_secs(6),
                    )
                    .await
                }))
            } else {
                None
            };

            // Wait for proxy to stop, but do not let validation block disconnect.
            let mut join = join;
            let result = loop {
                if let Some(task) = validation_task.as_mut() {
                    tokio::select! {
                        proxy_check = task => {
                            let proxy_check = proxy_check
                                .map_err(|e| anyhow::anyhow!("task join error: {}", e))
                                .and_then(|r| r);
                            weak.update(cx, |this: &mut AppState, cx| {
                                if this.proxy_session_id != session_id || !this.proxy_running {
                                    return;
                                }
                                this.proxy_validation_status = match proxy_check {
                                    Ok(summary) => format!("✓ {}", summary),
                                    Err(err) => format!("✗ {:#}", err),
                                };
                                cx.notify();
                            })
                            .ok();
                            validation_task = None;
                        }
                        result = &mut join => {
                            if let Some(task) = validation_task.take() {
                                task.abort();
                            }
                            break result;
                        }
                    }
                } else {
                    break join.await;
                }
            };

            weak.update(cx, |this: &mut AppState, cx| {
                if this.proxy_session_id != session_id {
                    return;
                }
                if this.system_proxy_managed_by_app {
                    match clear_os_proxy() {
                        Ok(()) => {
                            this.system_proxy_managed_by_app = false;
                            this.refresh_system_proxy_status(None, cx);
                        }
                        Err(err) => {
                            this.refresh_system_proxy_status(Some(err.to_string()), cx);
                        }
                    }
                }
                this.proxy_running = false;
                this.proxy_stop_tx = None;
                this.proxy_stats = None;
                this.active_proxy_node = None;
                match result {
                    Ok(Ok(())) => {
                        this.proxy_status = "Disconnected".to_string();
                        this.proxy_validation_status = "Not validated".to_string();
                    }
                    Ok(Err(e)) => this.proxy_status = format!("✗ {:#}", e),
                    Err(e) => this.proxy_status = format!("✗ Task error: {}", e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn stop_proxy(&mut self, cx: &mut Context<Self>) {
        // Stop TUN first if active
        if self.tun_enabled {
            self.stop_tun(cx);
        }
        if self.system_proxy_managed_by_app {
            match clear_os_proxy() {
                Ok(()) => {
                    self.system_proxy_managed_by_app = false;
                    self.refresh_system_proxy_status(None, cx);
                }
                Err(err) => {
                    self.refresh_system_proxy_status(Some(err.to_string()), cx);
                }
            }
        }
        if let Some(tx) = self.proxy_stop_tx.take() {
            let _ = tx.send(());
        }
        self.proxy_status = "Disconnecting...".to_string();
        self.proxy_validation_status = "Stopping local proxy".to_string();
        cx.notify();
    }

    fn restart_proxy_with_current_state(&mut self, cx: &mut Context<Self>) {
        if !self.proxy_running {
            return;
        }

        let restore_tun = self.tun_enabled;
        let handle = self.tokio_handle.clone();
        self.stop_proxy(cx);

        cx.spawn(async move |weak, cx| {
            let mut proxy_started = false;
            for _ in 0..20 {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    })
                    .await
                    .ok();

                proxy_started = weak
                    .update(cx, |this: &mut AppState, cx| {
                        if this.proxy_running {
                            false
                        } else {
                            this.start_proxy(cx);
                            true
                        }
                    })
                    .unwrap_or(false);

                if proxy_started {
                    break;
                }
            }

            if !restore_tun || !proxy_started {
                return;
            }

            for _ in 0..20 {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    })
                    .await
                    .ok();

                let tun_started = weak
                    .update(cx, |this: &mut AppState, cx| {
                        if this.tun_enabled || !this.proxy_running {
                            false
                        } else {
                            this.start_tun(cx);
                            true
                        }
                    })
                    .unwrap_or(false);

                if tun_started {
                    break;
                }
            }
        })
        .detach();
    }

    // === Import ===

    fn import_subscription(&mut self, cx: &mut Context<Self>) {
        let url = self.import_url_input.read(cx).value().to_string();
        if url.trim().is_empty() {
            self.import_status = "⚠ Please enter a subscription URL".to_string();
            cx.notify();
            return;
        }

        self.import_status = "⏳ Importing...".to_string();
        cx.notify();

        let handle = self.tokio_handle.clone();
        let sub = Subscription {
            name: "Imported".to_string(),
            url: url.clone(),
            format: "auto".to_string(),
            last_updated: None,
        };

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move { fetch_subscription(&sub).await })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                match result {
                    Ok(Ok(mut new_nodes)) => {
                        normalize_node_names(&mut new_nodes);
                        let first_new_index = this.nodes.len();
                        let count = new_nodes.len();
                        this.nodes.extend(new_nodes);
                        if count > 0 && this.selected_node.is_none() {
                            this.selected_node = Some(first_new_index);
                        }
                        this.import_status =
                            format!("✓ Imported {} nodes (total: {})", count, this.nodes.len());
                        this.persist_gui_state();
                    }
                    Ok(Err(e)) => {
                        this.import_status = format!("✗ Error: {}", e);
                    }
                    Err(e) => {
                        this.import_status = format!("✗ Error: {}", e);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear_nodes(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            self.stop_proxy(cx);
        }
        self.nodes.clear();
        self.selected_node = None;
        self.active_proxy_node = None;
        self.import_status = "Nodes cleared".to_string();
        self.proxy_validation_status = "Not validated".to_string();
        self.persist_gui_state();
        cx.notify();
    }

    // === Latency Test ===

    fn test_node_latency(&mut self, index: usize, cx: &mut Context<Self>) {
        let node = match self.nodes.get(index) {
            Some(n) => n.clone(),
            None => return,
        };

        // Mark as testing and clear previous latency
        self.latency_testing.insert(index);
        if let Some(n) = self.nodes.get_mut(index) {
            n.latency_ms = None;
        }
        cx.notify();

        let handle = self.tokio_handle.clone();
        let semaphore = self.latency_semaphore.clone();

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move {
                    // Acquire semaphore to cap concurrent tests (avoids saturating
                    // the local network when testing many nodes simultaneously).
                    let _permit = semaphore.acquire_owned().await.ok();

                    let outbound =
                        xtune_core::create_outbound(&node).map_err(|e| format!("{:#}", e))?;
                    // Run TCP ping and full HTTP probe concurrently. HTTP gives
                    // protocol-realistic latency; TCP is the fallback if the proxy
                    // probe target is temporarily unreachable.
                    let server = node.server.clone();
                    let port = node.port;
                    let (tcp_result, http_result) = tokio::join!(
                        xtune_core::tcp_latency_test(&server, port, 5),
                        xtune_core::http_latency_test(&outbound, 10),
                    );
                    match http_result {
                        Ok(ms) => Ok(ms),
                        Err(http_err) => match tcp_result {
                            Ok(ms) => Ok(ms),
                            Err(tcp_err) => Err(format!(
                                "proxy probe failed: {:#}; tcp fallback failed: {:#}",
                                http_err, tcp_err
                            )),
                        },
                    }
                })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                this.latency_testing.remove(&index);
                if let Some(n) = this.nodes.get_mut(index) {
                    match result {
                        Ok(Ok(ms)) => n.latency_ms = Some(ms),
                        Ok(Err(_)) | Err(_) => n.latency_ms = Some(9999),
                    }
                }
                this.persist_gui_state();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn test_all_latency(&mut self, cx: &mut Context<Self>) {
        let count = self.nodes.len();
        for i in 0..count {
            self.test_node_latency(i, cx);
        }
    }

    // === Settings ===

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let addr = self.listen_addr_input.read(cx).value().trim().to_string();
        let socks = self.socks_port_input.read(cx).value().trim().to_string();
        let http = self.http_port_input.read(cx).value().trim().to_string();

        if addr.is_empty() {
            self.settings_status = "✗ Listen address cannot be empty".to_string();
            cx.notify();
            return;
        }

        let socks_port = match socks.parse::<u16>() {
            Ok(port) => port,
            Err(_) => {
                self.settings_status = format!("✗ Invalid SOCKS5 port: {}", socks);
                cx.notify();
                return;
            }
        };
        let http_port = match http.parse::<u16>() {
            Ok(port) => port,
            Err(_) => {
                self.settings_status = format!("✗ Invalid HTTP port: {}", http);
                cx.notify();
                return;
            }
        };

        let changed = self.listen_addr != addr
            || self.socks_port != socks_port
            || self.http_port != http_port;

        self.listen_addr = addr;
        self.socks_port = socks_port;
        self.http_port = http_port;
        self.persist_gui_state();
        self.refresh_system_proxy_status(None, cx);

        self.settings_status = if changed {
            if self.proxy_running {
                "✓ Settings applied. Restarting proxy with new ports.".to_string()
            } else {
                "✓ Settings applied".to_string()
            }
        } else {
            "✓ Settings already up to date".to_string()
        };

        if changed && self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    fn enable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match set_os_proxy(&self.listen_addr, self.http_port) {
            Ok(()) => {
                self.system_proxy_managed_by_app = false;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(format!("{:#}", err)), cx),
        }
    }

    fn disable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match clear_os_proxy() {
            Ok(()) => {
                self.system_proxy_managed_by_app = false;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(format!("{:#}", err)), cx),
        }
    }

    fn refresh_system_proxy_status(
        &mut self,
        fallback_error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (enabled, status) = if let Some(msg) = fallback_error {
            (false, format!("Error: {msg}"))
        } else {
            current_system_proxy_state()
        };

        self.system_proxy_enabled = enabled;
        self.system_proxy_status = status;
        cx.notify();
    }

    // === TUN Mode ===

    #[allow(dead_code)]
    fn toggle_tun(&mut self, cx: &mut Context<Self>) {
        if self.tun_enabled {
            self.stop_tun(cx);
        } else {
            self.start_tun(cx);
        }
    }

    fn start_tun(&mut self, cx: &mut Context<Self>) {
        if self.tun_enabled {
            return;
        }
        if !self.proxy_running {
            self.tun_status = "⚠ Start proxy first".to_string();
            cx.notify();
            return;
        }

        // Get proxy server IP for loop avoidance
        let proxy_server_ips: Vec<std::net::Ipv4Addr> = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| resolve_to_ipv4(&n.server))
            .unwrap_or_default();

        // Build outbound from current state
        let node = self.selected_node.and_then(|i| self.nodes.get(i).cloned());
        let outbound = match &node {
            Some(n) => match create_outbound(n) {
                Ok(o) => o,
                Err(e) => {
                    self.tun_status = format!("Error: {}", e);
                    cx.notify();
                    return;
                }
            },
            None => SharedOutbound::direct(),
        };

        // Wrap with routing mode
        let final_outbound = match self.proxy_mode {
            ProxyMode::Global => outbound,
            ProxyMode::Rule => {
                let ruleset = rule_mode_ruleset(&self.rules);
                let router = std::sync::Arc::new(Router::new(ruleset));
                let routing = RoutingOutbound::new(router, outbound);
                SharedOutbound(std::sync::Arc::new(routing))
            }
            ProxyMode::Direct => SharedOutbound::direct(),
        };

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        self.tun_stop_tx = Some(stop_tx);
        self.tun_status = "Preparing TUN driver...".to_string();
        cx.notify();

        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Step 1: ensure wintun.dll (Windows only, no-op elsewhere)
            if !xtune_core::wintun_dll_available() {
                weak.update(cx, |this: &mut AppState, cx| {
                    this.tun_status = "Extracting WinTun driver...".to_string();
                    cx.notify();
                })
                .ok();

                let dll_result = handle.spawn(xtune_core::ensure_wintun_dll()).await;
                match dll_result {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_stop_tx = None;
                            this.tun_status = format!("❌ WinTun install failed: {:#}", e);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(e) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_stop_tx = None;
                            this.tun_status = format!("❌ WinTun task error: {}", e);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                }
            }

            weak.update(cx, |this: &mut AppState, cx| {
                this.tun_status = "Starting TUN...".to_string();
                cx.notify();
            })
            .ok();

            // Step 2: Create TUN device
            let result = handle
                .spawn(async move {
                    // Wrap TUN creation with a timeout to prevent indefinite blocking
                    let tun_result = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                            let tun_proxy = TunProxy::start(final_outbound)?;
                            let route_info = tun_proxy.route_info();
                            let route_guard = setup_tun_routes(&route_info, &proxy_server_ips)?;
                            Ok((tun_proxy, route_guard))
                        }),
                    )
                    .await;

                    match tun_result {
                        Ok(Ok(inner)) => inner,
                        Ok(Err(e)) => Err(anyhow::anyhow!("TUN task panicked: {}", e)),
                        Err(_) => Err(anyhow::anyhow!(
                            "TUN startup timed out (15s). Check permissions and TUN driver."
                        )),
                    }
                })
                .await;

            match result {
                Ok(Ok((tun_proxy, route_guard))) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_enabled = true;
                        this.tun_status = "✅ TUN active — all traffic proxied".to_string();
                        cx.notify();
                    })
                    .ok();

                    // Hold TUN alive until stop signal
                    let _ = stop_rx.await;

                    // Clean up: restore routes then stop TUN
                    route_guard.restore();
                    tun_proxy.stop().await;

                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_enabled = false;
                        this.tun_status = "Disabled".to_string();
                        cx.notify();
                    })
                    .ok();
                }
                Ok(Err(e)) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_stop_tx = None;
                        this.tun_status = format!("❌ {}", e);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_stop_tx = None;
                        this.tun_status = format!("❌ {}", e);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn stop_tun(&mut self, cx: &mut Context<Self>) {
        if let Some(tx) = self.tun_stop_tx.take() {
            let _ = tx.send(());
        }
        self.tun_status = "Stopping TUN...".to_string();
        cx.notify();
    }

    // === Node Management ===

    fn delete_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.nodes.len() {
            return;
        }
        // Stop proxy if deleting the active node
        if self.proxy_running && self.active_proxy_node == Some(index) {
            self.stop_proxy(cx);
        }
        self.nodes.remove(index);
        // Adjust selected/active indices
        if let Some(sel) = self.selected_node {
            if sel == index {
                self.selected_node = None;
            } else if sel > index {
                self.selected_node = Some(sel - 1);
            }
        }
        if let Some(act) = self.active_proxy_node {
            if act == index {
                self.active_proxy_node = None;
            } else if act > index {
                self.active_proxy_node = Some(act - 1);
            }
        }
        self.persist_gui_state();
        cx.notify();
    }

    fn delete_all_nodes(&mut self, cx: &mut Context<Self>) {
        if self.nodes.is_empty() {
            return;
        }
        if self.proxy_running {
            self.stop_proxy(cx);
        }
        self.nodes.clear();
        self.selected_node = None;
        self.active_proxy_node = None;
        self.persist_gui_state();
        cx.notify();
    }

    fn add_node_from_uri(&mut self, cx: &mut Context<Self>) {
        let uri = self.node_uri_input.read(cx).value().trim().to_string();
        if uri.is_empty() {
            self.import_status = "⚠ Please paste a proxy share link".to_string();
            cx.notify();
            return;
        }

        // Support pasting multiple URIs separated by newlines
        let uris: Vec<&str> = uri
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let mut added = 0u32;
        let mut errors = Vec::new();
        for u in &uris {
            match parse_proxy_uri(u) {
                Ok(mut node) => {
                    let normalized = xtune_core::config::model::decode_display_name(&node.name);
                    node.name = normalized;
                    self.nodes.push(node);
                    added += 1;
                }
                Err(e) => {
                    let short = if u.len() > 30 { &u[..30] } else { u };
                    errors.push(format!("{}: {:#}", short, e));
                }
            }
        }

        self.import_status = if errors.is_empty() {
            format!("✓ Added {} node(s) from URI", added)
        } else if added > 0 {
            format!(
                "✓ Added {} node(s), {} failed: {}",
                added,
                errors.len(),
                errors[0]
            )
        } else {
            format!("✗ Failed: {}", errors[0])
        };

        if added > 0 {
            self.persist_gui_state();
        }
        cx.notify();
    }

    fn sort_nodes_by_latency(&mut self, cx: &mut Context<Self>) {
        // Remember selected/active node names before sorting
        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());

        self.nodes
            .sort_by(|a, b| match (a.latency_ms, b.latency_ms) {
                (Some(a_ms), Some(b_ms)) => a_ms.cmp(&b_ms),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });

        // Restore selected/active indices by name
        if let Some(name) = selected_name {
            self.selected_node = self.nodes.iter().position(|n| n.name == name);
        }
        if let Some(name) = active_name {
            self.active_proxy_node = self.nodes.iter().position(|n| n.name == name);
        }
        self.persist_gui_state();
        cx.notify();
    }

    // === Node Selection ===

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);
        self.persist_gui_state();
        cx.notify();
    }

    fn set_proxy_mode(&mut self, mode: ProxyMode, cx: &mut Context<Self>) {
        if self.proxy_mode == mode {
            return;
        }
        self.proxy_mode = mode;
        self.persist_gui_state();

        // If proxy is running, restart with new mode
        if self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    fn activate_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);

        if self.proxy_running {
            self.restart_proxy_with_current_state(cx);
            return;
        }

        self.start_proxy(cx);
    }

    fn persist_gui_state(&self) {
        let config = AppConfig {
            listen_addr: self.listen_addr.clone(),
            socks_port: self.socks_port,
            http_port: self.http_port,
            proxy_mode: self.proxy_mode.clone(),
            nodes: self.nodes.clone(),
            active_node: self.selected_node,
            subscriptions: Vec::new(),
            rules: self.rules.clone(),
        };

        if let Err(err) = save_gui_state(&config) {
            tracing::error!("failed to persist GUI state: {}", err);
        }
    }
}

// === Render ===

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("app-root")
            .key_context("AppState")
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(BG_PRIMARY))
            .font(ui_font())
            .text_color(rgb(TEXT_PRIMARY))
            .on_action(cx.listener(|this, _: &ToggleProxy, _, cx| {
                this.toggle_proxy(cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToHome, _, cx| {
                this.set_view(ActiveView::Home, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToNodes, _, cx| {
                this.set_view(ActiveView::Nodes, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToConfig, _, cx| {
                this.set_view(ActiveView::Config, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToRules, _, cx| {
                this.set_view(ActiveView::Rules, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToLogs, _, cx| {
                this.set_view(ActiveView::Logs, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToSettings, _, cx| {
                this.set_view(ActiveView::Settings, cx);
            }))
            .on_action(cx.listener(|this, _: &TestAllLatency, _, cx| {
                this.test_all_latency(cx);
            }))
            .child(self.render_sidebar(cx))
            .child(self.render_content(cx))
    }
}

// === Sidebar ===

impl AppState {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        div()
            .w(px(220.0))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER_COLOR))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        // Logo area
                        div().px_5().py_5().child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(ACCENT))
                                        .child("⚡"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(TEXT_PRIMARY))
                                                .child("XTune"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(TEXT_MUTED))
                                                .child("Proxy Client"),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        // Nav items
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .px_3()
                            .mt_2()
                            .child(self.nav_item(
                                "Home",
                                "🏠",
                                "Ctrl+1",
                                ActiveView::Home,
                                &active,
                                cx,
                            ))
                            .child(self.nav_item(
                                "Nodes",
                                "📡",
                                "Ctrl+2",
                                ActiveView::Nodes,
                                &active,
                                cx,
                            ))
                            .child(self.nav_item(
                                "Config",
                                "⬇️",
                                "Ctrl+3",
                                ActiveView::Config,
                                &active,
                                cx,
                            ))
                            .child(self.nav_item(
                                "Rules",
                                "📋",
                                "Ctrl+4",
                                ActiveView::Rules,
                                &active,
                                cx,
                            ))
                            .child(self.nav_item(
                                "Settings",
                                "⚙️",
                                "Ctrl+6",
                                ActiveView::Settings,
                                &active,
                                cx,
                            ))
                            .child(self.nav_item(
                                "Logs",
                                "📜",
                                "Ctrl+5",
                                ActiveView::Logs,
                                &active,
                                cx,
                            )),
                    ),
            )
            .child(
                // Bottom info
                div()
                    .px_5()
                    .py_4()
                    .border_t_1()
                    .border_color(rgb(BORDER_COLOR))
                    .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("v0.1.0")),
            )
    }

    fn nav_item(
        &mut self,
        label: &str,
        icon: &str,
        shortcut: &str,
        view: ActiveView,
        active: &ActiveView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = *active == view;

        let bg = if is_active {
            rgb(BG_ACCENT_SUBTLE)
        } else {
            rgb(BG_SIDEBAR)
        };
        let text_color = if is_active {
            rgb(ACCENT)
        } else {
            rgb(TEXT_SECONDARY)
        };
        let indicator_color = if is_active {
            rgb(ACCENT)
        } else {
            rgb(BG_SIDEBAR)
        };

        let tooltip_text = SharedString::from(format!("{} ({})", label, shortcut));
        div()
            .id(SharedString::from(format!("nav-{}", label)))
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(bg)
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BG_ACCENT_SUBTLE)))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip_text.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(view.clone(), cx);
            }))
            .child(
                // Active indicator bar
                div()
                    .w(px(3.0))
                    .h(px(18.0))
                    .rounded(px(1.5))
                    .mr_3()
                    .bg(indicator_color),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(text_color)
                    .child(format!("{} {}", icon, label)),
            )
    }
}

// === Content Router ===

impl AppState {
    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();
        div()
            .id("main-content")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .p_6()
            .child(match active {
                ActiveView::Home => self.render_home(cx).into_any_element(),
                ActiveView::Nodes => self.render_nodes(cx).into_any_element(),
                ActiveView::Config => self.render_config(cx).into_any_element(),
                ActiveView::Rules => self.render_rules(cx).into_any_element(),
                ActiveView::Settings => self.render_settings(cx).into_any_element(),
                ActiveView::Logs => self.render_logs(cx).into_any_element(),
            })
    }
}

// === Home View ===

impl AppState {
    fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        let status_color = if self.proxy_running {
            rgb(SUCCESS_COLOR)
        } else {
            rgb(TEXT_SECONDARY)
        };

        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "(none selected)".to_string());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "(not activated)".to_string());

        let proxy_mode_label = match self.proxy_mode {
            ProxyMode::Global => "Global (all traffic via proxy)",
            ProxyMode::Rule => "Rule mode",
            ProxyMode::Direct => "Direct (no proxy)",
        };
        let rule_mode_detail = if self.proxy_mode == ProxyMode::Rule {
            Some(rule_mode_summary(&self.rules))
        } else {
            None
        };

        let btn_label = if self.proxy_running {
            "⏹  Disconnect"
        } else {
            "▶  Connect"
        };

        let active_conn = self
            .proxy_stats
            .as_ref()
            .map(|s| s.active_connections())
            .unwrap_or(0);
        let total_conn = self
            .proxy_stats
            .as_ref()
            .map(|s| s.total_connections())
            .unwrap_or(0);
        let bytes_up = self
            .proxy_stats
            .as_ref()
            .map(|s| s.bytes_sent())
            .unwrap_or(0);
        let bytes_down = self
            .proxy_stats
            .as_ref()
            .map(|s| s.bytes_received())
            .unwrap_or(0);

        let cur_mode = self.proxy_mode.clone();

        div()
            .flex()
            .flex_col()
            .gap_5()
            // Title
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Dashboard"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.0))
                            .bg(rgb(BG_ACCENT_SUBTLE))
                            .child("v0.1.0"),
                    ),
            )
            // Proxy Mode card
            .child(
                self.card()
                    .child(self.card_title("Proxy Mode"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child({
                                let btn = Button::new("mode-global")
                                    .label("🌐 Global".to_string())
                                    .tooltip("Route all traffic through proxy")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Global, cx);
                                    }));
                                if cur_mode == ProxyMode::Global {
                                    btn.primary()
                                } else {
                                    btn.ghost()
                                }
                            })
                            .child({
                                let btn = Button::new("mode-rule")
                                    .label("🇨🇳 Rule".to_string())
                                    .tooltip("Route traffic based on rules (e.g. China direct)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Rule, cx);
                                    }));
                                if cur_mode == ProxyMode::Rule {
                                    btn.primary()
                                } else {
                                    btn.ghost()
                                }
                            })
                            .child({
                                let btn = Button::new("mode-direct")
                                    .label("⚡ Direct".to_string())
                                    .tooltip("Bypass proxy, connect directly")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Direct, cx);
                                    }));
                                if cur_mode == ProxyMode::Direct {
                                    btn.primary()
                                } else {
                                    btn.ghost()
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .mt_2()
                            .child(proxy_mode_label),
                    )
                    .child(if let Some(detail) = rule_mode_detail {
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .mt_1()
                            .child(detail)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }),
            )
            // System Proxy card
            .child(
                self.card()
                    .child(self.card_title("System Proxy"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child({
                                if self.system_proxy_enabled {
                                    Button::new("sys-proxy-toggle")
                                        .label("🟢 Enabled — Click to Disable".to_string())
                                        .tooltip("Disable system-wide proxy settings")
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.disable_system_proxy(cx);
                                        }))
                                } else {
                                    Button::new("sys-proxy-toggle")
                                        .label("⭕ Disabled — Click to Enable".to_string())
                                        .tooltip("Enable system-wide proxy settings")
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.enable_system_proxy(cx);
                                        }))
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .mt_2()
                            .child(format!("Status: {}", self.system_proxy_status)),
                    ),
            )
            // TUN Mode card
            .child(
                self.card()
                    .child(self.card_title("🔒 TUN Mode"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child({
                                if self.tun_enabled {
                                    Button::new("tun-toggle")
                                        .label("🟢 Disable TUN".to_string())
                                        .tooltip("Stop TUN virtual network interface")
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.stop_tun(cx);
                                        }))
                                } else if tun_supported() {
                                    Button::new("tun-toggle")
                                        .label("⭕ Enable TUN".to_string())
                                        .tooltip("Start TUN virtual network interface for system-wide proxying")
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_tun(cx);
                                        }))
                                } else {
                                    Button::new("tun-toggle")
                                        .label("⛔ TUN not supported on this platform".to_string())
                                        .tooltip("TUN mode requires administrator privileges and platform support")
                                        .ghost()
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .mt_2()
                            .child(format!("{}", self.tun_status)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .mt_1()
                            .child(format!("Requirements: {}", tun_requirements())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .mt_1()
                            .child("Routes all TCP/UDP traffic through the proxy tunnel with built-in DNS interception"),
                    ),
            )
            // Status card
            .child({
                let card = self.card();
                let card = if self.proxy_running {
                    card.border_color(rgb(ACCENT_DIM))
                } else {
                    card
                };
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .rounded_full()
                                        .bg(status_color)
                                        .with_animation(
                                            "status-pulse",
                                            Animation::new(std::time::Duration::from_secs(2))
                                                .repeat()
                                                .with_easing(pulsating_between(0.5, 1.0)),
                                            move |el, delta| el.opacity(delta),
                                        ),
                                )
                                .child({
                                    let is_transitioning = self.proxy_status.contains("...");
                                    let status_text = div()
                                        .text_base()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.proxy_status.clone());
                                    if is_transitioning {
                                        status_text.with_animation(
                                            "status-text-pulse",
                                            Animation::new(std::time::Duration::from_millis(1200))
                                                .repeat()
                                                .with_easing(pulsating_between(0.4, 1.0)),
                                            |el, delta| el.opacity(delta),
                                        ).into_any_element()
                                    } else {
                                        status_text.into_any_element()
                                    }
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_SECONDARY))
                                .font(localized_font())
                                .child(format!("Selected Node: {}", selected_name)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_SECONDARY))
                                .font(localized_font())
                                .child(format!("Active Node: {}", active_name)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(status_text_color(&self.proxy_validation_status)))
                                .child(format!(
                                    "Proxy Validation: {}",
                                    self.proxy_validation_status
                                )),
                        )
                        .child({
                            let is_connecting = self.proxy_status.contains("...");
                            let connect_btn = Button::new("connect-btn")
                                .label(btn_label.to_string())
                                .tooltip("Toggle proxy connection (Ctrl+Shift+C)")
                                .loading(is_connecting)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_proxy(cx);
                                }));
                            if self.proxy_running {
                                connect_btn.ghost()
                            } else {
                                connect_btn.primary()
                            }
                        }),
                )
            })
            // Proxy info card
            .child(
                self.card()
                    .child(self.card_title("Endpoints"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(self.info_row(
                                "SOCKS5",
                                &format!("{}:{}", self.listen_addr, self.socks_port),
                            ))
                            .child(self.info_row(
                                "HTTP",
                                &format!("{}:{}", self.listen_addr, self.http_port),
                            )),
                    ),
            )
            // Stats card
            .child(
                self.card()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .mb_4()
                            .child(self.card_title("Statistics").mb_0())
                            .child(
                                Button::new("refresh-stats")
                                    .label("↻ Refresh".to_string())
                                    .tooltip("Refresh connection statistics")
                                    .ghost()
                                    .on_click(cx.listener(|_this, _, _, cx| {
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_6()
                            .child(self.stat_item("Active", &active_conn.to_string()))
                            .child(self.stat_item("Total", &total_conn.to_string()))
                            .child(self.stat_item("Nodes", &self.nodes.len().to_string()))
                            .child(self.stat_item("↑ Upload", &format_bytes(bytes_up)))
                            .child(self.stat_item("↓ Download", &format_bytes(bytes_down))),
                    ),
            )
            // Protocols card
            .child(
                self.card()
                    .child(self.card_title("Supported Protocols"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(Tag::primary().child("Shadowsocks"))
                            .child(Tag::info().child("VMess"))
                            .child(Tag::success().child("VLESS"))
                            .child(Tag::warning().child("TUIC v5"))
                            .child(Tag::danger().child("Trojan"))
                            .child(Tag::secondary().child("Hysteria2")),
                    ),
            )
    }

    fn card(&self) -> Div {
        div()
            .p_5()
            .rounded_xl()
            .bg(rgb(BG_CARD))
            .border_1()
            .border_color(rgb(BORDER_COLOR))
            .shadow_md()
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)).border_color(rgb(ACCENT_DIM)))
    }

    /// Section title inside a card (e.g. "Proxy Mode", "System Proxy").
    fn card_title(&self, text: &str) -> Div {
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT_PRIMARY))
            .mb_4()
            .child(text.to_string())
    }

    fn info_row(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT_MUTED))
                    .w(px(80.0))
                    .child(format!("{}", label)),
            )
            .child(
                div()
                    .text_sm()
                    .font(localized_font())
                    .text_color(rgb(TEXT_PRIMARY))
                    .child(value.to_string()),
            )
    }

    fn stat_item(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(rgb(BG_ACCENT_SUBTLE))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(ACCENT))
                    .child(value.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child(label.to_string()),
            )
    }
}

// === Nodes View ===

impl AppState {
    fn render_nodes(&mut self, cx: &mut Context<Self>) -> Div {
        let node_count = self.nodes.len();

        // Read and apply filter
        let filter_text = self
            .node_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        if filter_text != self.node_filter {
            self.node_filter = filter_text.clone();
        }
        let filtered_indices: Vec<usize> = if filter_text.is_empty() {
            (0..node_count).collect()
        } else {
            (0..node_count)
                .filter(|&i| {
                    let n = &self.nodes[i];
                    n.name.to_lowercase().contains(&filter_text)
                        || n.server.to_lowercase().contains(&filter_text)
                        || protocol_short_name(&n.protocol)
                            .to_lowercase()
                            .contains(&filter_text)
                })
                .collect()
        };
        let shown_count = filtered_indices.len();
        let node_filter_input = self.node_filter_input.clone();

        let mut content = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Nodes"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                    .child(if filter_text.is_empty() {
                                        format!("{}", node_count)
                                    } else {
                                        format!("{}/{}", shown_count, node_count)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                Button::new("sort-latency-btn")
                                    .label("📊 Sort by Latency".to_string())
                                    .tooltip("Sort nodes by latency (fastest first)")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sort_nodes_by_latency(cx);
                                    })),
                            )
                            .child(
                                Button::new("test-all-btn")
                                    .label("⚡ Test All".to_string())
                                    .tooltip("Test latency for all nodes (Ctrl+T)")
                                    .primary()
                                    .loading(!self.latency_testing.is_empty())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.test_all_latency(cx);
                                    })),
                            )
                            .child(
                                Button::new("delete-all-btn")
                                    .label("🗑 Delete All".to_string())
                                    .tooltip("Delete all nodes")
                                    .danger()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_all_nodes(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .child(gpui_component::input::Input::new(&node_filter_input).w_full()),
            );

        if node_count == 0 {
            content = content.child(
                self.card().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .py_8()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child("No nodes configured"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_MUTED))
                                .child("Go to Config tab to import a subscription"),
                        ),
                ),
            );
        } else {
            let mut list = div().flex().flex_col().gap_1();
            for &i in &filtered_indices {
                list = list.child(self.render_node_item(i, cx));
            }
            content = content.child(list);
            if let Some(index) = self.selected_node {
                if let Some(node) = self.nodes.get(index) {
                    content = content.child(self.render_selected_node_details(node, index));
                }
            }
        }

        content
    }

    fn render_node_item(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let node = &self.nodes[index];
        let is_selected = self.selected_node == Some(index);
        let is_active = self.proxy_running && self.active_proxy_node == Some(index);
        let name = node.name.clone();
        let server = format!("{}:{}", node.server, node.port);
        let protocol_tag = protocol_tag_label(&node.protocol);
        let action_label = if is_active {
            "Active"
        } else if self.proxy_running {
            "Switch"
        } else {
            "Activate"
        };
        let is_testing = self.latency_testing.contains(&index);
        let latency = node
            .latency_ms
            .map(|ms| {
                if ms >= 9999 {
                    "timeout".to_string()
                } else {
                    format!("{}ms", ms)
                }
            })
            .unwrap_or_else(|| "---".to_string());
        let latency_color = node
            .latency_ms
            .map(|ms| {
                if ms < 100 {
                    rgb(SUCCESS_COLOR)
                } else if ms < 300 {
                    rgb(WARNING_COLOR)
                } else {
                    rgb(DANGER_COLOR)
                }
            })
            .unwrap_or(rgb(TEXT_MUTED));

        let bg = if is_active {
            rgb(BG_ACCENT_SUBTLE)
        } else if is_selected {
            rgb(BG_CARD_HOVER)
        } else {
            rgb(BG_CARD)
        };

        let select_indicator_bg = if is_active {
            rgb(ACCENT)
        } else if is_selected {
            rgb(ACCENT_DIM)
        } else {
            rgb(BORDER_COLOR)
        };

        let border = if is_active {
            rgb(ACCENT_DIM)
        } else if is_selected {
            rgb(0x3a4060)
        } else {
            rgb(BORDER_COLOR)
        };

        div()
            .id(SharedString::from(format!("node-{}", index)))
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_3()
            .rounded_xl()
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow_sm()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)).border_color(rgb(ACCENT_DIM)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_node(index, cx);
            }))
            .child(
                // Selection indicator
                div()
                    .w(px(4.0))
                    .h(px(32.0))
                    .rounded(px(2.0))
                    .mr_3()
                    .bg(select_indicator_bg),
            )
            .child(
                // Node info
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font(localized_font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(server),
                    ),
            )
            .child(
                // Protocol tag
                div().mx_2().child(protocol_tag),
            )
            .child(
                // Latency or testing indicator
                if is_testing {
                    div()
                        .w(px(64.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(rgb(ACCENT))
                                .with_animation(
                                    SharedString::from(format!("lat-spin-{}", index)),
                                    Animation::new(std::time::Duration::from_millis(800))
                                        .repeat()
                                        .with_easing(pulsating_between(0.2, 1.0)),
                                    |el, delta| el.opacity(delta),
                                ),
                        )
                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("testing"))
                } else {
                    div()
                        .w(px(64.0))
                        .text_sm()
                        .text_color(latency_color)
                        .child(latency)
                },
            )
            .child({
                let activate_tooltip = if is_active {
                    "Currently active node"
                } else {
                    "Switch to this node"
                };
                let button = Button::new(("activate-node", index))
                    .label(action_label.to_string())
                    .tooltip(activate_tooltip)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_node(index, cx);
                    }));
                if is_active {
                    button.primary()
                } else {
                    button.ghost()
                }
            })
            .child(
                Button::new(("delete-node", index))
                    .label("✕".to_string())
                    .tooltip("Remove this node")
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.delete_node(index, cx);
                    })),
            )
    }
}

// === Config View ===

impl AppState {
    fn render_config(&mut self, cx: &mut Context<Self>) -> Div {
        let import_status = self.import_status.clone();
        let node_count = self.nodes.len();

        // Count nodes by protocol
        let mut ss_count = 0u32;
        let mut vmess_count = 0u32;
        let mut vless_count = 0u32;
        let mut tuic_count = 0u32;
        let mut trojan_count = 0u32;
        let mut hy2_count = 0u32;
        for node in &self.nodes {
            match &node.protocol {
                ProxyProtocol::Shadowsocks { .. } => ss_count += 1,
                ProxyProtocol::VMess { .. } => vmess_count += 1,
                ProxyProtocol::VLess { .. } => vless_count += 1,
                ProxyProtocol::Tuic { .. } => tuic_count += 1,
                ProxyProtocol::Trojan { .. } => trojan_count += 1,
                ProxyProtocol::Hysteria2 { .. } => hy2_count += 1,
            }
        }

        let import_url_input = self.import_url_input.clone();
        let node_uri_input = self.node_uri_input.clone();

        div()
            .flex()
            .flex_col()
            .gap_5()
            // Title
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Configuration"),
            )
            // Import card
            .child(
                self.card()
                    .child(self.card_title("Import Subscription"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                gpui_component::input::Input::new(&import_url_input)
                                    .cleanable(true),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        Button::new("import-btn")
                                            .label("⬇ Import".to_string())
                                            .tooltip("Fetch nodes from subscription URL")
                                            .primary()
                                            .loading(self.import_status.contains("Importing"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_subscription(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("clear-btn")
                                            .label("🗑 Clear Nodes".to_string())
                                            .tooltip("Remove all imported nodes")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.clear_nodes(cx);
                                            })),
                                    ),
                            )
                            .child(if !import_status.is_empty() {
                                let is_importing = import_status.contains("Importing");
                                let status_div = div()
                                    .text_sm()
                                    .text_color(if import_status.starts_with('✓') {
                                        rgb(SUCCESS_COLOR)
                                    } else if import_status.starts_with('✗') {
                                        rgb(DANGER_COLOR)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .child(import_status);
                                if is_importing {
                                    status_div.with_animation(
                                        "import-pulse",
                                        Animation::new(std::time::Duration::from_millis(1000))
                                            .repeat()
                                            .with_easing(pulsating_between(0.3, 1.0)),
                                        |el, delta| el.opacity(delta),
                                    ).into_any_element()
                                } else {
                                    status_div.into_any_element()
                                }
                            } else {
                                div().into_any_element()
                            }),
                    ),
            )
            // Add node by URI card
            .child(
                self.card()
                    .child(self.card_title("Add Node by Share Link"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                gpui_component::input::Input::new(&node_uri_input)
                                    .cleanable(true),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        Button::new("add-uri-btn")
                                            .label("+ Add Node".to_string())
                                            .tooltip("Parse and add node from URI")
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_node_from_uri(cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(TEXT_MUTED))
                                            .child("Supports vless://, vmess://, ss://, trojan://, tuic://, hy2://"),
                                    ),
                            ),
                    ),
            )
            // Node summary card
            .child(
                self.card()
                    .child(self.card_title(&format!("Node Summary — {} total", node_count)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_3()
                            .child(self.protocol_count_item("SS", ss_count, TagVariant::Primary))
                            .child(self.protocol_count_item("VMess", vmess_count, TagVariant::Info))
                            .child(self.protocol_count_item(
                                "VLESS",
                                vless_count,
                                TagVariant::Success,
                            ))
                            .child(self.protocol_count_item(
                                "TUIC",
                                tuic_count,
                                TagVariant::Warning,
                            ))
                            .child(self.protocol_count_item(
                                "Trojan",
                                trojan_count,
                                TagVariant::Danger,
                            ))
                            .child(self.protocol_count_item(
                                "Hy2",
                                hy2_count,
                                TagVariant::Secondary,
                            )),
                    ),
            )
    }

    fn protocol_count_item(&self, name: &str, count: u32, variant: TagVariant) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(Tag::new().with_variant(variant).child(name.to_string()))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(count.to_string()),
            )
    }
}

// === Rules View ===

impl AppState {
    fn add_rule(&mut self, cx: &mut Context<Self>) {
        let rule_type = self.rule_type_input.read(cx).value().to_string();
        let pattern = self.rule_pattern_input.read(cx).value().to_string();
        let target = self.rule_target_input.read(cx).value().to_string();

        if rule_type.trim().is_empty() || pattern.trim().is_empty() || target.trim().is_empty() {
            self.rules_status = "⚠ Rule type, pattern, and target are all required".to_string();
            cx.notify();
            return;
        }

        let valid_types = [
            "domain",
            "domain-suffix",
            "domain-keyword",
            "ip-cidr",
            "geoip",
            "match",
        ];
        let valid_targets = ["direct", "proxy", "reject"];

        if !valid_types.contains(&rule_type.trim()) {
            self.rules_status = format!("✗ Unsupported rule type: {}", rule_type.trim());
            cx.notify();
            return;
        }
        if !valid_targets.contains(&target.trim()) {
            self.rules_status = format!("✗ Unsupported rule target: {}", target.trim());
            cx.notify();
            return;
        }

        let new_rule = RoutingRule {
            rule_type: rule_type.trim().to_string(),
            pattern: pattern.trim().to_string(),
            target: target.trim().to_string(),
        };

        let action_msg = if let Some(edit_idx) = self.editing_rule_index.take() {
            if edit_idx < self.rules.len() {
                self.rules[edit_idx] = new_rule;
                "✓ Rule updated"
            } else {
                self.rules.push(new_rule);
                "✓ Rule added"
            }
        } else {
            self.rules.push(new_rule);
            "✓ Rule added"
        };

        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            format!("{}. Restarting Rule mode to apply changes.", action_msg)
        } else {
            action_msg.to_string()
        };
        self.persist_gui_state();
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    fn start_edit_rule(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.rules.len() {
            return;
        }
        let rule = self.rules[index].clone();
        self.rule_type_input.update(cx, |state, cx| {
            state.set_value(rule.rule_type, window, cx);
        });
        self.rule_pattern_input.update(cx, |state, cx| {
            state.set_value(rule.pattern, window, cx);
        });
        self.rule_target_input.update(cx, |state, cx| {
            state.set_value(rule.target, window, cx);
        });
        self.editing_rule_index = Some(index);
        self.rules_status = format!("Editing rule #{}", index + 1);
        cx.notify();
    }

    fn cancel_edit_rule(&mut self, cx: &mut Context<Self>) {
        self.editing_rule_index = None;
        self.rules_status = String::new();
        cx.notify();
    }

    fn delete_rule(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.rules.len() {
            self.rules.remove(index);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "✓ Rule removed. Restarting Rule mode to apply changes.".to_string()
            } else {
                "✓ Rule removed".to_string()
            };
            self.persist_gui_state();
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    fn move_rule_up(&mut self, index: usize, cx: &mut Context<Self>) {
        if index > 0 && index < self.rules.len() {
            self.rules.swap(index, index - 1);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "✓ Rule order updated. Restarting Rule mode to apply changes.".to_string()
            } else {
                "✓ Rule moved".to_string()
            };
            self.persist_gui_state();
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    fn move_rule_down(&mut self, index: usize, cx: &mut Context<Self>) {
        if index + 1 < self.rules.len() {
            self.rules.swap(index, index + 1);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "✓ Rule order updated. Restarting Rule mode to apply changes.".to_string()
            } else {
                "✓ Rule moved".to_string()
            };
            self.persist_gui_state();
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    fn load_china_rules(&mut self, cx: &mut Context<Self>) {
        // Add the built-in China direct ruleset as explicit rules
        let china_rules = vec![
            RoutingRule {
                rule_type: "geoip".into(),
                pattern: "CN".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "baidu.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "qq.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "taobao.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "aliyun.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "jd.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "163.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "bilibili.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "zhihu.com".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "10.0.0.0/8".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "172.16.0.0/12".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
            },
        ];
        self.rules = china_rules;
        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            "✓ China Direct preset loaded. Restarting Rule mode to apply changes.".to_string()
        } else {
            "✓ China Direct preset loaded".to_string()
        };
        self.persist_gui_state();
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    fn clear_rules(&mut self, cx: &mut Context<Self>) {
        self.rules.clear();
        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            "✓ Custom rules cleared. Falling back to built-in China Direct rules.".to_string()
        } else {
            "✓ Custom rules cleared".to_string()
        };
        self.persist_gui_state();
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    fn render_rules(&mut self, cx: &mut Context<Self>) -> Div {
        let rule_count = self.rules.len();
        let rules_status = self.rules_status.clone();
        let rule_type_input = self.rule_type_input.clone();
        let rule_pattern_input = self.rule_pattern_input.clone();
        let rule_target_input = self.rule_target_input.clone();

        let mut content = div()
            .flex()
            .flex_col()
            .gap_5()
            // Title
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Routing Rules"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                    .child(format!("{}", rule_count)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                Button::new("load-china-rules")
                                    .label("🇨🇳 Load China Direct".to_string())
                                    .tooltip("Load built-in rules to bypass proxy for Chinese sites")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_china_rules(cx);
                                    })),
                            )
                            .child(
                                Button::new("clear-rules")
                                    .label("🗑 Clear All".to_string())
                                    .tooltip("Remove all routing rules")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_rules(cx);
                                    })),
                            ),
                    ),
            )
            // Add/Edit rule card
            .child(
                self.card()
                    .child(
                        self.card_title(if self.editing_rule_index.is_some() {
                            "Edit Rule"
                        } else {
                            "Add Rule"
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("Type"))
                                            .child(gpui_component::input::Input::new(&rule_type_input)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("Pattern"))
                                            .child(gpui_component::input::Input::new(&rule_pattern_input)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("Target"))
                                            .child(gpui_component::input::Input::new(&rule_target_input)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        Button::new("add-rule-btn")
                                            .label(if self.editing_rule_index.is_some() {
                                                "💾 Save Rule".to_string()
                                            } else {
                                                "+ Add Rule".to_string()
                                            })
                                            .tooltip(if self.editing_rule_index.is_some() {
                                                "Save changes to rule"
                                            } else {
                                                "Add a new routing rule"
                                            })
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_rule(cx);
                                            })),
                                    )
                                    .children(if self.editing_rule_index.is_some() {
                                        Some(
                                            Button::new("cancel-edit-btn")
                                                .label("Cancel".to_string())
                                                .tooltip("Discard changes and stop editing")
                                                .ghost()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_edit_rule(cx);
                                                })),
                                        )
                                    } else {
                                        None
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .mt_1()
                                    .child("Types: domain, domain-suffix, domain-keyword, ip-cidr, geoip, match  |  Targets: direct, proxy, reject"),
                            )
                            .child(if !rules_status.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(rgb(status_text_color(&rules_status)))
                                    .child(rules_status)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    ),
            );

        // Rule list
        if rule_count == 0 {
            content = content.child(
                self.card().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .py_8()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child("No routing rules configured"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_MUTED))
                                .child("In Rule mode, built-in China direct rules are used. Add custom rules here to override."),
                        ),
                ),
            );
        } else {
            let mut list = div().flex().flex_col().gap_1();
            for i in 0..rule_count {
                list = list.child(self.render_rule_item(i, cx));
            }
            content = content.child(list);
        }

        // Info card
        content = content.child(
            self.card()
                .bg(rgb(BG_ACCENT_SUBTLE))
                .border_color(rgb(ACCENT_DIM))
                .child(self.card_title("ℹ️ How Rules Work"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("• Rules are evaluated top-to-bottom; first match wins"))
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("• \"direct\" = connect without proxy, \"proxy\" = use selected node, \"reject\" = block"))
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("• Rule mode uses your custom rules when present, otherwise it falls back to the built-in China Direct profile"))
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("• Use ▲▼ arrows to reorder rules")),
                ),
        );

        content
    }

    fn render_rule_item(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let rule = &self.rules[index];
        let type_tag = match rule.rule_type.as_str() {
            "domain" => Tag::primary().child("Domain"),
            "domain-suffix" => Tag::info().child("Suffix"),
            "domain-keyword" => Tag::warning().child("Keyword"),
            "ip-cidr" => Tag::success().child("CIDR"),
            "geoip" => Tag::danger().child("GeoIP"),
            "match" => Tag::secondary().child("Match"),
            _ => Tag::secondary().child(rule.rule_type.clone()),
        };
        let target_color = match rule.target.as_str() {
            "direct" => rgb(SUCCESS_COLOR),
            "proxy" => rgb(ACCENT),
            "reject" => rgb(DANGER_COLOR),
            _ => rgb(TEXT_SECONDARY),
        };
        let pattern = rule.pattern.clone();
        let target = rule.target.clone();

        div()
            .id(SharedString::from(format!("rule-{}", index)))
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_2()
            .rounded_xl()
            .bg(rgb(BG_CARD))
            .border_1()
            .border_color(rgb(BORDER_COLOR))
            .shadow_sm()
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)).border_color(rgb(ACCENT_DIM)))
            .child(
                div()
                    .w(px(20.0))
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child(format!("#{}", index + 1)),
            )
            .child(div().mx_2().child(type_tag))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font(localized_font())
                    .child(pattern),
            )
            .child(
                div()
                    .w(px(60.0))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(target_color)
                    .child(target),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(
                        Button::new(("rule-up", index))
                            .label("▲".to_string())
                            .tooltip("Move rule up (higher priority)")
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_rule_up(index, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-down", index))
                            .label("▼".to_string())
                            .tooltip("Move rule down (lower priority)")
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_rule_down(index, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-edit", index))
                            .label("✎".to_string())
                            .tooltip("Edit this rule")
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_edit_rule(index, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-del", index))
                            .label("✕".to_string())
                            .tooltip("Delete this rule")
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_rule(index, cx);
                            })),
                    ),
            )
    }
}

// === Logs View ===

impl AppState {
    fn render_logs(&mut self, cx: &mut Context<Self>) -> Div {
        let entries: Vec<crate::log_buffer::LogEntry> = self
            .log_buffer
            .lock()
            .map(|buf| buf.iter().rev().cloned().collect())
            .unwrap_or_default();

        let count = entries.len();

        let mut content = div().flex().flex_col().gap_5().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Logs"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.0))
                                .bg(rgb(BG_ACCENT_SUBTLE))
                                .child(format!("{}", count)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            Button::new("refresh-logs")
                                .label("🔄 Refresh".to_string())
                                .tooltip("Refresh log display")
                                .ghost()
                                .on_click(cx.listener(|_this, _, _, cx| {
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("clear-logs")
                                .label("🗑 Clear".to_string())
                                .tooltip("Clear all log entries")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Ok(mut buf) = this.log_buffer.lock() {
                                        buf.clear();
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        );

        if entries.is_empty() {
            content = content.child(
                self.card().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .py_8()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child("No log entries yet"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_MUTED))
                                .child("Logs will appear here as the proxy operates"),
                        ),
                ),
            );
        } else {
            let mut list = div()
                .flex()
                .flex_col()
                .gap_0p5()
                .p_4()
                .rounded_xl()
                .bg(rgb(0x0c0e14))
                .border_1()
                .border_color(rgb(BORDER_COLOR));

            for (log_idx, entry) in entries.iter().take(200).enumerate() {
                let (level_label, level_color) = match entry.level {
                    tracing::Level::ERROR => ("ERR", DANGER_COLOR),
                    tracing::Level::WARN => ("WRN", WARNING_COLOR),
                    tracing::Level::INFO => ("INF", SUCCESS_COLOR),
                    tracing::Level::DEBUG => ("DBG", TEXT_MUTED),
                    tracing::Level::TRACE => ("TRC", TEXT_MUTED),
                };

                // Shorten target: e.g. "xtune_core::proxy::tun" → "proxy::tun"
                let short_target = entry
                    .target
                    .strip_prefix("xtune_core::")
                    .or_else(|| entry.target.strip_prefix("xtune_gui::"))
                    .or_else(|| entry.target.strip_prefix("xtune::"))
                    .unwrap_or(&entry.target);

                list = list.child(
                    div()
                        .id(SharedString::from(format!("log-{}", log_idx)))
                        .flex()
                        .flex_row()
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.0))
                        .hover(|s| s.bg(rgb(BG_CARD)))
                        .child(
                            div()
                                .w(px(30.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(level_color))
                                .child(level_label.to_string()),
                        )
                        .child(
                            div()
                                .w(px(140.0))
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .overflow_x_hidden()
                                .child(short_target.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .overflow_x_hidden()
                                .child(entry.message.clone()),
                        ),
                );
            }

            content = content.child(list);
        }

        content
    }
}

// === Settings View ===

impl AppState {
    fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let listen_addr_input = self.listen_addr_input.clone();
        let socks_port_input = self.socks_port_input.clone();
        let http_port_input = self.http_port_input.clone();
        let settings_status = self.settings_status.clone();

        div()
            .flex()
            .flex_col()
            .gap_5()
            // Title
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Settings"),
            )
            // Proxy settings card
            .child(
                self.card()
                    .child(self.card_title("Proxy Settings"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.setting_row("Listen Address", &listen_addr_input))
                            .child(self.setting_row("SOCKS5 Port", &socks_port_input))
                            .child(self.setting_row("HTTP Port", &http_port_input))
                            .child(
                                Button::new("apply-settings")
                                    .label("💾 Apply Settings".to_string())
                                    .tooltip("Save and apply proxy settings")
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_settings(cx);
                                    })),
                            )
                            .child(if !settings_status.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(rgb(status_text_color(&settings_status)))
                                    .child(settings_status)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(format!(
                                "Current: {}:{} (SOCKS5) / {}:{} (HTTP)",
                                self.listen_addr, self.socks_port, self.listen_addr, self.http_port
                            )))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        Button::new("enable-system-proxy")
                                            .label(if self.system_proxy_enabled {
                                                "🔄 Update System Proxy".to_string()
                                            } else {
                                                "🌐 Set System Proxy".to_string()
                                            })
                                            .tooltip("Configure OS to use xtune as system proxy")
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.enable_system_proxy(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("disable-system-proxy")
                                            .label("🧹 Clear System Proxy".to_string())
                                            .tooltip("Remove xtune from OS proxy settings")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.disable_system_proxy(cx);
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(format!("System Proxy: {}", self.system_proxy_status)),
                            ),
                    ),
            )
            .child(
                self.card()
                    .child(self.card_title("System Proxy Status"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.info_row(
                                "Selected",
                                if self.system_proxy_managed_by_app {
                                    "XTune local HTTP proxy"
                                } else if self.system_proxy_enabled {
                                    "External proxy"
                                } else {
                                    "Disabled"
                                },
                            ))
                            .child(self.info_row("HTTP Target", &format!("{}:{}", self.listen_addr, self.http_port)))
                            .child(self.info_row("SOCKS5 Target", &format!("{}:{}", self.listen_addr, self.socks_port)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("System proxy uses the local HTTP endpoint because desktop environments generally consume HTTP proxy settings directly."),
                            ),
                    ),
            )
            // About card
            .child(
                self.card()
                    .child(self.card_title("About"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_sm().child("XTune v0.1.0"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("A cross-platform Rust proxy client"),
                            )
                            .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child(
                                "Supports: Shadowsocks, VMess, VLESS, TUIC v5, Trojan, Hysteria2",
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("Compatible with shoes server"),
                            ),
                    ),
            )
    }

    fn setting_row(&self, label: &str, input: &Entity<InputState>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .child(
                div()
                    .w(px(130.0))
                    .text_sm()
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .child(gpui_component::input::Input::new(input)),
            )
    }

    fn render_selected_node_details(&self, node: &Node, index: usize) -> Div {
        let status = if self.proxy_running && self.active_proxy_node == Some(index) {
            "Active"
        } else if self.selected_node == Some(index) {
            "Selected"
        } else {
            "Idle"
        };

        let latency_str = node
            .latency_ms
            .map(|ms| {
                if ms >= 9999 {
                    "timeout".to_string()
                } else {
                    format!("{} ms", ms)
                }
            })
            .unwrap_or_else(|| "Not tested".to_string());

        self.card()
            .child(
                self.card_title(&format!("Node Details — {}", node.name))
                    .font(localized_font()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.info_row("Status", status))
                    .child(self.info_row("Protocol", protocol_name(&node.protocol)))
                    .child(self.info_row("Server", &node.server))
                    .child(self.info_row("Port", &node.port.to_string()))
                    .child(self.info_row("Latency", &latency_str))
                    .child(self.info_row("Transport", &transport_summary(node.transport.as_ref())))
                    .child(self.info_row("Security", &security_summary(node)))
                    .child(self.info_row("Auth", &auth_summary(&node.protocol)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(protocol_note(&node.protocol)),
                    ),
            )
    }
}

// === Helpers ===

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn ui_font() -> Font {
    let mut font = font("Noto Sans CJK SC");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Microsoft YaHei UI".to_string(),
        "Microsoft YaHei".to_string(),
        "PingFang SC".to_string(),
        "Hiragino Sans GB".to_string(),
        "WenQuanYi Micro Hei".to_string(),
        "Source Han Sans SC".to_string(),
        "Source Han Sans CN".to_string(),
        "Noto Sans CJK SC".to_string(),
        "Noto Sans CJK TC".to_string(),
        "Noto Sans CJK JP".to_string(),
        "Noto Sans CJK KR".to_string(),
        ".SystemUIFont".to_string(),
        "Noto Sans".to_string(),
    ]));
    font
}

fn localized_font() -> Font {
    ui_font()
}

async fn verify_local_http_proxy(
    listen_addr: &str,
    http_port: u16,
    timeout: std::time::Duration,
) -> anyhow::Result<String> {
    // Run both targets in parallel per attempt; take the first success.
    // This halves the worst-case wait compared to sequential probing.
    let mut errors = Vec::new();

    for attempt in 1..=2u32 {
        let (r1, r2) = tokio::join!(
            verify_local_http_proxy_once(
                listen_addr,
                http_port,
                timeout,
                "www.gstatic.com",
                "/generate_204"
            ),
            verify_local_http_proxy_once(
                listen_addr,
                http_port,
                timeout,
                "cp.cloudflare.com",
                "/generate_204"
            ),
        );
        match (r1, r2) {
            (Ok(summary), _) | (_, Ok(summary)) => {
                return if attempt == 1 {
                    Ok(summary)
                } else {
                    Ok(format!("{} (after retry)", summary))
                };
            }
            (Err(e1), Err(e2)) => {
                errors.push(format!(
                    "attempt {}: gstatic: {}; cloudflare: {}",
                    attempt, e1, e2
                ));
            }
        }
    }

    anyhow::bail!(
        "internet reachability check failed after retries: {}",
        errors.join(" | ")
    );
}

async fn verify_local_http_proxy_once(
    listen_addr: &str,
    http_port: u16,
    timeout: std::time::Duration,
    target_host: &str,
    target_path: &str,
) -> anyhow::Result<String> {
    // Use a short timeout for the local TCP connect (proxy should be on localhost)
    let connect_timeout = std::cmp::min(timeout, std::time::Duration::from_secs(2));
    let mut stream = tokio::time::timeout(
        connect_timeout,
        tokio::net::TcpStream::connect((listen_addr, http_port)),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "local proxy not responding on {}:{}",
            listen_addr,
            http_port
        )
    })??;

    let connect_request = format!(
        "CONNECT {0}:443 HTTP/1.1\r\nHost: {0}:443\r\nProxy-Connection: keep-alive\r\n\r\n",
        target_host
    );
    stream.write_all(connect_request.as_bytes()).await?;

    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(timeout, stream.read(&mut buf))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "proxy did not respond within {}s — node may be unreachable or slow",
                timeout.as_secs()
            )
        })??;
    if read == 0 {
        anyhow::bail!("proxy closed connection without a response");
    }

    let response = String::from_utf8_lossy(&buf[..read]);
    let status_line = response
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !status_line.starts_with("HTTP/1.") {
        anyhow::bail!("unexpected proxy response: {}", status_line);
    }
    if !status_line.contains(" 200 ") {
        anyhow::bail!("tunnel establishment failed: {}", status_line);
    }

    // Phase 2: TLS handshake to verify end-to-end connectivity
    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config));
    let server_name =
        tokio_rustls::rustls::pki_types::ServerName::try_from(target_host.to_string())
            .map_err(|e| anyhow::anyhow!("invalid server name: {}", e))?;

    let tls_stream = match tokio::time::timeout(
        timeout,
        connector.connect(server_name.to_owned(), stream),
    )
    .await
    {
        Ok(Ok(tls_stream)) => tls_stream,
        Ok(Err(err)) => {
            return Ok(format!(
                "Connected — tunnel established via {}, TLS verification inconclusive ({})",
                target_host, err
            ));
        }
        Err(_) => {
            return Ok(format!(
                "Connected — tunnel established via {}, remote verification still warming up",
                target_host
            ));
        }
    };

    let (mut reader, mut writer) = tokio::io::split(tls_stream);
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        target_path, target_host
    );
    writer.write_all(request.as_bytes()).await?;

    let mut resp_buf = vec![0u8; 1024];
    let n = match tokio::time::timeout(timeout, reader.read(&mut resp_buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(err)) => {
            return Ok(format!(
                "Connected — tunnel established via {}, response verification inconclusive ({})",
                target_host, err
            ));
        }
        Err(_) => {
            return Ok(format!(
                "Connected — tunnel established via {}, remote verification still warming up",
                target_host
            ));
        }
    };

    if n == 0 {
        return Ok(format!(
            "Connected — tunnel established via {}, server closed before verification response",
            target_host
        ));
    }

    let resp = String::from_utf8_lossy(&resp_buf[..n]);
    let resp_status = resp.lines().next().unwrap_or_default().trim().to_string();

    if resp_status.contains("204") || resp_status.contains("200") {
        Ok(format!(
            "Connected — internet access verified via {}",
            target_host
        ))
    } else {
        Ok(format!(
            "Connected — tunnel established via {}, server returned: {}",
            target_host, resp_status
        ))
    }
}

fn load_gui_state() -> Option<AppConfig> {
    let path = gui_state_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&content).ok()
}

fn save_gui_state(config: &AppConfig) -> anyhow::Result<()> {
    let path = gui_state_path().ok_or_else(|| anyhow::anyhow!("no GUI config directory found"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_yaml::to_string(config)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn gui_state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("xtune").join("gui-state.yaml"))
}

fn protocol_tag_label(protocol: &ProxyProtocol) -> Tag {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => Tag::primary().child("SS"),
        ProxyProtocol::VMess { .. } => Tag::info().child("VMess"),
        ProxyProtocol::VLess { .. } => Tag::success().child("VLESS"),
        ProxyProtocol::Tuic { .. } => Tag::warning().child("TUIC"),
        ProxyProtocol::Trojan { .. } => Tag::danger().child("Trojan"),
        ProxyProtocol::Hysteria2 { .. } => Tag::secondary().child("Hy2"),
    }
}

fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "Shadowsocks",
        ProxyProtocol::VMess { .. } => "VMess",
        ProxyProtocol::VLess { .. } => "VLESS",
        ProxyProtocol::Tuic { .. } => "TUIC v5",
        ProxyProtocol::Trojan { .. } => "Trojan",
        ProxyProtocol::Hysteria2 { .. } => "Hysteria2",
    }
}

fn protocol_short_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "ss",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hy2",
    }
}

fn transport_summary(transport: Option<&TransportConfig>) -> String {
    match transport {
        None => "TCP".to_string(),
        Some(transport) => {
            let mode = match transport.transport_type {
                TransportType::Tcp => "TCP",
                TransportType::Tls => "TLS",
                TransportType::WebSocket => "WebSocket",
                TransportType::Quic => "QUIC",
                TransportType::Reality => "Reality",
            };

            let mut parts = vec![mode.to_string()];
            if let Some(tls) = &transport.tls {
                if let Some(sni) = &tls.sni {
                    parts.push(format!("SNI {sni}"));
                }
            }
            if let Some(ws) = &transport.ws {
                if let Some(path) = &ws.path {
                    if !path.is_empty() {
                        parts.push(format!("Path {}", path));
                    }
                }
            }
            if let Some(reality) = &transport.reality {
                if let Some(sni) = &reality.sni {
                    parts.push(format!("Server {}", sni));
                }
            }
            parts.join(" · ")
        }
    }
}

fn security_summary(node: &Node) -> String {
    if let Some(transport) = &node.transport {
        if let Some(tls) = &transport.tls {
            return if tls.skip_cert_verify {
                "TLS (skip cert verify)".to_string()
            } else {
                "TLS".to_string()
            };
        }
        if transport.reality.is_some() {
            return "Reality".to_string();
        }
    }

    match node.protocol {
        ProxyProtocol::Tuic { .. } => "QUIC + TLS".to_string(),
        _ => "Default".to_string(),
    }
}

fn auth_summary(protocol: &ProxyProtocol) -> String {
    match protocol {
        ProxyProtocol::Shadowsocks { cipher, .. } => format!("Cipher {cipher}"),
        ProxyProtocol::VMess {
            cipher, alter_id, ..
        } => {
            format!("Cipher {cipher}, alterId {alter_id}")
        }
        ProxyProtocol::VLess { flow, .. } => flow
            .as_ref()
            .map(|flow| format!("Flow {flow}"))
            .unwrap_or_else(|| "UUID auth".to_string()),
        ProxyProtocol::Tuic {
            congestion_control, ..
        } => format!("Congestion {congestion_control}"),
        ProxyProtocol::Trojan { .. } => "Password auth".to_string(),
        ProxyProtocol::Hysteria2 { .. } => "Password auth".to_string(),
    }
}

fn protocol_note(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Tuic { .. } => {
            "TUIC 节点激活后会自动把系统代理指向本地 HTTP 代理端口，用于桌面流量接管。"
        }
        ProxyProtocol::VMess { .. } | ProxyProtocol::VLess { .. } => {
            "VMess / VLESS 的传输与 TLS 细节可在 Transport 一项查看。"
        }
        _ => "选中节点后可直接激活，激活完成后会显示真实的 Active 状态。",
    }
}

#[cfg(test)]
mod tests {
    use super::{Router, rule_mode_ruleset};
    use xtune_core::config::model::RoutingRule;
    use xtune_core::{RouteAction, decode_display_name};

    #[test]
    fn decode_display_name_handles_double_encoded_names() {
        assert_eq!(
            decode_display_name("%25E9%25A9%25AC%25E6%259D%25A5%25E8%25A5%25BF%25E4%25BA%259A"),
            "马来西亚"
        );
    }

    #[test]
    fn rule_mode_ruleset_uses_builtin_rules_when_empty() {
        let router = Router::new(rule_mode_ruleset(&[]));
        assert_eq!(router.route("www.baidu.com", 443), RouteAction::Direct);
    }

    #[test]
    fn rule_mode_ruleset_prefers_custom_rules_when_present() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "reject".to_string(),
            },
            RoutingRule {
                rule_type: "match".to_string(),
                pattern: "*".to_string(),
                target: "proxy".to_string(),
            },
        ];

        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("api.example.com", 443), RouteAction::Reject);
    }
}
