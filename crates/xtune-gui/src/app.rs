use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::InputState;
use gpui_component::tag::{Tag, TagVariant};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use xtune_core::config::model::{
    AppConfig, Node, ProxyProtocol, Subscription, TransportConfig, TransportType,
};
use xtune_core::proxy::ProxyStats;
use xtune_core::{
    ProxyService, SharedOutbound, clear_system_proxy as clear_os_proxy, create_outbound,
    fetch_subscription, get_system_proxy as get_os_proxy, normalize_node_names,
    set_system_proxy as set_os_proxy, system_proxy_supported,
};

// Color palette
const BG_PRIMARY: u32 = 0x1a1a2e;
const BG_SIDEBAR: u32 = 0x16213e;
const BG_CARD: u32 = 0x1f2940;
const BG_CARD_HOVER: u32 = 0x263352;
const BORDER_COLOR: u32 = 0x2a2a4a;
const ACCENT: u32 = 0x00d4ff;
const TEXT_PRIMARY: u32 = 0xe0e0e0;
const TEXT_SECONDARY: u32 = 0x888888;
const TEXT_MUTED: u32 = 0x666666;
const SUCCESS_COLOR: u32 = 0x4ade80;
const WARNING_COLOR: u32 = 0xfbbf24;
const DANGER_COLOR: u32 = 0xf87171;

/// Main application state
pub struct AppState {
    active_view: ActiveView,

    // Proxy state
    proxy_running: bool,
    proxy_status: String,
    proxy_validation_status: String,
    proxy_session_id: u64,

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
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActiveView {
    Home,
    Nodes,
    Config,
    Settings,
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
        Err(err) => (false, format!("Error: {}", err)),
    }
}

impl AppState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        tokio_handle: tokio::runtime::Handle,
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

        Self {
            active_view: ActiveView::Home,
            proxy_running: false,
            proxy_status: "Disconnected".to_string(),
            proxy_validation_status: "Not validated".to_string(),
            proxy_session_id: 0,
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
            listen_addr: persisted.listen_addr,
            socks_port: persisted.socks_port,
            http_port: persisted.http_port,
            system_proxy_enabled,
            system_proxy_status,
            system_proxy_managed_by_app: false,
            proxy_stats: None,
            tokio_handle,
            proxy_stop_tx: None,
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
                Ok(o) => o,
                Err(e) => {
                    self.proxy_status = format!("Error: {}", e);
                    cx.notify();
                    return;
                }
            },
            None => SharedOutbound::direct(),
        };

        let service = ProxyService::with_outbound(outbound);
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
                            "Verifying Google reachability...".to_string();
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
                        this.proxy_status = format!("Error: {}", err);
                        this.proxy_validation_status = "Not validated".to_string();
                        cx.notify();
                    })
                    .ok();
                    false
                }
                Err(_) => false,
            };

            if ready_ok {
                let addr_for_check = listen_addr.clone();
                let proxy_check = handle
                    .spawn(async move {
                        verify_local_http_proxy(
                            &addr_for_check,
                            http_port,
                            std::time::Duration::from_secs(10),
                        )
                        .await
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("task join error: {}", e))
                    .and_then(|r| r);
                weak.update(cx, |this: &mut AppState, cx| {
                    if this.proxy_session_id != session_id {
                        return;
                    }
                    this.proxy_validation_status = match proxy_check {
                        Ok(summary) => format!("Verified — {}", summary),
                        Err(err) => format!("Validation failed — {}", err),
                    };
                    cx.notify();
                })
                .ok();
            }

            // Wait for proxy to stop
            let result = join.await;

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
                    Ok(Err(e)) => this.proxy_status = format!("Error: {}", e),
                    Err(e) => this.proxy_status = format!("Error: {}", e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn stop_proxy(&mut self, cx: &mut Context<Self>) {
        if let Some(tx) = self.proxy_stop_tx.take() {
            let _ = tx.send(());
        }
        self.proxy_status = "Disconnecting...".to_string();
        self.proxy_validation_status = "Stopping local proxy".to_string();
        cx.notify();
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

        let handle = self.tokio_handle.clone();

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move {
                    let outbound = match create_outbound(&node) {
                        Ok(o) => o,
                        Err(_) => {
                            // Fallback: raw TCP latency if outbound creation fails
                            let addr = format!("{}:{}", node.server, node.port);
                            let start = std::time::Instant::now();
                            let timeout = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                tokio::net::TcpStream::connect(&addr),
                            )
                            .await;
                            return match timeout {
                                Ok(Ok(_)) => Ok(start.elapsed().as_millis() as u32),
                                Ok(Err(e)) => Err(format!("{}", e)),
                                Err(_) => Err("Timeout".to_string()),
                            };
                        }
                    };
                    xtune_core::latency_test_node(&outbound, 15)
                        .await
                        .map_err(|e| format!("{}", e))
                })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
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
        let addr = self.listen_addr_input.read(cx).value().to_string();
        let socks = self.socks_port_input.read(cx).value().to_string();
        let http = self.http_port_input.read(cx).value().to_string();

        if !addr.is_empty() {
            self.listen_addr = addr;
        }
        if let Ok(p) = socks.parse::<u16>() {
            self.socks_port = p;
        }
        if let Ok(p) = http.parse::<u16>() {
            self.http_port = p;
        }
        self.persist_gui_state();
        cx.notify();
    }

    fn enable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match set_os_proxy(&self.listen_addr, self.http_port) {
            Ok(()) => {
                self.system_proxy_managed_by_app = false;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(err.to_string()), cx),
        }
    }

    fn disable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match clear_os_proxy() {
            Ok(()) => {
                self.system_proxy_managed_by_app = false;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(err.to_string()), cx),
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

    // === Node Selection ===

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);
        self.persist_gui_state();
        cx.notify();
    }

    fn activate_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);

        if self.proxy_running {
            self.stop_proxy(cx);
            let handle = self.tokio_handle.clone();
            cx.spawn(async move |weak, cx| {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    })
                    .await
                    .ok();
                weak.update(cx, |this: &mut AppState, cx| {
                    this.start_proxy(cx);
                })
                .ok();
            })
            .detach();
            return;
        }

        self.start_proxy(cx);
    }

    fn persist_gui_state(&self) {
        let config = AppConfig {
            listen_addr: self.listen_addr.clone(),
            socks_port: self.socks_port,
            http_port: self.http_port,
            nodes: self.nodes.clone(),
            active_node: self.selected_node,
            subscriptions: Vec::new(),
            rules: Vec::new(),
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
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(BG_PRIMARY))
            .font(ui_font())
            .text_color(rgb(TEXT_PRIMARY))
            .child(self.render_sidebar(cx))
            .child(self.render_content(cx))
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
            .justify_between()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER_COLOR))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        // Logo
                        div().px_4().py_4().child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(ACCENT))
                                .child("⚡ XTune"),
                        ),
                    )
                    .child(
                        // Nav items
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .px_2()
                            .child(self.nav_item("🏠  Home", ActiveView::Home, &active, cx))
                            .child(self.nav_item("📡  Nodes", ActiveView::Nodes, &active, cx))
                            .child(self.nav_item("⬇️  Config", ActiveView::Config, &active, cx))
                            .child(self.nav_item(
                                "⚙️  Settings",
                                ActiveView::Settings,
                                &active,
                                cx,
                            )),
                    ),
            )
            .child(
                // Bottom info
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(BORDER_COLOR))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child("v0.1.0 • Rust Proxy Client"),
                    ),
            )
    }

    fn nav_item(
        &mut self,
        label: &str,
        view: ActiveView,
        active: &ActiveView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = *active == view;
        let label_owned = label.to_string();

        let btn = Button::new(SharedString::from(label_owned.clone()))
            .label(label_owned)
            .w_full()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(view.clone(), cx);
            }));

        if is_active {
            btn.primary()
        } else {
            btn.ghost()
        }
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
                ActiveView::Settings => self.render_settings(cx).into_any_element(),
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
        let system_proxy_mode = if self.system_proxy_managed_by_app {
            "Managed by XTune"
        } else if self.system_proxy_enabled {
            "Enabled externally"
        } else {
            "Disabled"
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

        div()
            .flex()
            .flex_col()
            .gap_4()
            // Title
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("XTune Proxy Client"),
            )
            // Status card
            .child(
                self.card().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .rounded_full()
                                        .bg(status_color),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!("Status: {}", self.proxy_status)),
                                ),
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
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!(
                                    "Proxy Validation: {}",
                                    self.proxy_validation_status
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!("System Proxy Mode: {}", system_proxy_mode)),
                        )
                        .child({
                            let connect_btn = Button::new("connect-btn")
                                .label(btn_label.to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_proxy(cx);
                                }));
                            if self.proxy_running {
                                connect_btn.ghost()
                            } else {
                                connect_btn.primary()
                            }
                        }),
                ),
            )
            // Proxy info card
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_2()
                            .child("Proxy Endpoints"),
                    )
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
                            .mb_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Statistics"),
                            )
                            .child(
                                Button::new("refresh-stats")
                                    .label("↻ Refresh".to_string())
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
                            .child(self.stat_item("Nodes", &self.nodes.len().to_string())),
                    ),
            )
            // Protocols card
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_2()
                            .child("Supported Protocols"),
                    )
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
            .p_4()
            .rounded_lg()
            .bg(rgb(BG_CARD))
            .border_1()
            .border_color(rgb(BORDER_COLOR))
    }

    fn info_row(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT_SECONDARY))
                    .w(px(60.0))
                    .child(format!("{}:", label)),
            )
            .child(
                div()
                    .text_sm()
                    .font(localized_font())
                    .child(value.to_string()),
            )
    }

    fn stat_item(&self, label: &str, value: &str) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(ACCENT))
                    .child(value.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(label.to_string()),
            )
    }
}

// === Nodes View ===

impl AppState {
    fn render_nodes(&mut self, cx: &mut Context<Self>) -> Div {
        let node_count = self.nodes.len();

        let mut content = div().flex().flex_col().gap_4().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .child(format!("Nodes ({})", node_count)),
                )
                .child(
                    Button::new("test-all-btn")
                        .label("⚡ Test All".to_string())
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.test_all_latency(cx);
                        })),
                ),
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
            for i in 0..node_count {
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
            rgb(0x204262)
        } else if is_selected {
            rgb(BG_CARD_HOVER)
        } else {
            rgb(BG_CARD)
        };

        let select_indicator_bg = if is_active || is_selected {
            rgb(ACCENT)
        } else {
            rgb(BG_CARD)
        };

        let border = if is_active || is_selected {
            rgb(ACCENT)
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
            .rounded_lg()
            .bg(bg)
            .border_1()
            .border_color(border)
            .cursor_pointer()
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
                // Latency
                div()
                    .w(px(64.0))
                    .text_sm()
                    .text_color(latency_color)
                    .child(latency),
            )
            .child({
                let button = Button::new(("activate-node", index))
                    .label(action_label.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_node(index, cx);
                    }));
                if is_active {
                    button.primary()
                } else {
                    button.ghost()
                }
            })
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

        div()
            .flex()
            .flex_col()
            .gap_4()
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
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_3()
                            .child("Import Subscription"),
                    )
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
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_subscription(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("clear-btn")
                                            .label("🗑 Clear Nodes".to_string())
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.clear_nodes(cx);
                                            })),
                                    ),
                            )
                            .child(if !import_status.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(if import_status.starts_with('✓') {
                                        rgb(SUCCESS_COLOR)
                                    } else if import_status.starts_with('✗') {
                                        rgb(DANGER_COLOR)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .child(import_status)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    ),
            )
            // Node summary card
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_3()
                            .child(format!("Node Summary — {} total", node_count)),
                    )
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

// === Settings View ===

impl AppState {
    fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let listen_addr_input = self.listen_addr_input.clone();
        let socks_port_input = self.socks_port_input.clone();
        let http_port_input = self.http_port_input.clone();

        div()
            .flex()
            .flex_col()
            .gap_4()
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
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_3()
                            .child("Proxy Settings"),
                    )
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
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_settings(cx);
                                    })),
                            )
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
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.enable_system_proxy(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("disable-system-proxy")
                                            .label("🧹 Clear System Proxy".to_string())
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
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_3()
                            .child("System Proxy Status"),
                    )
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
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb_3()
                            .child("About"),
                    )
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
            .gap_3()
            .child(
                div()
                    .w(px(120.0))
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

        self.card()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .mb_3()
                    .font(localized_font())
                    .child(format!("Node Details — {}", node.name)),
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
    let mut stream = tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect((listen_addr, http_port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out connecting to local HTTP proxy"))??;

    stream
        .write_all(
            b"CONNECT www.google.com:443 HTTP/1.1\r\nHost: www.google.com:443\r\nProxy-Connection: keep-alive\r\n\r\n",
        )
        .await?;

    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(timeout, stream.read(&mut buf))
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for proxy response"))??;
    if read == 0 {
        anyhow::bail!("local proxy returned no response");
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
        anyhow::bail!("google tunnel failed: {}", status_line);
    }

    // Phase 2: Perform a TLS handshake over the tunnel to fully verify connectivity
    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config));
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from("www.google.com")
        .map_err(|e| anyhow::anyhow!("invalid server name: {}", e))?;

    let tls_stream = tokio::time::timeout(timeout, connector.connect(server_name.to_owned(), stream))
        .await
        .map_err(|_| anyhow::anyhow!("TLS handshake timed out"))?
        .map_err(|e| anyhow::anyhow!("TLS handshake failed: {}", e))?;

    let (mut reader, mut writer) = tokio::io::split(tls_stream);
    writer
        .write_all(b"GET /generate_204 HTTP/1.1\r\nHost: www.google.com\r\nConnection: close\r\n\r\n")
        .await?;

    let mut resp_buf = vec![0u8; 1024];
    let n = tokio::time::timeout(timeout, reader.read(&mut resp_buf))
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for Google response"))??;

    let resp = String::from_utf8_lossy(&resp_buf[..n]);
    let resp_status = resp.lines().next().unwrap_or_default().trim().to_string();

    if resp_status.contains("204") || resp_status.contains("200") {
        Ok(format!("Google reachable via TLS ({})", resp_status))
    } else {
        Ok(format!("Tunnel OK but Google returned: {}", resp_status))
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
    use xtune_core::decode_display_name;

    #[test]
    fn decode_display_name_handles_double_encoded_names() {
        assert_eq!(
            decode_display_name("%25E9%25A9%25AC%25E6%259D%25A5%25E8%25A5%25BF%25E4%25BA%259A"),
            "马来西亚"
        );
    }
}
