use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Disableable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::InputState;
use gpui_component::tag::Tag;
use gpui_component::{Icon, IconName, Sizable as _, TitleBar};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::log_buffer::SharedLogBuffer;

// Keyboard shortcut actions
actions!(
    vex,
    [
        ToggleProxy,
        SwitchToHome,
        SwitchToNodes,
        SwitchToConfig,
        SwitchToRules,
        SwitchToLogs,
        SwitchToSettings,
        TestAllLatency,
        CycleProxyMode,
    ]
);
use vex_core::config::model::{
    AppConfig, Node, ProxyProtocol, RoutingRule, Subscription, TransportConfig, TransportType,
};
use vex_core::proxy::ProxyStats;
use vex_core::{
    ProxyMode, ProxyService, Router, RoutingOutbound, RuleSet, SharedOutbound, TunProxy,
    china_direct_ruleset, clear_system_proxy as clear_os_proxy, create_outbound,
    fetch_subscription, get_system_proxy as get_os_proxy, normalize_node_names, parse_proxy_uri,
    resolve_to_ipv4, set_system_proxy as set_os_proxy, setup_tun_routes, system_proxy_supported,
    tun_requirements, tun_supported,
};

// Color palette — Zed Pro Onyx-inspired dark theme
const BG_PRIMARY: u32 = 0x1c1c1c; // Main content background
const BG_SIDEBAR: u32 = 0x141414; // Panel / sidebar
const BG_CARD: u32 = 0x242424; // Elevated surface
const BG_CARD_HOVER: u32 = 0x2b2b2b; // Hover lift
const BORDER_COLOR: u32 = 0x333333; // Subtle structural border
const ACCENT: u32 = 0x4c8ef8; // Primary blue accent
const ACCENT_DIM: u32 = 0x3b74d7; // Dimmer accent for indicators
const TEXT_PRIMARY: u32 = 0xc5c5c5; // Main readable text
const TEXT_SECONDARY: u32 = 0x7d868f; // Secondary / meta text
const TEXT_MUTED: u32 = 0x616870; // Muted labels
const SUCCESS_COLOR: u32 = 0x56b16c; // Green
const WARNING_COLOR: u32 = 0xc8a459; // Amber
const DANGER_COLOR: u32 = 0xe06c75; // Red
const BG_ACCENT_SUBTLE: u32 = 0x1e2b42; // Active item tinted bg
const BG_STATUSBAR: u32 = 0x111111; // Status bar

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
    /// Snapshot for computing bandwidth speed: (bytes_up, bytes_down, instant)
    prev_bandwidth_snapshot: Option<(u64, u64, std::time::Instant)>,
    /// Computed upload speed in bytes/sec
    upload_speed_bps: f64,
    /// Computed download speed in bytes/sec
    download_speed_bps: f64,
    /// Ring buffer of recent upload speeds (bytes/sec), sampled ~1/s; last 60 samples
    upload_history: std::collections::VecDeque<f64>,
    /// Ring buffer of recent download speeds (bytes/sec), sampled ~1/s; last 60 samples
    download_history: std::collections::VecDeque<f64>,

    // Tokio runtime handle
    tokio_handle: tokio::runtime::Handle,

    // Proxy shutdown
    proxy_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Cancels in-flight proxy reachability validation immediately on disconnect.
    proxy_validation_cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // TUN mode
    tun_enabled: bool,
    /// True between start_tun() spawning the task and tun_enabled being set.
    /// Guards against a second start_tun() call racing during the 15 s startup.
    tun_starting: bool,
    tun_status: String,
    tun_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // Rules management
    rules: Vec<RoutingRule>,
    rule_pattern_input: Entity<InputState>,
    // Selected values for type/target button groups
    rule_type_sel: String,
    rule_target_sel: String,

    // Node search filter
    node_filter: String,
    node_filter_input: Entity<InputState>,
    // Protocol chip filter — None means "All", Some("ss") etc. restricts to one protocol
    protocol_filter: Option<&'static str>,
    // Tag filter — None means show all nodes, Some(tag) restricts to nodes with that tag
    tag_filter: Option<String>,
    // Input for adding a tag to the selected node
    node_tag_input: Entity<InputState>,

    // Batch-selected node indices (for delete/share operations)
    selected_node_indices: std::collections::HashSet<usize>,

    // Nodes currently being latency-tested
    latency_testing: std::collections::HashSet<usize>,
    // Nodes whose most recent latency test completed with a failure
    latency_failed: std::collections::HashSet<usize>,
    // Semaphore to cap concurrent latency tests and avoid network saturation
    latency_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    // Debounce flag: a persist task is already scheduled (avoid 50+ writes during batch test)
    pending_persist: bool,
    // Batch test tracking: count of batch-initiated tests still in-flight
    pending_latency_batch: usize,
    // When true, automatically switch to the fastest node after a batch test completes
    auto_select_best: bool,

    // Saved subscriptions (URL + metadata) for auto-refresh
    subscriptions: Vec<Subscription>,
    // Indices of subscriptions currently being refreshed (shows spinner)
    refreshing_subscriptions: std::collections::HashSet<usize>,

    // Manual node URI input
    node_uri_input: Entity<InputState>,

    // Inline node rename
    node_rename_input: Entity<InputState>,
    editing_node_index: Option<usize>,

    // Rule editing
    editing_rule_index: Option<usize>,

    // Config view active tab
    config_tab: ConfigTab,

    // Settings view active category
    settings_category: SettingsCategory,

    // Log viewer
    log_buffer: SharedLogBuffer,
    log_level_filter: tracing::Level,
    log_filter: String,
    log_filter_input: Entity<InputState>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigTab {
    ImportSubscription,
    AddByUri,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsCategory {
    ProxyConfig,
    SystemProxy,
    About,
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
    let enabled: Vec<RoutingRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
    if enabled.is_empty() {
        china_direct_ruleset()
    } else {
        RuleSet::from_config(&enabled)
    }
}

fn rule_mode_summary(rules: &[RoutingRule]) -> String {
    let enabled = rules.iter().filter(|r| r.enabled).count();
    let total = rules.len();
    if total == 0 {
        "Built-in China direct rules".to_string()
    } else if enabled < total {
        format!("{} rule(s) active, {} disabled", enabled, total - enabled)
    } else {
        format!("{} custom rule(s)", total)
    }
}

fn status_text_color(message: &str) -> u32 {
    if message.starts_with('✓') || message.starts_with('✅') {
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
        let rule_pattern_input = cx.new(|cx| InputState::new(window, cx).placeholder("google.com"));
        let node_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search nodes..."));
        let node_tag_input = cx.new(|cx| InputState::new(window, cx).placeholder("Add tag..."));
        let node_uri_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("vless://... or vmess://... or ss://... or trojan://...")
        });
        let node_rename_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New node name"));
        let log_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter logs..."));

        let mut state = Self {
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
            prev_bandwidth_snapshot: None,
            upload_speed_bps: 0.0,
            download_speed_bps: 0.0,
            upload_history: std::collections::VecDeque::new(),
            download_history: std::collections::VecDeque::new(),
            tokio_handle,
            proxy_stop_tx: None,
            proxy_validation_cancel_tx: None,
            tun_enabled: false,
            tun_starting: false,
            tun_status: "Disabled".to_string(),
            tun_stop_tx: None,
            rules: persisted.rules.clone(),
            rule_pattern_input,
            rule_type_sel: "domain-suffix".to_string(),
            rule_target_sel: "proxy".to_string(),
            node_filter: String::new(),
            node_filter_input,
            protocol_filter: None,
            tag_filter: None,
            node_tag_input,
            latency_testing: std::collections::HashSet::new(),
            latency_failed: std::collections::HashSet::new(),
            latency_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(5)),
            pending_persist: false,
            pending_latency_batch: 0,
            auto_select_best: false,
            selected_node_indices: std::collections::HashSet::new(),
            subscriptions: persisted.subscriptions.clone(),
            refreshing_subscriptions: std::collections::HashSet::new(),
            node_uri_input,
            node_rename_input,
            editing_node_index: None,
            editing_rule_index: None,
            config_tab: ConfigTab::ImportSubscription,
            settings_category: SettingsCategory::ProxyConfig,
            log_buffer,
            log_level_filter: tracing::Level::INFO,
            log_filter: String::new(),
            log_filter_input,
        };
        state.start_auto_refresh(cx);
        state
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
        let (validation_cancel_tx, validation_cancel_rx) = tokio::sync::oneshot::channel::<()>();
        self.proxy_session_id += 1;
        let session_id = self.proxy_session_id;
        self.proxy_stop_tx = Some(stop_tx);
        self.proxy_validation_cancel_tx = Some(validation_cancel_tx);
        self.proxy_stats = Some(stats);
        self.prev_bandwidth_snapshot = None;
        self.upload_speed_bps = 0.0;
        self.download_speed_bps = 0.0;
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
                        this.proxy_validation_cancel_tx = None;
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
                    tokio::select! {
                        result = verify_local_http_proxy(
                            &addr_for_check,
                            http_port,
                            std::time::Duration::from_secs(8),
                        ) => result,
                        _ = validation_cancel_rx => {
                            Err(anyhow::anyhow!("cancelled"))
                        }
                    }
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
                                    // Validation inconclusive ≠ proxy broken; use ⚠ not ✗
                                    // (proxy is connected, specific probe hosts may be blocked)
                                    Err(err) => format!("⚠ Inconclusive — {:#}", err),
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
                // System proxy was already cleared in stop_proxy().  Only
                // handle the case where the proxy service terminated on its
                // own (crash/error) without stop_proxy() being called.
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
                // These are already false/None when stop_proxy() was called;
                // set them here too to handle the self-termination path.
                this.proxy_running = false;
                this.proxy_stop_tx = None;
                this.proxy_validation_cancel_tx = None;
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
        // Cancel validation immediately so it doesn't continue probing after disconnect.
        if let Some(tx) = self.proxy_validation_cancel_tx.take() {
            let _ = tx.send(());
        }
        // Immediately reflect disconnected state in the UI so the user sees
        // the change on the same frame as the click, rather than waiting for
        // the background Tokio task to finish service.stop() and call back.
        // The background task's cleanup callback will still run and update
        // proxy_status to "Disconnected" (or an error string), completing the
        // transition.  These double-sets are harmless; session_id guards
        // prevent any stale callback from clobbering a new session.
        self.proxy_running = false;
        self.proxy_stats = None;
        self.active_proxy_node = None;
        self.proxy_validation_status = "Not validated".to_string();
        self.proxy_status = "Disconnecting...".to_string();
        cx.notify();
    }

    fn restart_proxy_with_current_state(&mut self, cx: &mut Context<Self>) {
        if !self.proxy_running {
            return;
        }

        let restore_tun = self.tun_enabled;
        let handle = self.tokio_handle.clone();
        // stop_proxy() sets proxy_running = false immediately; we only need a
        // brief pause so the OS releases the port before the new bind attempt.
        self.stop_proxy(cx);

        cx.spawn(async move |weak, cx| {
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                })
                .await
                .ok();

            let proxy_started = weak
                .update(cx, |this: &mut AppState, cx| {
                    this.start_proxy(cx);
                    true
                })
                .unwrap_or(false);

            if !restore_tun || !proxy_started {
                return;
            }

            // Wait for the proxy to finish starting before re-enabling TUN.
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
            refresh_interval_hours: 24,
        };

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move { fetch_subscription(&sub).await })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                match result {
                    Ok(Ok(mut new_nodes)) => {
                        normalize_node_names(&mut new_nodes);
                        // Tag nodes with source subscription URL for later refresh tracking
                        for n in &mut new_nodes {
                            n.extra.insert("sub_url".to_string(), url.clone());
                        }
                        let first_new_index = this.nodes.len();
                        let count = new_nodes.len();
                        this.nodes.extend(new_nodes);
                        if count > 0 && this.selected_node.is_none() {
                            this.selected_node = Some(first_new_index);
                        }
                        this.import_status =
                            format!("✓ Imported {} nodes (total: {})", count, this.nodes.len());
                        // Save subscription for auto-refresh (upsert by URL)
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        if let Some(existing) = this.subscriptions.iter_mut().find(|s| s.url == url)
                        {
                            existing.last_updated = Some(now);
                        } else {
                            this.subscriptions.push(Subscription {
                                name: format!("Sub {}", this.subscriptions.len() + 1),
                                url: url.clone(),
                                format: "auto".to_string(),
                                last_updated: Some(now),
                                refresh_interval_hours: 24,
                            });
                        }
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
        self.latency_testing.clear();
        self.latency_failed.clear();
        self.pending_latency_batch = 0;
        self.auto_select_best = false;
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

        // Mark as testing; keep the previous latency value visible while the test
        // runs (cleared only if the new test fails).  Clear the failed flag so the
        // spinner renders instead of "✗" while in-flight.
        self.latency_testing.insert(index);
        self.latency_failed.remove(&index);
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
                        vex_core::create_outbound(&node).map_err(|e| format!("{:#}", e))?;
                    // Run TCP ping and full HTTP probe concurrently.
                    // HTTP gives protocol-realistic latency; TCP is the fallback
                    // if the proxy probe target is temporarily unreachable.
                    // Timeout reduced to 5s (was 10s) to keep batch tests responsive.
                    let server = node.server.clone();
                    let port = node.port;
                    let (tcp_result, http_result) = tokio::join!(
                        vex_core::tcp_latency_test(&server, port, 5),
                        vex_core::http_latency_test(&outbound, 5),
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
                match result {
                    Ok(Ok(ms)) => {
                        this.latency_failed.remove(&index);
                        if let Some(n) = this.nodes.get_mut(index) {
                            n.latency_ms = Some(ms);
                        }
                    }
                    Ok(Err(_)) | Err(_) => {
                        this.latency_failed.insert(index);
                        if let Some(n) = this.nodes.get_mut(index) {
                            n.latency_ms = None;
                        }
                    }
                }
                // Batch test tracking: decrement counter and auto-select best when done
                if this.pending_latency_batch > 0 {
                    this.pending_latency_batch -= 1;
                    if this.pending_latency_batch == 0 && this.auto_select_best {
                        // Find the node with the lowest latency
                        let best = this
                            .nodes
                            .iter()
                            .enumerate()
                            .filter_map(|(i, n)| n.latency_ms.map(|ms| (i, ms)))
                            .min_by_key(|&(_, ms)| ms)
                            .map(|(i, _)| i);
                        if let Some(best_idx) = best {
                            this.activate_node(best_idx, cx);
                        }
                    }
                }
                this.schedule_persist(cx); // debounced: coalesces N node completions → 1 write
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn test_all_latency(&mut self, cx: &mut Context<Self>) {
        let count = self.nodes.len();
        self.pending_latency_batch = 0; // plain test-all doesn't trigger auto-select
        for i in 0..count {
            self.test_node_latency(i, cx);
        }
    }

    /// Test all nodes and automatically switch to the fastest one when done.
    fn test_all_and_select_best(&mut self, cx: &mut Context<Self>) {
        let count = self.nodes.len();
        if count == 0 {
            return;
        }
        self.pending_latency_batch = count;
        self.auto_select_best = true;
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
        if self.tun_enabled || self.tun_starting {
            self.stop_tun(cx);
        } else {
            self.start_tun(cx);
        }
    }

    fn start_tun(&mut self, cx: &mut Context<Self>) {
        // Guard against re-entry: block if already running OR if the startup
        // task is still in-flight (tun_enabled is false during the ~15 s setup).
        if self.tun_enabled || self.tun_starting {
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
        self.tun_starting = true;
        self.tun_status = "Preparing TUN driver...".to_string();
        cx.notify();

        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Step 1: ensure wintun.dll (Windows only, no-op elsewhere)
            if !vex_core::wintun_dll_available() {
                weak.update(cx, |this: &mut AppState, cx| {
                    this.tun_status = "Extracting WinTun driver...".to_string();
                    cx.notify();
                })
                .ok();

                let dll_result = handle.spawn(vex_core::ensure_wintun_dll()).await;
                match dll_result {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_starting = false;
                            this.tun_stop_tx = None;
                            this.tun_status = format!("❌ WinTun install failed: {:#}", e);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(e) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_starting = false;
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
                        this.tun_starting = false;
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
                        this.tun_starting = false;
                        this.tun_stop_tx = None;
                        this.tun_status = format!("❌ {}", e);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_starting = false;
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
                    let normalized = vex_core::config::model::decode_display_name(&node.name);
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

    fn sort_nodes_alphabetically(&mut self, cx: &mut Context<Self>) {
        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());

        self.nodes.sort_by(|a, b| a.name.cmp(&b.name));

        if let Some(name) = selected_name {
            self.selected_node = self.nodes.iter().position(|n| n.name == name);
        }
        if let Some(name) = active_name {
            self.active_proxy_node = self.nodes.iter().position(|n| n.name == name);
        }
        self.persist_gui_state();
        cx.notify();
    }

    fn clear_node_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.node_filter.is_empty() {
            return;
        }
        self.node_filter_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.node_filter.clear();
        cx.notify();
    }

    fn clear_log_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.log_filter.is_empty() {
            return;
        }
        self.log_filter_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.log_filter.clear();
        cx.notify();
    }

    // === Node Selection ===

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);
        self.schedule_persist(cx);
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

    fn cycle_proxy_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self.proxy_mode {
            ProxyMode::Global => ProxyMode::Rule,
            ProxyMode::Rule => ProxyMode::Direct,
            ProxyMode::Direct => ProxyMode::Global,
        };
        self.set_proxy_mode(next, cx);
    }

    fn activate_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);

        // Already connected to this node — nothing to do.
        if self.proxy_running && self.active_proxy_node == Some(index) {
            cx.notify();
            return;
        }

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
            subscriptions: self.subscriptions.clone(),
            rules: self.rules.clone(),
        };

        if let Err(err) = save_gui_state(&config) {
            tracing::error!("failed to persist GUI state: {}", err);
        }
    }

    /// Debounced persist: coalesces multiple rapid calls (e.g. batch latency tests)
    /// into a single disk write 500ms after the last call.
    fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        if self.pending_persist {
            return; // write already queued
        }
        self.pending_persist = true;
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                })
                .await
                .ok();
            weak.update(cx, |this, _cx| {
                this.pending_persist = false;
                this.persist_gui_state();
            })
            .ok();
        })
        .detach();
    }

    // === Subscription Auto-Refresh ===

    /// Refresh a single subscription: replaces its nodes (by sub_url tag) with fresh ones.
    fn refresh_subscription(&mut self, sub_index: usize, cx: &mut Context<Self>) {
        let Some(sub) = self.subscriptions.get(sub_index).cloned() else {
            return;
        };
        if self.refreshing_subscriptions.contains(&sub_index) {
            return; // already refreshing this one
        }
        self.refreshing_subscriptions.insert(sub_index);
        cx.notify();

        let handle = self.tokio_handle.clone();
        let url = sub.url.clone();

        tracing::info!("Auto-refreshing subscription: {}", sub.name);
        cx.spawn(async move |weak, cx| {
            // Retry up to 3 times with exponential backoff
            let mut last_err = String::new();
            for attempt in 0..3u32 {
                if attempt > 0 {
                    handle
                        .spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt)))
                                .await;
                        })
                        .await
                        .ok();
                }
                let sub_clone = sub.clone();
                match handle
                    .spawn(async move { fetch_subscription(&sub_clone).await })
                    .await
                {
                    Ok(Ok(mut new_nodes)) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            normalize_node_names(&mut new_nodes);
                            // Tag new nodes with subscription URL
                            for n in &mut new_nodes {
                                n.extra.insert("sub_url".to_string(), url.clone());
                            }
                            // Save selection/active names before removing old nodes
                            let selected_name = this
                                .selected_node
                                .and_then(|i| this.nodes.get(i))
                                .map(|n| n.name.clone());
                            let active_name = this
                                .active_proxy_node
                                .and_then(|i| this.nodes.get(i))
                                .map(|n| n.name.clone());

                            // Remove stale nodes from this subscription
                            this.nodes
                                .retain(|n| n.extra.get("sub_url").map_or(true, |u| u != &url));

                            let count = new_nodes.len();
                            this.nodes.extend(new_nodes);

                            // Restore selection/active by name
                            if let Some(name) = selected_name {
                                this.selected_node = this.nodes.iter().position(|n| n.name == name);
                            }
                            if let Some(name) = active_name {
                                this.active_proxy_node =
                                    this.nodes.iter().position(|n| n.name == name);
                            }

                            // Update last_updated timestamp
                            if let Some(s) = this.subscriptions.get_mut(sub_index) {
                                s.last_updated = Some(
                                    SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                );
                            }
                            this.refreshing_subscriptions.remove(&sub_index);
                            this.import_status = format!(
                                "✓ Refreshed '{}': {} nodes",
                                this.subscriptions
                                    .get(sub_index)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("subscription"),
                                count
                            );
                            this.persist_gui_state();
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Ok(Err(e)) => last_err = format!("{:#}", e),
                    Err(e) => last_err = format!("task: {:#}", e),
                }
            }
            tracing::warn!(
                "Auto-refresh of subscription {} failed after 3 attempts: {}",
                sub_index,
                last_err
            );
            // Show failure in the import status bar so the user sees it in the Config tab.
            weak.update(cx, |this: &mut AppState, cx| {
                let name = this
                    .subscriptions
                    .get(sub_index)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "subscription".to_string());
                this.refreshing_subscriptions.remove(&sub_index);
                this.import_status = format!("⚠ Refresh failed for '{}': {}", name, last_err);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Refresh all subscriptions that are past their refresh_interval_hours.
    fn check_and_refresh_stale(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stale: Vec<usize> = self
            .subscriptions
            .iter()
            .enumerate()
            .filter(|(_, sub)| {
                sub.refresh_interval_hours > 0 && {
                    let threshold = sub.refresh_interval_hours * 3600;
                    match sub.last_updated {
                        None => true,
                        Some(ts) => now.saturating_sub(ts) >= threshold,
                    }
                }
            })
            .map(|(i, _)| i)
            .collect();

        for i in stale {
            self.refresh_subscription(i, cx);
        }
    }

    /// Start the periodic auto-refresh background task.
    /// Called once at startup; checks for stale subscriptions every 30 minutes.
    fn start_auto_refresh(&mut self, cx: &mut Context<Self>) {
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Brief delay to let the UI initialize before the first refresh.
            // Must run inside the tokio runtime (via handle.spawn) because
            // gpui's own executor does not provide a tokio reactor.
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                })
                .await
                .ok();
            // Initial stale check on startup
            if let Ok(()) = weak.update(cx, |this, cx| {
                this.check_and_refresh_stale(cx);
            }) {}
            // Then check every 30 minutes
            loop {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_secs(1800)).await;
                    })
                    .await
                    .ok();
                match weak.update(cx, |this, cx| {
                    this.check_and_refresh_stale(cx);
                }) {
                    Ok(_) => {}
                    Err(_) => break, // entity dropped, stop the loop
                }
            }
        })
        .detach();
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
            .flex_col()
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
            .on_action(cx.listener(|this, _: &CycleProxyMode, _, cx| {
                this.cycle_proxy_mode(cx);
            }))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_content(cx)),
            )
            .child(self.render_statusbar(cx))
    }
}

// === Title Bar ===

impl AppState {
    pub fn titlebar_options() -> TitlebarOptions {
        TitleBar::title_bar_options()
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_node_info = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| {
                let latency = n
                    .latency_ms
                    .map(|ms| format!("{}ms", ms))
                    .unwrap_or_default();
                (n.name.clone(), latency)
            });

        let proxy_running = self.proxy_running;

        let has_custom_rules = !self.rules.is_empty();
        let mode_label = match self.proxy_mode {
            ProxyMode::Global => "Global",
            ProxyMode::Rule => {
                if has_custom_rules {
                    "Rule"
                } else {
                    "Auto"
                }
            }
            ProxyMode::Direct => "Direct",
        };

        let (mode_bg, mode_fg) = match self.proxy_mode {
            ProxyMode::Global => (rgb(ACCENT), rgb(0xffffffu32)),
            ProxyMode::Rule if has_custom_rules => (rgb(0x2d4a22u32), rgb(SUCCESS_COLOR)),
            ProxyMode::Rule => (rgb(0x2a3a3au32), rgb(0x7ecee8u32)), // teal = built-in rules
            ProxyMode::Direct => (rgb(0x3a2a1au32), rgb(WARNING_COLOR)),
        };

        TitleBar::new()
            .h(px(26.0))
            // ── Left: app name ──────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h_full()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Vex"),
                    ),
            )
            // ── Right: mode pill + node indicator ───────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .h_full()
                    .pr_2()
                    // Clickable mode pill (cycles Global → Rule → Direct)
                    .child(
                        div()
                            .id("mode-pill")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .h(px(20.0))
                            .rounded(px(4.0))
                            .bg(mode_bg)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cycle_proxy_mode(cx);
                            }))
                            .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(
                                if proxy_running {
                                    rgb(SUCCESS_COLOR)
                                } else {
                                    rgb(TEXT_MUTED)
                                },
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(mode_fg)
                                    .child(mode_label),
                            ),
                    )
                    // Active node label
                    .children(active_node_info.map(|(name, latency)| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(div().w(px(1.0)).h(px(12.0)).bg(rgb(BORDER_COLOR)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .font(localized_font())
                                    .child(name),
                            )
                            .when(!latency.is_empty(), |this| {
                                this.child(
                                    div().text_xs().text_color(rgb(TEXT_MUTED)).child(latency),
                                )
                            })
                    })),
            )
    }

    fn render_statusbar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let (dot_color, status_text) = if self.proxy_running {
            (rgb(SUCCESS_COLOR), self.proxy_status.clone())
        } else {
            (rgb(TEXT_MUTED), self.proxy_status.clone())
        };

        let mode_label = match self.proxy_mode {
            ProxyMode::Global => "Global",
            ProxyMode::Rule => "Rule",
            ProxyMode::Direct => "Direct",
        };

        div()
            .w_full()
            .h(px(24.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .bg(rgb(BG_STATUSBAR))
            .border_t_1()
            .border_color(rgb(BORDER_COLOR))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(dot_color))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(status_text),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("Mode: {}", mode_label)),
                    )
                    .child(div().w(px(1.0)).h(px(10.0)).bg(rgb(BORDER_COLOR)))
                    .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("v0.1.0")),
            )
    }
}

// === Sidebar ===

impl AppState {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        div()
            .w(px(200.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER_COLOR))
            // Nav section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .pt_2()
                    .pb_1()
                    .child(self.nav_item(
                        "Dashboard",
                        "Ctrl+1",
                        IconName::LayoutDashboard,
                        ActiveView::Home,
                        &active,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Nodes",
                        "Ctrl+2",
                        IconName::Globe,
                        ActiveView::Nodes,
                        &active,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Config",
                        "Ctrl+3",
                        IconName::Settings2,
                        ActiveView::Config,
                        &active,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Rules",
                        "Ctrl+4",
                        IconName::Map,
                        ActiveView::Rules,
                        &active,
                        cx,
                    )),
            )
            // Separator + secondary section
            .child(
                div()
                    .px_3()
                    .py_1()
                    .child(div().border_b_1().border_color(rgb(BORDER_COLOR))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .py_1()
                    .child(self.nav_item(
                        "Settings",
                        "Ctrl+6",
                        IconName::Settings,
                        ActiveView::Settings,
                        &active,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Logs",
                        "Ctrl+5",
                        IconName::SquareTerminal,
                        ActiveView::Logs,
                        &active,
                        cx,
                    )),
            )
    }

    fn nav_item(
        &mut self,
        label: &str,
        shortcut: &str,
        icon: IconName,
        view: ActiveView,
        active: &ActiveView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = *active == view;

        let (text_color, icon_color, bg) = if is_active {
            (rgb(TEXT_PRIMARY), rgb(ACCENT), rgb(BG_ACCENT_SUBTLE))
        } else {
            (rgb(TEXT_SECONDARY), rgb(TEXT_MUTED), rgba(0x00000000u32))
        };

        let tooltip_text = SharedString::from(format!("{} ({})", label, shortcut));

        div()
            .id(SharedString::from(format!("nav-{}", label)))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .mx_2()
            .w(px(184.0))
            .px_2()
            .py_1p5()
            .rounded(px(5.0))
            .bg(bg)
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip_text.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(view.clone(), cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.0))
                    .h(px(16.0))
                    .text_color(icon_color)
                    .child(Icon::new(icon).xsmall()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(text_color)
                    .child(label.to_string()),
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
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_3()
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
            "Disconnect"
        } else {
            "Connect"
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

        // Compute bandwidth speed from delta since last render
        const HISTORY_LEN: usize = 60;
        let now = std::time::Instant::now();
        if self.proxy_running {
            if let Some((prev_up, prev_down, prev_time)) = self.prev_bandwidth_snapshot {
                let dt = now.duration_since(prev_time).as_secs_f64();
                if dt >= 0.5 {
                    self.upload_speed_bps = (bytes_up.saturating_sub(prev_up)) as f64 / dt;
                    self.download_speed_bps = (bytes_down.saturating_sub(prev_down)) as f64 / dt;
                    self.prev_bandwidth_snapshot = Some((bytes_up, bytes_down, now));
                    // Push to history ring buffers
                    self.upload_history.push_back(self.upload_speed_bps);
                    self.download_history.push_back(self.download_speed_bps);
                    if self.upload_history.len() > HISTORY_LEN {
                        self.upload_history.pop_front();
                    }
                    if self.download_history.len() > HISTORY_LEN {
                        self.download_history.pop_front();
                    }
                }
            } else {
                self.prev_bandwidth_snapshot = Some((bytes_up, bytes_down, now));
            }
        } else {
            self.prev_bandwidth_snapshot = None;
            self.upload_speed_bps = 0.0;
            self.download_speed_bps = 0.0;
            // Drain history when proxy stops
            if !self.upload_history.is_empty() {
                self.upload_history.clear();
                self.download_history.clear();
            }
        }
        let upload_speed = self.upload_speed_bps;
        let download_speed = self.download_speed_bps;
        let upload_hist: Vec<f64> = self.upload_history.iter().cloned().collect();
        let download_hist: Vec<f64> = self.download_history.iter().cloned().collect();

        let cur_mode = self.proxy_mode.clone();

        // Derive startup phase from existing status strings (no extra field needed):
        // 0 = idle, 1 = service starting, 2 = connected+verifying, 3 = verified
        let startup_phase: u8 = if !self.proxy_running {
            if self.proxy_status.starts_with("Connecting")
                || self.proxy_status == "Disconnecting..."
            {
                1
            } else {
                0
            }
        } else if self.proxy_validation_status.contains("Verifying")
            || self.proxy_validation_status.contains("Waiting")
        {
            2
        } else {
            3
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            // Page header
            .child(self.page_header("Dashboard"))
            // Two-column grid
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .items_start()
                    // Left column: Connection status + Statistics
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_3()
                            // Status card
                            .child({
                                let card = if self.proxy_running {
                                    self.card().border_l_2().border_color(rgb(ACCENT))
                                } else {
                                    self.card()
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
                                                        .w(px(8.0))
                                                        .h(px(8.0))
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
                                                        .text_xs()
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
                                                })
                                                .child(
                                                    div().flex_1()
                                                )
                                                .child({
                                                    let is_connecting = self.proxy_status.contains("...");
                                                    let connect_btn = Button::new("connect-btn").xsmall()
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
                                        .when(startup_phase >= 1 && startup_phase <= 2, |d| {
                                            d.child(div().child(render_proxy_steps(startup_phase)))
                                        })
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_0()
                                                .pt_2()
                                                .child(self.stat_item("Active node", &active_name))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .justify_between()
                                                        .py_1p5()
                                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Validation"))
                                                        .child(div().text_xs().text_color(rgb(status_text_color(&self.proxy_validation_status))).child(self.proxy_validation_status.clone())),
                                                ),
                                        ),
                                )
                            })
                            // Statistics card
                            .child(
                                self.card()
                                    .child(self.card_title("Statistics"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(self.stat_item("Active Connections", &active_conn.to_string()))
                                            .child(self.stat_item("Total Connections", &total_conn.to_string()))
                                            .child(self.stat_item("Nodes", &self.nodes.len().to_string()))
                                            .child(self.stat_item("Uploaded", &format_bytes(bytes_up)))
                                            .child(self.stat_item("Downloaded", &format_bytes(bytes_down)))
                                            .when(self.proxy_running, |d| {
                                                d.child(self.stat_item("Upload Speed", &format_speed(upload_speed)))
                                                 .child(self.stat_item("Download Speed", &format_speed(download_speed)))
                                            }),
                                    )
                                    .when(!upload_hist.is_empty() || !download_hist.is_empty(), |card| {
                                        let uh = upload_hist.clone();
                                        let dh = download_hist.clone();
                                        card.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .pt_2()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Traffic History (60s)"))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_row()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .flex_row()
                                                                        .items_center()
                                                                        .gap_1()
                                                                        .child(div().w(px(8.0)).h(px(3.0)).rounded(px(1.5)).bg(rgba(0x3b82f6ccu32)))
                                                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("↑")),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .flex_row()
                                                                        .items_center()
                                                                        .gap_1()
                                                                        .child(div().w(px(8.0)).h(px(3.0)).rounded(px(1.5)).bg(rgba(0x22c55eccu32)))
                                                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("↓")),
                                                                ),
                                                        ),
                                                )
                                                .child(
                                                    canvas(
                                                        |_, _, _| {},
                                                        move |bounds, _, window, _| {
                                                            let w = f32::from(bounds.size.width);
                                                            let h = f32::from(bounds.size.height);
                                                            let ox = f32::from(bounds.origin.x);
                                                            let oy = f32::from(bounds.origin.y);
                                                            const N: usize = 60;
                                                            let max_val = uh.iter().chain(dh.iter())
                                                                .cloned()
                                                                .fold(1.0_f64, f64::max);
                                                            let bar_w = w / N as f32;
                                                            let gap = (bar_w * 0.1).max(0.5);
                                                            let half = bar_w * 0.5;
                                                            for (i, &val) in uh.iter().enumerate() {
                                                                let frac = (val / max_val).min(1.0) as f32;
                                                                let bh = (frac * h).max(1.0);
                                                                window.paint_quad(gpui::fill(
                                                                    gpui::Bounds {
                                                                        origin: gpui::point(gpui::px(ox + i as f32 * bar_w + gap), gpui::px(oy + h - bh)),
                                                                        size: gpui::size(gpui::px((half - gap * 2.0).max(1.0)), gpui::px(bh)),
                                                                    },
                                                                    rgba(0x3b82f6aau32),
                                                                ));
                                                            }
                                                            for (i, &val) in dh.iter().enumerate() {
                                                                let frac = (val / max_val).min(1.0) as f32;
                                                                let bh = (frac * h).max(1.0);
                                                                window.paint_quad(gpui::fill(
                                                                    gpui::Bounds {
                                                                        origin: gpui::point(gpui::px(ox + i as f32 * bar_w + half + gap), gpui::px(oy + h - bh)),
                                                                        size: gpui::size(gpui::px((half - gap * 2.0).max(1.0)), gpui::px(bh)),
                                                                    },
                                                                    rgba(0x22c55eaau32),
                                                                ));
                                                            }
                                                        },
                                                    )
                                                    .w_full()
                                                    .h(px(64.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(BG_ACCENT_SUBTLE)),
                                                ),
                                        )
                                    }),
                            )
                    )
                    // Right column: Mode + Endpoints + System Proxy + TUN
                    .child(
                        div()
                            .w(px(240.0))
                            .flex()
                            .flex_col()
                            .gap_3()
                            // Proxy Mode card
                            .child(
                                self.card()
                                    .child(self.card_title("Proxy Mode"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .gap_1()
                                            .child({
                                                let btn = Button::new("mode-global").xsmall()
                                                    .label("Global".to_string())
                                                    .tooltip("Route all traffic through proxy")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.set_proxy_mode(ProxyMode::Global, cx);
                                                    }));
                                                if cur_mode == ProxyMode::Global { btn.primary() } else { btn.ghost() }
                                            })
                                            .child({
                                                let btn = Button::new("mode-rule").xsmall()
                                                    .label("Rule".to_string())
                                                    .tooltip("Route traffic based on rules (e.g. China direct)")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.set_proxy_mode(ProxyMode::Rule, cx);
                                                    }));
                                                if cur_mode == ProxyMode::Rule { btn.primary() } else { btn.ghost() }
                                            })
                                            .child({
                                                let btn = Button::new("mode-direct").xsmall()
                                                    .label("Direct".to_string())
                                                    .tooltip("Bypass proxy, connect directly")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.set_proxy_mode(ProxyMode::Direct, cx);
                                                    }));
                                                if cur_mode == ProxyMode::Direct { btn.primary() } else { btn.ghost() }
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(TEXT_MUTED))
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
                            // Endpoints card
                            .child(
                                self.card()
                                    .child(self.card_title("Endpoints"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(self.stat_item(
                                                "SOCKS5",
                                                &format!("{}:{}", self.listen_addr, self.socks_port),
                                            ))
                                            .child(self.stat_item(
                                                "HTTP",
                                                &format!("{}:{}", self.listen_addr, self.http_port),
                                            )),
                                    ),
                            )
                            // Network Control card (System Proxy + TUN)
                            .child(
                                self.card()
                                    .child(self.card_title("Network Control"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_0()
                                            // System Proxy row
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .py_2()
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap_0p5()
                                                            .child(div().text_xs().text_color(rgb(TEXT_PRIMARY)).child("System Proxy"))
                                                            .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(self.system_proxy_status.clone())),
                                                    )
                                                    .child(if self.system_proxy_enabled {
                                                        Button::new("sys-proxy-toggle").xsmall()
                                                            .label("Disable".to_string())
                                                            .tooltip("Remove vex from OS proxy settings")
                                                            .ghost()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.disable_system_proxy(cx);
                                                            }))
                                                    } else {
                                                        Button::new("sys-proxy-toggle").xsmall()
                                                            .label("Enable".to_string())
                                                            .tooltip("Set vex as system proxy")
                                                            .primary()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.enable_system_proxy(cx);
                                                            }))
                                                    }),
                                            )
                                            // TUN row
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_between()
                                                    .py_2()
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap_0p5()
                                                            .child(div().text_xs().text_color(rgb(TEXT_PRIMARY)).child("TUN Mode"))
                                                            .child(div().text_xs().text_color(rgb(status_text_color(&self.tun_status))).child(self.tun_status.clone())),
                                                    )
                                                    .child(if self.tun_enabled {
                                                        Button::new("tun-toggle").xsmall()
                                                            .label("Disable".to_string())
                                                            .tooltip("Stop TUN virtual network interface")
                                                            .ghost()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.stop_tun(cx);
                                                            }))
                                                    } else if self.tun_starting {
                                                        Button::new("tun-toggle").xsmall()
                                                            .label("Starting...".to_string())
                                                            .tooltip("TUN is initializing, please wait")
                                                            .loading(true)
                                                            .ghost()
                                                    } else if tun_supported() {
                                                        Button::new("tun-toggle").xsmall()
                                                            .label("Enable".to_string())
                                                            .tooltip("Start TUN virtual network interface for system-wide proxying")
                                                            .primary()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.start_tun(cx);
                                                            }))
                                                    } else {
                                                        Button::new("tun-toggle").xsmall()
                                                            .label("N/A".to_string())
                                                            .tooltip(tun_requirements())
                                                            .disabled(true)
                                                            .ghost()
                                                    }),
                                            ),
                                    ),
                            )
                    )
            )
    }

    fn card(&self) -> Div {
        div().p_3().rounded(px(8.0)).bg(rgb(BG_CARD))
    }

    /// Section title inside a card — semibold, muted, no border (Zed panel label style).
    fn card_title(&self, text: &str) -> Div {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT_MUTED))
            .pb_1()
            .mb_1()
            .child(text.to_string())
    }

    fn stat_item(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .py_0p5()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT_PRIMARY))
                    .child(value.to_string()),
            )
    }

    fn page_header(&self, title: &str) -> Div {
        div().pb_1().mb_1().child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT_PRIMARY))
                .child(title.to_string()),
        )
    }
}

// === Nodes View ===

impl AppState {
    fn render_nodes(&mut self, cx: &mut Context<Self>) -> Div {
        let node_count = self.nodes.len();

        // Read and apply text filter
        let filter_text = self
            .node_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        if filter_text != self.node_filter {
            self.node_filter = filter_text.clone();
        }

        // Collect distinct protocols present in the node list for chips
        let mut present_protocols: Vec<&'static str> = {
            let mut seen = std::collections::HashSet::new();
            self.nodes
                .iter()
                .map(|n| protocol_short_name(&n.protocol))
                .filter(|s| seen.insert(*s))
                .collect()
        };
        // Stable order: SS, VMess, VLESS, Trojan, TUIC, Hy2
        let protocol_order = ["ss", "vmess", "vless", "trojan", "tuic", "hy2"];
        present_protocols.sort_by_key(|p| protocol_order.iter().position(|o| o == p).unwrap_or(99));

        let proto_filter = self.protocol_filter;
        let tag_filter = self.tag_filter.clone();
        let filtered_indices: Vec<usize> = (0..node_count)
            .filter(|&i| {
                let n = &self.nodes[i];
                // Protocol chip filter
                let proto_match = match proto_filter {
                    None => true,
                    Some(pf) => protocol_short_name(&n.protocol) == pf,
                };
                // Tag filter
                let tag_match = match &tag_filter {
                    None => true,
                    Some(tf) => n.tags.iter().any(|t| t == tf),
                };
                // Text filter
                let text_match = filter_text.is_empty()
                    || n.name.to_lowercase().contains(&filter_text)
                    || n.server.to_lowercase().contains(&filter_text)
                    || protocol_short_name(&n.protocol)
                        .to_lowercase()
                        .contains(&filter_text)
                    || n.tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&filter_text));
                proto_match && tag_match && text_match
            })
            .collect();
        // Sort: tested nodes by latency ascending, untested last
        let mut filtered_indices = filtered_indices;
        filtered_indices.sort_by(|&a, &b| {
            match (self.nodes[a].latency_ms, self.nodes[b].latency_ms) {
                (Some(a_ms), Some(b_ms)) => a_ms.cmp(&b_ms),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        let shown_count = filtered_indices.len();
        let node_filter_input = self.node_filter_input.clone();

        // Collect all unique tags across all nodes for filter chips
        let all_tags: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            let mut tags: Vec<String> = self
                .nodes
                .iter()
                .flat_map(|n| n.tags.iter().cloned())
                .filter(|t| seen.insert(t.clone()))
                .collect();
            tags.sort();
            tags
        };
        let cur_tag_filter = self.tag_filter.clone();

        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb_3()
                    .mb_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
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
                                Button::new("sort-alpha-btn").xsmall()
                                    .label("A→Z".to_string())
                                    .tooltip("Sort nodes alphabetically by name")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sort_nodes_alphabetically(cx);
                                    })),
                            )
                            .child(
                                Button::new("sort-latency-btn").xsmall()
                                    .label("Sort by Latency".to_string())
                                    .tooltip("Sort nodes by latency (fastest first)")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sort_nodes_by_latency(cx);
                                    })),
                            )
                            .child(
                                Button::new("test-all-btn").xsmall()
                                    .label("Test All".to_string())
                                    .tooltip("Test latency for all nodes (Ctrl+T)")
                                    .ghost()
                                    .loading(!self.latency_testing.is_empty())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.auto_select_best = false;
                                        this.test_all_latency(cx);
                                    })),
                            )
                            .child(
                                Button::new("test-select-best-btn").xsmall()
                                    .label("Best Node".to_string())
                                    .tooltip("Test all nodes and automatically connect to the fastest one")
                                    .primary()
                                    .loading(self.pending_latency_batch > 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.test_all_and_select_best(cx);
                                    })),
                            )
                            .child(
                                Button::new("delete-all-btn").xsmall()
                                    .label("Delete All".to_string())
                                    .tooltip("Delete all nodes")
                                    .danger()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_all_nodes(cx);
                                    })),
                            )
                            .child(
                                Button::new("select-all-btn").xsmall()
                                    .label("Select All".to_string())
                                    .tooltip("Select all visible nodes")
                                    .ghost()
                                    .on_click({
                                        let fi = filtered_indices.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            for &i in &fi {
                                                this.selected_node_indices.insert(i);
                                            }
                                            cx.notify();
                                        })
                                    }),
                            )
                            .when(!self.selected_node_indices.is_empty(), |row| {
                                let sel_count = self.selected_node_indices.len();
                                row
                                    .child(
                                        Button::new("deselect-all-btn").xsmall()
                                            .label("Deselect All".to_string())
                                            .tooltip("Clear selection")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.selected_node_indices.clear();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("delete-selected-btn").xsmall()
                                            .label(format!("Delete ({sel_count})"))
                                            .tooltip("Delete selected nodes")
                                            .danger()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let mut indices: Vec<usize> = this.selected_node_indices.drain().collect();
                                                indices.sort_unstable_by(|a, b| b.cmp(a));
                                                for idx in indices {
                                                    if idx < this.nodes.len() {
                                                        this.nodes.remove(idx);
                                                    }
                                                }
                                                this.selected_node = None;
                                                this.active_proxy_node = None;
                                                this.persist_gui_state();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("copy-share-btn").xsmall()
                                            .label(format!("Copy Links ({sel_count})"))
                                            .tooltip("Copy share URIs for selected nodes to clipboard")
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let links: Vec<String> = this.selected_node_indices
                                                    .iter()
                                                    .filter(|&&i| i < this.nodes.len())
                                                    .map(|&i| vex_core::config::v2ray::node_to_share_uri(&this.nodes[i]))
                                                    .collect();
                                                let text = links.join("\n");
                                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                            })),
                                    )
                            }),
                    ),
            )
            .child({
                // Filter toolbar: filter input + protocol chips
                let filter_toolbar = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(gpui_component::input::Input::new(&node_filter_input).w_full()),
                            )
                            .child({
                                let has_filter = !self.node_filter.is_empty();
                                Button::new("clear-filter-btn").xsmall()
                                    .label("Clear".to_string())
                                    .tooltip("Clear filter")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_node_filter(window, cx);
                                    }))
                                    .when(!has_filter, |b| b.disabled(true))
                            }),
                    )
                    .child({
                        let mut row = div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .flex_wrap()
                            .items_center();
                        if !present_protocols.is_empty() {
                            let all_active = proto_filter.is_none();
                            row = row.child(
                                Button::new("proto-all").xsmall()
                                    .label("All".to_string())
                                    .tooltip("Show all protocols")
                                    .when(all_active, |b| b.primary())
                                    .when(!all_active, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.protocol_filter = None;
                                        cx.notify();
                                    })),
                            );
                            for proto_key in &present_protocols {
                                let key: &'static str = proto_key;
                                let active = proto_filter == Some(key);
                                let display = match key {
                                    "ss" => "SS",
                                    "vmess" => "VMess",
                                    "vless" => "VLESS",
                                    "trojan" => "Trojan",
                                    "tuic" => "TUIC",
                                    "hy2" => "Hy2",
                                    _ => key,
                                };
                                let btn_id = format!("proto-{}", key);
                                row = row.child(
                                    Button::new(SharedString::from(btn_id)).xsmall()
                                        .label(display.to_string())
                                        .tooltip(format!("Show only {} nodes", display))
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.protocol_filter = Some(key);
                                            cx.notify();
                                        })),
                                );
                            }
                        }
                        // Tag filter chips (only if any tags exist)
                        if !all_tags.is_empty() {
                            row = row.child(
                                div()
                                    .w(px(1.0))
                                    .h(px(16.0))
                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                    .mx_1(),
                            );
                            let no_tag = cur_tag_filter.is_none();
                            row = row.child(
                                Button::new("tag-all").xsmall()
                                    .label("All Tags".to_string())
                                    .tooltip("Show nodes from all tags")
                                    .when(no_tag, |b| b.primary())
                                    .when(!no_tag, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tag_filter = None;
                                        cx.notify();
                                    })),
                            );
                            for tag in &all_tags {
                                let tag_clone = tag.clone();
                                let active = cur_tag_filter.as_deref() == Some(tag.as_str());
                                let btn_id = format!("tag-{}", tag);
                                row = row.child(
                                    Button::new(SharedString::from(btn_id)).xsmall()
                                        .label(format!("#{}", tag))
                                        .tooltip(format!("Show only nodes tagged #{}", tag))
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.tag_filter = Some(tag_clone.clone());
                                            cx.notify();
                                        })),
                                );
                            }
                        }
                        row
                    });

                // Two-column body: left = filters+list, right = detail panel
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .items_start()
                    // Left column: filter toolbar + node list
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.0))
                            .max_w(px(520.0))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(filter_toolbar)
                            .child({
                                if node_count == 0 {
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .py_8()
                                        .gap_2()
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("No nodes configured"))
                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Go to Config tab to import a subscription"))
                                        .into_any_element()
                                } else if filtered_indices.is_empty() {
                                    div()
                                        .py_4()
                                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("No nodes match the current filter"))
                                        .into_any_element()
                                } else {
                                    let mut list = div().flex().flex_col().gap_0p5();
                                    for &i in &filtered_indices {
                                        list = list.child(self.render_node_item(i, cx));
                                    }
                                    list.into_any_element()
                                }
                            }),
                    )
                    // Right column: detail panel (always visible, 280px fixed)
                    .child(
                        div()
                            .w(px(240.0))
                            .flex()
                            .flex_col()
                            .child(
                                if let Some(index) = self.selected_node {
                                    if let Some(node) = self.nodes.get(index).cloned() {
                                        self.render_selected_node_details(&node, index, cx)
                                            .into_any_element()
                                    } else {
                                        div().into_any_element()
                                    }
                                } else {
                                    self.card()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .py_8()
                                                .gap_2()
                                                .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("No node selected"))
                                                .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Click a node to view details")),
                                        )
                                        .into_any_element()
                                },
                            ),
                    )
            });

        content
    }

    fn render_node_item(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let node = &self.nodes[index];
        let is_selected = self.selected_node == Some(index);
        let is_active = self.proxy_running && self.active_proxy_node == Some(index);
        let name = node.name.clone();
        let server = format!("{}:{}", node.server, node.port);
        let protocol_tag = protocol_tag_label(&node.protocol);
        let is_testing = self.latency_testing.contains(&index);
        let is_failed = self.latency_failed.contains(&index);
        let node_tags = node.tags.clone();
        let latency = node
            .latency_ms
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| {
                if is_failed {
                    "err".to_string()
                } else {
                    "---".to_string()
                }
            });
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
            .unwrap_or_else(|| {
                if is_failed {
                    rgb(DANGER_COLOR)
                } else {
                    rgb(TEXT_MUTED)
                }
            });

        // Subscription source badge
        let sub_badge: Option<String> = node.extra.get("sub_url").and_then(|url| {
            self.subscriptions
                .iter()
                .find(|s| &s.url == url)
                .map(|s| s.name.clone())
        });

        let bg = if is_active {
            rgb(BG_ACCENT_SUBTLE)
        } else if is_selected {
            rgb(BG_CARD_HOVER)
        } else {
            rgba(0x00000000u32)
        };

        let dot_color = if is_active {
            rgb(ACCENT)
        } else if is_selected {
            rgb(ACCENT_DIM)
        } else {
            rgba(0x00000000u32)
        };

        div()
            .id(SharedString::from(format!("node-{}", index)))
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .py_1()
            .rounded(px(5.0))
            .bg(bg)
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_node(index, cx);
            }))
            .child({
                // Selection checkbox
                let is_checked = self.selected_node_indices.contains(&index);
                let checkbox_bg = if is_checked {
                    rgb(ACCENT)
                } else {
                    rgba(0x00000000u32)
                };
                let checkbox_border = if is_checked {
                    rgb(ACCENT)
                } else {
                    rgb(BORDER_COLOR)
                };
                div()
                    .id(SharedString::from(format!("node-check-{}", index)))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(px(3.0))
                    .flex_shrink_0()
                    .mr_2()
                    .cursor_pointer()
                    .border_1()
                    .border_color(checkbox_border)
                    .bg(checkbox_bg)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.selected_node_indices.contains(&index) {
                            this.selected_node_indices.remove(&index);
                        } else {
                            this.selected_node_indices.insert(index);
                        }
                        cx.notify();
                    }))
            })
            .child(
                // Active/selected dot indicator
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .mr_3()
                    .flex_shrink_0()
                    .bg(dot_color),
            )
            .child(
                // Node info
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font(localized_font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(server),
                            )
                            .when_some(sub_badge, |d, badge| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(ACCENT_DIM))
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(3.0))
                                        .bg(rgb(BG_ACCENT_SUBTLE))
                                        .child(badge),
                                )
                            }),
                    )
                    .when(!node_tags.is_empty(), |d| {
                        let mut tag_row = div().flex().flex_row().gap_1().flex_wrap();
                        for tag in &node_tags {
                            tag_row = tag_row.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(3.0))
                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                    .child(format!("#{}", tag)),
                            );
                        }
                        d.child(tag_row)
                    }),
            )
            .child(
                // Protocol tag
                div().mx_2().child(protocol_tag),
            )
            .child(
                // Latency or testing indicator
                if is_testing {
                    div()
                        .w(px(54.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .w(px(5.0))
                                .h(px(5.0))
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
                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("..."))
                        .into_any_element()
                } else {
                    div()
                        .w(px(54.0))
                        .text_xs()
                        .text_color(latency_color)
                        .child(latency)
                        .into_any_element()
                },
            )
            .child(
                Button::new(("delete-node", index))
                    .xsmall()
                    .label("×".to_string())
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
    fn delete_subscription(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.subscriptions.len() {
            return;
        }
        let url = self.subscriptions[index].url.clone();
        self.subscriptions.remove(index);
        // Remove nodes tagged with this subscription URL
        self.nodes
            .retain(|n| n.extra.get("sub_url").map_or(true, |u| u != &url));
        // Restore indices after removal
        let len = self.nodes.len();
        if let Some(i) = self.selected_node {
            if i >= len {
                self.selected_node = if len > 0 { Some(len - 1) } else { None };
            }
        }
        if let Some(i) = self.active_proxy_node {
            if i >= len {
                self.active_proxy_node = None;
            }
        }
        self.persist_gui_state();
        cx.notify();
    }

    /// Cycle through common refresh intervals: 24h → 12h → 6h → off (0) → 24h
    fn cycle_refresh_interval(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(sub) = self.subscriptions.get_mut(index) {
            sub.refresh_interval_hours = match sub.refresh_interval_hours {
                0 => 1,
                1 => 2,
                2 => 6,
                6 => 12,
                12 => 24,
                _ => 0, // 24 or any custom → Off
            };
        }
        self.persist_gui_state();
        cx.notify();
    }

    fn format_last_updated(ts: Option<u64>) -> String {
        let Some(ts) = ts else {
            return "Never".to_string();
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let age = now.saturating_sub(ts);
        if age < 60 {
            "Just now".to_string()
        } else if age < 3600 {
            format!("{}m ago", age / 60)
        } else if age < 86400 {
            format!("{}h ago", age / 3600)
        } else {
            format!("{}d ago", age / 86400)
        }
    }

    fn render_subscription_list(&mut self, cx: &mut Context<Self>) -> Div {
        let sub_count = self.subscriptions.len();
        let card = self.card().child(self.card_title(&format!(
            "Saved Subscriptions{}",
            if sub_count > 0 {
                format!(" — {}", sub_count)
            } else {
                String::new()
            }
        )));

        if sub_count == 0 {
            return card.child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("No subscriptions saved. Import one above."),
            );
        }

        let mut rows = div().flex().flex_col().gap_2();
        for (i, sub) in self.subscriptions.iter().enumerate() {
            let last_updated = Self::format_last_updated(sub.last_updated);
            let interval_label = match sub.refresh_interval_hours {
                0 => "Off".to_string(),
                1 => "1h".to_string(),
                h => format!("{}h", h),
            };
            let url_display = if sub.url.len() > 48 {
                format!("{}…", &sub.url[..48])
            } else {
                sub.url.clone()
            };
            let name = sub.name.clone();
            let node_count = self
                .nodes
                .iter()
                .filter(|n| n.extra.get("sub_url").map_or(false, |u| u == &sub.url))
                .count();
            let is_refreshing = self.refreshing_subscriptions.contains(&i);

            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(px(4.0))
                .bg(rgb(BG_CARD))
                .border_1()
                .border_color(rgb(BORDER_COLOR))
                .hover(|s| s.bg(rgb(BG_CARD_HOVER)))
                // Left: name + meta info
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(3.0))
                                        .bg(rgb(BG_ACCENT_SUBTLE))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{} nodes", node_count)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .overflow_x_hidden()
                                        .child(url_display),
                                )
                                .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(
                                    if is_refreshing {
                                        format!("· {}", "refreshing…")
                                    } else {
                                        format!("· {}", last_updated)
                                    },
                                )),
                        ),
                )
                // Right: action buttons
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new(("interval-btn", i))
                                .xsmall()
                                .label(format!("Auto: {}", interval_label))
                                .tooltip("Click to cycle: Off → 1h → 2h → 6h → 12h → 24h → Off")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cycle_refresh_interval(i, cx);
                                })),
                        )
                        .child(
                            Button::new(("refresh-sub-btn", i))
                                .xsmall()
                                .label("Refresh".to_string())
                                .tooltip("Fetch new nodes from this subscription now")
                                .ghost()
                                .loading(is_refreshing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.refresh_subscription(i, cx);
                                })),
                        )
                        .child(
                            Button::new(("delete-sub-btn", i))
                                .xsmall()
                                .label("×".to_string())
                                .tooltip("Remove this subscription and its nodes")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_subscription(i, cx);
                                })),
                        ),
                );
            rows = rows.child(row);
        }
        card.child(rows)
    }

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
            .gap_3()
            // Title
            .child(self.page_header("Configuration"))
            // Tabbed import card: Import Subscription | Add by URI
            .child({
                let is_import_tab = self.config_tab == ConfigTab::ImportSubscription;
                self.card()
                    .child(
                        // Tab strip
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .pb_3()
                            .mb_2()
                            .child(
                                Button::new("config-tab-import").xsmall()
                                    .label("Import Subscription".to_string())
                                    .when(is_import_tab, |b| b.primary())
                                    .when(!is_import_tab, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.config_tab = ConfigTab::ImportSubscription;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("config-tab-uri").xsmall()
                                    .label("Add by Share Link".to_string())
                                    .when(!is_import_tab, |b| b.primary())
                                    .when(is_import_tab, |b| b.ghost())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.config_tab = ConfigTab::AddByUri;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(if is_import_tab {
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
                                        Button::new("import-btn").xsmall()
                                            .label("Import".to_string())
                                            .tooltip("Fetch nodes from subscription URL")
                                            .primary()
                                            .loading(self.import_status.contains("Importing"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_subscription(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("clear-btn").xsmall()
                                            .label("Clear Nodes".to_string())
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
                                    .text_xs()
                                    .text_color(if import_status.starts_with('\u{2713}') {
                                        rgb(SUCCESS_COLOR)
                                    } else if import_status.starts_with('\u{2717}') {
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
                            })
                            .into_any_element()
                    } else {
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
                                    .items_center()
                                    .child(
                                        Button::new("add-uri-btn").xsmall()
                                            .label("Add Node".to_string())
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
                                            .child("vless:// · vmess:// · ss:// · trojan:// · tuic:// · hy2://"),
                                    ),
                            )
                            .into_any_element()
                    })
            })
            // Saved subscriptions card
            .child(self.render_subscription_list(cx))
            // Node summary card
            .child(
                self.card()
                    .child(self.card_title("Node Summary"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0()
                            .child(self.stat_item("Shadowsocks", &ss_count.to_string()))
                            .child(self.stat_item("VMess", &vmess_count.to_string()))
                            .child(self.stat_item("VLESS", &vless_count.to_string()))
                            .child(self.stat_item("TUIC", &tuic_count.to_string()))
                            .child(self.stat_item("Trojan", &trojan_count.to_string()))
                            .child(self.stat_item("Hysteria2", &hy2_count.to_string()))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .pt_2()
                                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT_SECONDARY)).child("Total"))
                                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT_PRIMARY)).child(node_count.to_string()))
                            ),
                    ),
            )
    }
}

// === Rules View ===

impl AppState {
    fn add_rule(&mut self, cx: &mut Context<Self>) {
        let rule_type = self.rule_type_sel.clone();
        let pattern = self.rule_pattern_input.read(cx).value().to_string();
        let target = self.rule_target_sel.clone();

        if pattern.trim().is_empty() {
            self.rules_status = "⚠ Pattern is required".to_string();
            cx.notify();
            return;
        }

        let new_rule = RoutingRule {
            rule_type: rule_type.trim().to_string(),
            pattern: pattern.trim().to_string(),
            target: target.trim().to_string(),
            enabled: true,
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
        // Set button-group selectors
        self.rule_type_sel = rule.rule_type.clone();
        self.rule_target_sel = rule.target.clone();
        self.rule_pattern_input.update(cx, |state, cx| {
            state.set_value(rule.pattern, window, cx);
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
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "baidu.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "qq.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "taobao.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "aliyun.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "jd.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "163.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "bilibili.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "zhihu.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "10.0.0.0/8".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "172.16.0.0/12".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
                enabled: true,
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
        let rule_pattern_input = self.rule_pattern_input.clone();
        let cur_type = self.rule_type_sel.clone();
        let cur_target = self.rule_target_sel.clone();

        let pattern_hint: &'static str = match cur_type.as_str() {
            "domain" => "e.g. google.com",
            "domain-suffix" => "e.g. google.com (matches *.google.com too)",
            "domain-keyword" => "e.g. youtube",
            "ip-cidr" => "e.g. 192.168.0.0/24",
            "geoip" => "e.g. CN",
            "match" => "no pattern needed — matches all",
            _ => "",
        };

        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            // Title
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb_3()
                    .mb_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
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
                                    .xsmall()
                                    .label("Load China Direct".to_string())
                                    .tooltip(
                                        "Load built-in rules to bypass proxy for Chinese sites",
                                    )
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_china_rules(cx);
                                    })),
                            )
                            .child(
                                Button::new("clear-rules")
                                    .xsmall()
                                    .label("Clear All".to_string())
                                    .tooltip("Remove all routing rules")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_rules(cx);
                                    })),
                            ),
                    ),
            )
            // Inline add/edit rule form
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // Row 1: Type button chips
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("Type"),
                            )
                            .child({
                                let t = cur_type.clone();
                                let types: &[(&str, &str)] = &[
                                    ("domain", "Domain"),
                                    ("domain-suffix", "Suffix"),
                                    ("domain-keyword", "Keyword"),
                                    ("ip-cidr", "IP CIDR"),
                                    ("geoip", "GeoIP"),
                                    ("match", "Match All"),
                                ];
                                let mut row = div().flex().flex_row().gap_1().flex_wrap();
                                for &(val, label) in types {
                                    let active = t == val;
                                    row =
                                        row.child(
                                            Button::new(SharedString::from(format!(
                                                "rule-type-{}",
                                                val
                                            )))
                                            .xsmall()
                                            .label(label.to_string())
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.rule_type_sel = val.to_string();
                                                cx.notify();
                                            })),
                                        );
                                }
                                row
                            }),
                    )
                    // Row 2: Pattern input + Target buttons + Action
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_end()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child("Pattern"),
                                    )
                                    .child(
                                        gpui_component::input::Input::new(&rule_pattern_input)
                                            .w_full(),
                                    )
                                    .when(!pattern_hint.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(TEXT_MUTED))
                                                .child(pattern_hint),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child("Target"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .gap_1()
                                            .child({
                                                let active = cur_target == "direct";
                                                Button::new("target-direct")
                                                    .xsmall()
                                                    .label("Direct".to_string())
                                                    .tooltip("直连，不经过代理")
                                                    .when(active, |b| b.primary())
                                                    .when(!active, |b| b.ghost())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.rule_target_sel = "direct".to_string();
                                                        cx.notify();
                                                    }))
                                            })
                                            .child({
                                                let active = cur_target == "proxy";
                                                Button::new("target-proxy")
                                                    .xsmall()
                                                    .label("Proxy".to_string())
                                                    .tooltip("走代理节点")
                                                    .when(active, |b| b.primary())
                                                    .when(!active, |b| b.ghost())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.rule_target_sel = "proxy".to_string();
                                                        cx.notify();
                                                    }))
                                            })
                                            .child({
                                                let active = cur_target == "reject";
                                                Button::new("target-reject")
                                                    .xsmall()
                                                    .label("Block".to_string())
                                                    .tooltip("拦截，拒绝连接")
                                                    .when(active, |b| b.primary())
                                                    .when(!active, |b| b.ghost())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.rule_target_sel = "reject".to_string();
                                                        cx.notify();
                                                    }))
                                            }),
                                    ),
                            )
                            .child(
                                Button::new("add-rule-btn")
                                    .xsmall()
                                    .label(if self.editing_rule_index.is_some() {
                                        "Save".to_string()
                                    } else {
                                        "Add".to_string()
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
                                        .xsmall()
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
                    .children(if !rules_status.is_empty() {
                        Some(
                            div()
                                .text_xs()
                                .text_color(rgb(status_text_color(&rules_status)))
                                .child(rules_status),
                        )
                    } else {
                        None
                    }),
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
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child("No routing rules configured"),
                        )
                        .child(
                            div()
                                .text_xs()
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
                .child(self.card_title("How Rules Work"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("Rules are evaluated top-to-bottom; first match wins."))
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("direct = connect without proxy  |  proxy = use selected node  |  reject = block"))
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child("Rule mode uses your custom rules, falling back to built-in China Direct profile."))
                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Use the up/down buttons to reorder rules.")),
                ),
        );

        content
    }

    fn render_rule_item(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let rule = &self.rules[index];
        let is_enabled = rule.enabled;
        let type_tag = match rule.rule_type.as_str() {
            "domain" => Tag::primary().child("Domain"),
            "domain-suffix" => Tag::info().child("Suffix"),
            "domain-keyword" => Tag::warning().child("Keyword"),
            "ip-cidr" => Tag::success().child("CIDR"),
            "geoip" => Tag::danger().child("GeoIP"),
            "match" => Tag::secondary().child("Match"),
            _ => Tag::secondary().child(rule.rule_type.clone()),
        };
        let target_color = if !is_enabled {
            rgb(TEXT_MUTED)
        } else {
            match rule.target.as_str() {
                "direct" => rgb(SUCCESS_COLOR),
                "proxy" => rgb(ACCENT),
                "reject" => rgb(DANGER_COLOR),
                _ => rgb(TEXT_SECONDARY),
            }
        };
        let pattern = rule.pattern.clone();
        let target = rule.target.clone();
        let text_color = if is_enabled {
            rgb(TEXT_PRIMARY)
        } else {
            rgb(TEXT_MUTED)
        };
        let toggle_label = if is_enabled { "On" } else { "Off" };
        let toggle_tooltip = if is_enabled {
            "Disable rule"
        } else {
            "Enable rule"
        };

        // Enable/disable indicator: left border color
        let left_bar_color = if is_enabled { ACCENT } else { BORDER_COLOR };

        div()
            .id(SharedString::from(format!("rule-{}", index)))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pl_2()
            .pr_2()
            .py_1p5()
            .rounded(px(4.0))
            .bg(rgb(BG_CARD))
            .border_1()
            .border_color(rgb(BORDER_COLOR))
            .when(!is_enabled, |d| d.opacity(0.5))
            .hover(|s| s.bg(rgb(BG_CARD_HOVER)).border_color(rgb(ACCENT_DIM)))
            // Colored left bar indicating enabled state
            .child(
                div()
                    .w(px(3.0))
                    .h(px(20.0))
                    .rounded(px(2.0))
                    .bg(rgb(left_bar_color)),
            )
            // Index number
            .child(
                div()
                    .w(px(22.0))
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child(format!("{}", index + 1)),
            )
            // Type tag
            .child(div().child(type_tag))
            // Pattern (flex_1, truncated)
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font(localized_font())
                    .text_color(text_color)
                    .overflow_x_hidden()
                    .child(pattern),
            )
            // Target with semantic color
            .child(
                div()
                    .w(px(50.0))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(target_color)
                    .child(target),
            )
            // Actions: compact buttons
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_0p5()
                    .items_center()
                    .child(
                        Button::new(("rule-toggle", index))
                            .xsmall()
                            .label(toggle_label.to_string())
                            .tooltip(toggle_tooltip)
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(rule) = this.rules.get_mut(index) {
                                    rule.enabled = !rule.enabled;
                                }
                                this.persist_gui_state();
                                if this.proxy_running && this.proxy_mode == ProxyMode::Rule {
                                    this.restart_proxy_with_current_state(cx);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("rule-up", index))
                            .xsmall()
                            .label("↑".to_string())
                            .tooltip("Move rule up (higher priority)")
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_rule_up(index, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-down", index))
                            .xsmall()
                            .label("↓".to_string())
                            .tooltip("Move rule down (lower priority)")
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_rule_down(index, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-edit", index))
                            .xsmall()
                            .label("Edit".to_string())
                            .tooltip("Edit this rule")
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_edit_rule(index, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-del", index))
                            .xsmall()
                            .label("×".to_string())
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

        let total_count = entries.len();
        let current_level = self.log_level_filter;

        // Sync filter from InputState (same pattern as node filter)
        let filter_text = self.log_filter_input.read(cx).value().trim().to_lowercase();
        if filter_text != self.log_filter {
            self.log_filter = filter_text;
        }
        let log_filter = self.log_filter.to_lowercase();

        // Apply both level and text filters
        let filtered: Vec<&crate::log_buffer::LogEntry> = entries
            .iter()
            .filter(|e| {
                // Level filter: only show entries at or above the selected level
                let level_ok = e.level <= current_level;
                let text_ok = log_filter.is_empty()
                    || e.message.to_lowercase().contains(&log_filter)
                    || e.target.to_lowercase().contains(&log_filter);
                level_ok && text_ok
            })
            .collect();

        let filtered_count = filtered.len();
        let log_filter_input = self.log_filter_input.clone();

        let level_chips = {
            let levels: &[(&str, tracing::Level, u32)] = &[
                ("ERR", tracing::Level::ERROR, DANGER_COLOR),
                ("WRN", tracing::Level::WARN, WARNING_COLOR),
                ("INF", tracing::Level::INFO, SUCCESS_COLOR),
                ("DBG", tracing::Level::DEBUG, TEXT_MUTED),
            ];
            let mut row = div().flex().flex_row().gap_1();
            for &(label, level, color) in levels {
                let is_active = current_level == level;
                let btn = Button::new(SharedString::from(format!("log-lvl-{}", label)))
                    .xsmall()
                    .label(label.to_string())
                    .when(is_active, |b| b.primary())
                    .when(!is_active, |b| b.ghost())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.log_level_filter = level;
                        cx.notify();
                    }));
                let chip = div()
                    .when(!is_active, |d| d.text_color(rgb(color)))
                    .child(btn);
                row = row.child(chip);
            }
            row
        };

        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb_3()
                    .mb_1()
                    .border_b_1()
                    .border_color(rgb(BORDER_COLOR))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Logs"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                    .child(
                                        if log_filter.is_empty() && filtered_count == total_count {
                                            format!("{}", total_count)
                                        } else {
                                            format!("{}/{}", filtered_count, total_count)
                                        },
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                Button::new("refresh-logs")
                                    .xsmall()
                                    .label("Refresh".to_string())
                                    .tooltip("Refresh log display")
                                    .ghost()
                                    .on_click(cx.listener(|_this, _, _, cx| {
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("clear-logs")
                                    .xsmall()
                                    .label("Clear".to_string())
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
            )
            // Level filter chips + text search row
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(level_chips)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(gpui_component::input::Input::new(&log_filter_input).w_full()),
                    )
                    .when(!self.log_filter.is_empty(), |d| {
                        d.child(
                            Button::new("clear-log-filter")
                                .xsmall()
                                .label("Clear".to_string())
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_log_filter(window, cx);
                                })),
                        )
                    }),
            );

        if filtered.is_empty() {
            content = content.child(
                self.card().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .py_8()
                        .gap_2()
                        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child(
                            if total_count == 0 {
                                "No log entries yet"
                            } else {
                                "No entries match the filter"
                            },
                        ))
                        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(
                            if total_count == 0 {
                                "Logs will appear here as the proxy operates"
                            } else {
                                "Try a different level or clear the search filter"
                            },
                        )),
                ),
            );
        } else {
            let mut list = div()
                .flex()
                .flex_col()
                .gap_0()
                .p_2()
                .rounded(px(4.0))
                .bg(rgb(BG_SIDEBAR))
                .border_1()
                .border_color(rgb(BORDER_COLOR));

            for (log_idx, entry) in filtered.iter().take(500).enumerate() {
                let (level_label, level_color, msg_color) = match entry.level {
                    tracing::Level::ERROR => ("ERR", DANGER_COLOR, DANGER_COLOR),
                    tracing::Level::WARN => ("WRN", WARNING_COLOR, WARNING_COLOR),
                    tracing::Level::INFO => ("INF", SUCCESS_COLOR, TEXT_PRIMARY),
                    tracing::Level::DEBUG => ("DBG", TEXT_MUTED, TEXT_SECONDARY),
                    tracing::Level::TRACE => ("TRC", TEXT_MUTED, TEXT_MUTED),
                };

                // Shorten target: e.g. "vex_core::proxy::tun" → "proxy::tun"
                let short_target = entry
                    .target
                    .strip_prefix("vex_core::")
                    .or_else(|| entry.target.strip_prefix("vex_gui::"))
                    .or_else(|| entry.target.strip_prefix("vex::"))
                    .unwrap_or(&entry.target);

                // Alternate row bg for readability
                let row_bg = if log_idx % 2 == 0 {
                    rgba(0x00000000u32) // transparent
                } else {
                    rgba(0xffffff04u32) // very subtle stripe
                };

                list = list.child(
                    div()
                        .id(SharedString::from(format!("log-{}", log_idx)))
                        .flex()
                        .flex_row()
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .rounded(px(2.0))
                        .bg(row_bg)
                        .hover(|s| s.bg(rgb(BG_CARD_HOVER)))
                        .child(
                            div()
                                .w(px(28.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(level_color))
                                .child(level_label.to_string()),
                        )
                        .child(
                            div()
                                .w(px(80.0))
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .overflow_x_hidden()
                                .child(short_target.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(rgb(msg_color))
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

        let category = self.settings_category.clone();

        div()
            .flex()
            .flex_col()
            .gap_3()
            // Title
            .child(self.page_header("Settings"))
            // Two-column: left nav + right content
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .items_start()
                    // Left: category nav (100px fixed, won't shrink)
                    .child(
                        div()
                            .w(px(100.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child({
                                let active = category == SettingsCategory::ProxyConfig;
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(BG_CARD_HOVER)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT_PRIMARY)))
                                    .when(!active, |d| d.text_color(rgb(TEXT_SECONDARY)).hover(|d| d.bg(rgb(BG_CARD))))
                                    .when(!active, |d| d.text_color(rgb(TEXT_SECONDARY)).border_color(rgba(0x00000000u32)).hover(|d| d.bg(rgb(BG_CARD_HOVER))))
                                    .text_xs()
                                    .child("Proxy Config")
                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.settings_category = SettingsCategory::ProxyConfig;
                                        cx.notify();
                                    }))
                            })
                            .child({
                                let active = category == SettingsCategory::SystemProxy;
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(BG_CARD_HOVER)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT_PRIMARY)))
                                    .when(!active, |d| d.text_color(rgb(TEXT_SECONDARY)).hover(|d| d.bg(rgb(BG_CARD))))
                                    .text_xs()
                                    .child("System Proxy")
                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.settings_category = SettingsCategory::SystemProxy;
                                        cx.notify();
                                    }))
                            })
                            .child({
                                let active = category == SettingsCategory::About;
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(BG_CARD_HOVER)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT_PRIMARY)))
                                    .when(!active, |d| d.text_color(rgb(TEXT_SECONDARY)).hover(|d| d.bg(rgb(BG_CARD))))
                                    .text_xs()
                                    .child("About")
                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.settings_category = SettingsCategory::About;
                                        cx.notify();
                                    }))
                            }),
                    )
                    // Right: content panel
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(match category {
                                SettingsCategory::ProxyConfig => self.card()
                                    .child(self.card_title("Proxy Config"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            .child(self.setting_row("Listen Address", &listen_addr_input))
                                            .child(self.setting_row("SOCKS5 Port", &socks_port_input))
                                            .child(self.setting_row("HTTP Port", &http_port_input))
                                            .child(
                                                Button::new("apply-settings").xsmall()
                                                    .label("Apply Settings".to_string())
                                                    .tooltip("Save and apply proxy settings")
                                                    .primary()
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.apply_settings(cx);
                                                    })),
                                            )
                                            .child(if !settings_status.is_empty() {
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(status_text_color(&settings_status)))
                                                    .child(settings_status)
                                                    .into_any_element()
                                            } else {
                                                div().into_any_element()
                                            })
                                            .child(self.stat_item("Listen", &format!(
                                                "{}:{} (SOCKS5) / {}:{} (HTTP)",
                                                self.listen_addr, self.socks_port, self.listen_addr, self.http_port
                                            ))),
                                    )
                                    .into_any_element(),
                                SettingsCategory::SystemProxy => self.card()
                                    .child(self.card_title("System Proxy"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_0()
                                                    .child(self.stat_item(
                                                        "Status",
                                                        if self.system_proxy_managed_by_app {
                                                            "Managed by Vex"
                                                        } else if self.system_proxy_enabled {
                                                            "External proxy"
                                                        } else {
                                                            "Disabled"
                                                        },
                                                    ))
                                                    .child(self.stat_item("HTTP", &format!("{}:{}", self.listen_addr, self.http_port)))
                                                    .child(self.stat_item("SOCKS5", &format!("{}:{}", self.listen_addr, self.socks_port)))
                                                    .child(self.stat_item("OS status", &self.system_proxy_status.clone())),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("enable-system-proxy").xsmall()
                                                            .label(if self.system_proxy_enabled {
                                                                "Update System Proxy".to_string()
                                                            } else {
                                                                "Set System Proxy".to_string()
                                                            })
                                                            .tooltip("Configure OS to use vex as system proxy")
                                                            .primary()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.enable_system_proxy(cx);
                                                            })),
                                                    )
                                                    .child(
                                                        Button::new("disable-system-proxy").xsmall()
                                                            .label("Clear System Proxy".to_string())
                                                            .tooltip("Remove vex from OS proxy settings")
                                                            .ghost()
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                this.disable_system_proxy(cx);
                                                            })),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .pt_1()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_MUTED))
                                                    .child("System proxy uses the local HTTP endpoint because desktop environments generally consume HTTP proxy settings directly."),
                                            ),
                                    )
                                    .into_any_element(),
                                SettingsCategory::About => self.card()
                                    .child(self.card_title("About Vex"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_3()
                                            // App name + version banner
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap_3()
                                                    .p_3()
                                                    .rounded(px(6.0))
                                                    .bg(rgb(BG_ACCENT_SUBTLE))
                                                    .border_1()
                                                    .border_color(rgb(BORDER_COLOR))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap_0p5()
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_weight(FontWeight::SEMIBOLD)
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .child("Vex"),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(TEXT_MUTED))
                                                                    .child("v0.1.0 · Proxy Client"),
                                                            ),
                                                    ),
                                            )
                                            // Metadata
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_0()
                                                    .child(self.stat_item("Framework", "GPUI (Rust)"))
                                                    .child(self.stat_item("Protocols", "SS · VMess · VLESS · TUIC v5 · Trojan · Hysteria2"))
                                                    .child(self.stat_item("Compatible with", "shoes server")),
                                            ),
                                    )
                                    .into_any_element(),
                            }),
                    ),
            )
    }

    fn setting_row(&self, label: &str, input: &Entity<InputState>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .py_0p5()
            .child(
                div()
                    .w(px(110.0))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(gpui_component::input::Input::new(input)),
            )
    }

    fn render_selected_node_details(
        &mut self,
        node: &Node,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = if self.proxy_running && self.active_proxy_node == Some(index) {
            "Active"
        } else if self.selected_node == Some(index) {
            "Selected"
        } else {
            "Idle"
        };

        let latency_str = node
            .latency_ms
            .map(|ms| format!("{} ms", ms))
            .unwrap_or_else(|| {
                if self.latency_failed.contains(&index) {
                    "Failed".to_string()
                } else {
                    "Not tested".to_string()
                }
            });

        let latency_color = node
            .latency_ms
            .map(|ms| {
                if ms < 100 {
                    SUCCESS_COLOR
                } else if ms < 300 {
                    WARNING_COLOR
                } else {
                    DANGER_COLOR
                }
            })
            .unwrap_or(TEXT_MUTED);

        // Subscription source (from sub_url tag)
        let sub_source = node.extra.get("sub_url").and_then(|url| {
            self.subscriptions
                .iter()
                .find(|s| &s.url == url)
                .map(|s| s.name.clone())
        });

        // UDP support
        let udp_enabled = match &node.protocol {
            ProxyProtocol::Shadowsocks { udp, .. } => *udp,
            ProxyProtocol::VMess { udp, .. } => *udp,
            ProxyProtocol::VLess { udp, .. } => *udp,
            ProxyProtocol::Tuic { udp, .. } => *udp,
            ProxyProtocol::Trojan { udp, .. } => *udp,
            ProxyProtocol::Hysteria2 { udp, .. } => *udp,
        };

        // Masked UUID/password (first 8 chars + "…")
        let masked_id: Option<String> = match &node.protocol {
            ProxyProtocol::VMess { uuid, .. }
            | ProxyProtocol::VLess { uuid, .. }
            | ProxyProtocol::Tuic { uuid, .. } => {
                Some(format!("{}…", uuid.get(..8).unwrap_or(uuid.as_str())))
            }
            _ => None,
        };

        let is_active = self.proxy_running && self.active_proxy_node == Some(index);
        let is_editing = self.editing_node_index == Some(index);
        let node_rename_input = self.node_rename_input.clone();
        let node_tag_input = self.node_tag_input.clone();
        let node_tags = node.tags.clone();

        self.card()
            // Header: node name + status dot
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pb_3()
                    .mb_1()
                    .border_b_1()
                    .border_color(rgb(BORDER_COLOR))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .font(localized_font())
                                    .child(node.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(3.0))
                                    .text_color(if is_active {
                                        rgb(SUCCESS_COLOR)
                                    } else {
                                        rgb(TEXT_MUTED)
                                    })
                                    .bg(if is_active {
                                        rgb(0x1a2e1e)
                                    } else {
                                        rgb(BG_ACCENT_SUBTLE)
                                    })
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .items_center()
                            .child(if is_active {
                                Button::new("deactivate-btn")
                                    .xsmall()
                                    .label("Disconnect".to_string())
                                    .tooltip("Stop proxy and deactivate this node")
                                    .danger()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stop_proxy(cx);
                                    }))
                            } else {
                                Button::new("activate-btn")
                                    .xsmall()
                                    .label("Connect".to_string())
                                    .tooltip("Connect through this node")
                                    .primary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected_node = Some(index);
                                        this.start_proxy(cx);
                                    }))
                            })
                            .child(
                                Button::new("test-lat-btn")
                                    .xsmall()
                                    .label("Test".to_string())
                                    .tooltip("Test latency for this node")
                                    .ghost()
                                    .loading(self.latency_testing.contains(&index))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.test_node_latency(index, cx);
                                    })),
                            )
                            .child(if is_editing {
                                Button::new("rename-cancel-btn")
                                    .xsmall()
                                    .label("Cancel".to_string())
                                    .tooltip("Cancel rename")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editing_node_index = None;
                                        cx.notify();
                                    }))
                            } else {
                                Button::new("rename-btn")
                                    .xsmall()
                                    .label("Rename".to_string())
                                    .tooltip("Rename this node")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.editing_node_index = Some(index);
                                        let current_name = this
                                            .nodes
                                            .get(index)
                                            .map(|n| n.name.clone())
                                            .unwrap_or_default();
                                        this.node_rename_input.update(cx, |state, cx| {
                                            state.set_value(current_name, window, cx);
                                        });
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .when(is_editing, |card| {
                card.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(
                            div().flex_1().child(
                                gpui_component::input::Input::new(&node_rename_input).w_full(),
                            ),
                        )
                        .child(
                            Button::new("rename-confirm-btn")
                                .xsmall()
                                .label("Save".to_string())
                                .tooltip("Confirm rename")
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let new_name =
                                        this.node_rename_input.read(cx).value().trim().to_string();
                                    if !new_name.is_empty() {
                                        if let Some(n) = this.nodes.get_mut(index) {
                                            n.name = new_name;
                                        }
                                        this.persist_gui_state();
                                    }
                                    this.editing_node_index = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(self.stat_item("Status", status))
                    .child(self.stat_item("Protocol", protocol_name(&node.protocol)))
                    .child(self.stat_item("Server", &node.server))
                    .child(self.stat_item("Port", &node.port.to_string()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .py_1p5()
                            .border_b_1()
                            .border_color(rgb(BORDER_COLOR))
                            .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Latency"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(latency_color))
                                    .child(latency_str),
                            ),
                    )
                    .child(self.stat_item("Transport", &transport_summary(node.transport.as_ref())))
                    .child(self.stat_item("Security", &security_summary(node)))
                    .child(self.stat_item("Auth", &auth_summary(&node.protocol)))
                    .when_some(masked_id, |d, id| {
                        d.child(self.stat_item("UUID (partial)", &id))
                    })
                    .child(self.stat_item("UDP", if udp_enabled { "Enabled" } else { "Disabled" }))
                    .when_some(sub_source, |d, name| {
                        d.child(self.stat_item("Subscription", &name))
                    })
                    // Tags section
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_1p5()
                            .border_b_1()
                            .border_color(rgb(BORDER_COLOR))
                            .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child("Tags"))
                            .when(!node_tags.is_empty(), |d| {
                                let mut tag_row = div().flex().flex_row().gap_1().flex_wrap();
                                for tag in &node_tags {
                                    let tag_clone = tag.clone();
                                    tag_row = tag_row.child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_0p5()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(4.0))
                                            .bg(rgb(BG_ACCENT_SUBTLE))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .child(format!("#{}", tag)),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "del-tag-{}",
                                                    tag_clone
                                                )))
                                                .xsmall()
                                                .label("×".to_string())
                                                .ghost()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(n) = this.nodes.get_mut(index) {
                                                        n.tags.retain(|t| t != &tag_clone);
                                                    }
                                                    this.persist_gui_state();
                                                    cx.notify();
                                                })),
                                            ),
                                    );
                                }
                                d.child(tag_row)
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_1()
                                    .items_center()
                                    .child(div().flex_1().min_w_0().child(
                                        gpui_component::input::Input::new(&node_tag_input).w_full(),
                                    ))
                                    .child(
                                        Button::new("add-tag-btn")
                                            .xsmall()
                                            .label("Add".to_string())
                                            .tooltip("Add tag to this node")
                                            .ghost()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let tag = this
                                                    .node_tag_input
                                                    .read(cx)
                                                    .value()
                                                    .trim()
                                                    .to_string();
                                                if tag.is_empty() {
                                                    return;
                                                }
                                                if let Some(n) = this.nodes.get_mut(index) {
                                                    if !n.tags.contains(&tag) {
                                                        n.tags.push(tag);
                                                    }
                                                }
                                                this.node_tag_input.update(cx, |s, cx| {
                                                    s.set_value("", window, cx);
                                                });
                                                this.persist_gui_state();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .pt_2()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(protocol_note(&node.protocol)),
                    ),
            )
    }
}

// === Helpers ===

/// Render a 3-step startup progress bar for the proxy connection.
/// `phase`: 0 = idle, 1 = service starting, 2 = verifying, 3 = done
fn render_proxy_steps(phase: u8) -> Div {
    let step_dot = |completed: bool, active: bool| {
        let (bg_color, border_color) = if completed {
            (SUCCESS_COLOR, SUCCESS_COLOR)
        } else if active {
            (ACCENT_DIM, ACCENT_DIM)
        } else {
            (0x2b2b2bu32, 0x383838u32)
        };
        div()
            .w(px(8.0))
            .h(px(8.0))
            .rounded_full()
            .bg(rgb(bg_color))
            .border_1()
            .border_color(rgb(border_color))
    };
    let step_line = |filled: bool| {
        let color = if filled { SUCCESS_COLOR } else { 0x2b2b2bu32 };
        div().h(px(2.0)).w(px(32.0)).bg(rgb(color))
    };
    let step_label = |text: &str, active: bool, done: bool| {
        let color = if done {
            SUCCESS_COLOR
        } else if active {
            ACCENT_DIM
        } else {
            TEXT_MUTED
        };
        div()
            .text_xs()
            .text_color(rgb(color))
            .child(text.to_string())
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        // Step 1
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_0p5()
                .child(step_dot(phase >= 2, phase == 1))
                .child(step_label("Start", phase == 1, phase >= 2)),
        )
        .child(step_line(phase >= 2))
        // Step 2
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_0p5()
                .child(step_dot(phase >= 3, phase == 2))
                .child(step_label("Verify", phase == 2, phase >= 3)),
        )
        .child(step_line(phase >= 3))
        // Step 3
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_0p5()
                .child(step_dot(phase >= 3, false))
                .child(step_label("Done", false, phase >= 3)),
        )
}

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

fn format_speed(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;
    if bps >= GB {
        format!("{:.1} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.1} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
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
    // Brief startup delay: let the proxy finish route table setup and any
    // per-protocol handshaking before we start probing.  800ms is enough for
    // the local listener to be ready; further latency is absorbed by the probe
    // timeout itself.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    // Quick liveness check: verify the local port is accepting connections
    // before sending any external CONNECT requests.  This avoids wasting the
    // full probe timeout when the proxy failed to bind.
    let local_ok = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect((listen_addr, http_port)),
    )
    .await;
    if !matches!(local_ok, Ok(Ok(_))) {
        anyhow::bail!(
            "local proxy not accepting connections on {}:{}",
            listen_addr,
            http_port
        );
    }

    // Per-probe timeout: use a shorter first-attempt timeout so we can
    // iterate to the retry faster when the node is temporarily slow.
    let first_timeout = std::cmp::min(timeout, std::time::Duration::from_secs(5));

    // Three well-known CONNECT-friendly endpoints.  They are probed in
    // parallel; a single success is sufficient to declare the proxy healthy.
    let mut errors = Vec::new();

    for attempt in 1..=2u32 {
        let probe_timeout = if attempt == 1 { first_timeout } else { timeout };
        let (r1, r2, r3) = tokio::join!(
            verify_local_http_proxy_once(listen_addr, http_port, probe_timeout, "www.gstatic.com",),
            verify_local_http_proxy_once(
                listen_addr,
                http_port,
                probe_timeout,
                "cp.cloudflare.com",
            ),
            verify_local_http_proxy_once(listen_addr, http_port, probe_timeout, "dns.google",),
        );
        match (r1, r2, r3) {
            // Any single success counts: proxy is reachable.
            (Ok(s), _, _) | (_, Ok(s), _) | (_, _, Ok(s)) => {
                return if attempt == 1 {
                    Ok(format!("Connected — {}", s))
                } else {
                    Ok(format!("Connected — {} (after retry)", s))
                };
            }
            (Err(e1), Err(e2), Err(e3)) => {
                errors.push(format!(
                    "attempt {}: gstatic: {}; cloudflare: {}; dns.google: {}",
                    attempt, e1, e2, e3
                ));
                if attempt < 2 {
                    // Wait briefly before retrying — lets transient proxy
                    // warm-up issues resolve without a long global timeout.
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                }
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
    // Accept "200" anywhere after the HTTP version — some proxies omit
    // the trailing space (e.g. "HTTP/1.1 200" without a reason phrase).
    let code = status_line.splitn(3, ' ').nth(1).unwrap_or_default().trim();
    if code != "200" {
        anyhow::bail!("tunnel establishment failed: {}", status_line);
    }

    // CONNECT 200 means the local proxy successfully connected to the upstream
    // node and established a tunnel.  That is sufficient proof the proxy is
    // routing traffic.  A full TLS handshake through the tunnel is skipped
    // because many VPN nodes perform TLS interception or have stricter TLS
    // policies for outbound verification probes, causing false negatives.
    Ok(format!("tunnel established via {}", target_host))
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
    Some(base.join("vex").join("gui-state.yaml"))
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
    use vex_core::config::model::RoutingRule;
    use vex_core::{RouteAction, decode_display_name};

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
                enabled: true,
            },
            RoutingRule {
                rule_type: "match".to_string(),
                pattern: "*".to_string(),
                target: "proxy".to_string(),
                enabled: true,
            },
        ];

        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("api.example.com", 443), RouteAction::Reject);
    }
}
