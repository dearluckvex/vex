use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::InputState;
use gpui_component::tag::{Tag, TagVariant};

use xtune_core::config::model::{Node, ProxyProtocol, Subscription};
use xtune_core::proxy::ProxyStats;
use xtune_core::{ProxyService, SharedOutbound, create_outbound, fetch_subscription};

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

    // Node management
    nodes: Vec<Node>,
    selected_node: Option<usize>,

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

impl AppState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        tokio_handle: tokio::runtime::Handle,
    ) -> Self {
        let import_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://example.com/subscribe?token=...")
        });
        let listen_addr_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("127.0.0.1")
                .default_value("127.0.0.1")
        });
        let socks_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1080")
                .default_value("1080")
        });
        let http_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1087")
                .default_value("1087")
        });

        Self {
            active_view: ActiveView::Home,
            proxy_running: false,
            proxy_status: "Disconnected".to_string(),
            nodes: Vec::new(),
            selected_node: None,
            import_url_input,
            listen_addr_input,
            socks_port_input,
            http_port_input,
            import_status: String::new(),
            listen_addr: "127.0.0.1".to_string(),
            socks_port: 1080,
            http_port: 1087,
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

        let node = self.selected_node.and_then(|i| self.nodes.get(i).cloned());

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
        self.proxy_stop_tx = Some(stop_tx);
        self.proxy_stats = Some(stats);
        self.proxy_running = true;
        self.proxy_status = "Connecting...".to_string();
        cx.notify();

        let listen_addr = self.listen_addr.clone();
        let socks_port = self.socks_port;
        let http_port = self.http_port;
        let handle = self.tokio_handle.clone();

        cx.spawn(async move |weak, cx| {
            let join = handle.spawn(async move {
                let mut service = service;
                service.start(&listen_addr, socks_port, http_port).await?;
                // Wait for stop signal
                let _ = stop_rx.await;
                service.stop().await;
                Ok::<_, anyhow::Error>(())
            });

            // Update status to connected
            weak.update(cx, |this: &mut AppState, cx| {
                if this.proxy_running {
                    this.proxy_status = "Connected".to_string();
                    cx.notify();
                }
            })
            .ok();

            // Wait for proxy to stop
            let result = join.await;

            weak.update(cx, |this: &mut AppState, cx| {
                this.proxy_running = false;
                this.proxy_stop_tx = None;
                this.proxy_stats = None;
                match result {
                    Ok(Ok(())) => this.proxy_status = "Disconnected".to_string(),
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
        self.proxy_running = false;
        self.proxy_status = "Disconnecting...".to_string();
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
                    Ok(Ok(new_nodes)) => {
                        let count = new_nodes.len();
                        this.nodes.extend(new_nodes);
                        this.import_status =
                            format!("✓ Imported {} nodes (total: {})", count, this.nodes.len());
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
        self.nodes.clear();
        self.selected_node = None;
        self.import_status = "Nodes cleared".to_string();
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
                    let addr = format!("{}:{}", node.server, node.port);
                    let start = std::time::Instant::now();
                    let timeout = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        tokio::net::TcpStream::connect(&addr),
                    )
                    .await;
                    match timeout {
                        Ok(Ok(_)) => Ok(start.elapsed().as_millis() as u32),
                        Ok(Err(e)) => Err(format!("{}", e)),
                        Err(_) => Err("Timeout".to_string()),
                    }
                })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                if let Some(n) = this.nodes.get_mut(index) {
                    match result {
                        Ok(Ok(ms)) => n.latency_ms = Some(ms),
                        Ok(Err(_)) | Err(_) => n.latency_ms = Some(9999),
                    }
                }
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
        cx.notify();
    }

    // === Node Selection ===

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);
        cx.notify();
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
                        div()
                            .px_4()
                            .py_4()
                            .child(
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
            .flex_1()
            .h_full()
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
            .child(self.card().child(
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
                            .child(format!("Active Node: {}", selected_name)),
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
            ))
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
                        div().flex().flex_row().flex_wrap().gap_2()
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
        }

        content
    }

    fn render_node_item(&mut self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let node = &self.nodes[index];
        let is_selected = self.selected_node == Some(index);
        let name = node.name.clone();
        let server = format!("{}:{}", node.server, node.port);
        let protocol_tag = protocol_tag_label(&node.protocol);
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

        let bg = if is_selected {
            rgb(BG_CARD_HOVER)
        } else {
            rgb(BG_CARD)
        };

        let select_indicator_bg = if is_selected {
            rgb(ACCENT)
        } else {
            rgb(BG_CARD)
        };

        let border = if is_selected {
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
                            .child(self.setting_row(
                                "Listen Address",
                                &listen_addr_input,
                            ))
                            .child(self.setting_row(
                                "SOCKS5 Port",
                                &socks_port_input,
                            ))
                            .child(self.setting_row(
                                "HTTP Port",
                                &http_port_input,
                            ))
                            .child(
                                Button::new("apply-settings")
                                    .label("💾 Apply Settings".to_string())
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.apply_settings(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "Current: {}:{} (SOCKS5) / {}:{} (HTTP)",
                                        self.listen_addr,
                                        self.socks_port,
                                        self.listen_addr,
                                        self.http_port
                                    )),
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
                            .child(
                                div()
                                    .text_sm()
                                    .child("XTune v0.1.0"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("A cross-platform Rust proxy client"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(
                                        "Supports: Shadowsocks, VMess, VLESS, TUIC v5, Trojan, Hysteria2",
                                    ),
                            )
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
}

// === Helpers ===

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
