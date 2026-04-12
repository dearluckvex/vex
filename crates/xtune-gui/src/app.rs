use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};

/// Main application state
pub struct AppState {
    active_view: ActiveView,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActiveView {
    Home,
    Nodes,
    Config,
    Settings,
}

impl AppState {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            active_view: ActiveView::Home,
        }
    }

    fn set_view(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        self.active_view = view;
        cx.notify();
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0x1a1a2e))
            .text_color(rgb(0xe0e0e0))
            .child(self.render_sidebar(cx))
            .child(self.render_content(&active))
    }
}

impl AppState {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        div()
            .w(px(200.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x16213e))
            .border_r_1()
            .border_color(rgb(0x2a2a4a))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x00d4ff))
                            .child("⚡ XTune"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_2()
                    .child(self.nav_item("Home", ActiveView::Home, &active, cx))
                    .child(self.nav_item("Nodes", ActiveView::Nodes, &active, cx))
                    .child(self.nav_item("Config", ActiveView::Config, &active, cx))
                    .child(self.nav_item("Settings", ActiveView::Settings, &active, cx)),
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
        let label_str = label.to_string();

        let btn = Button::new(SharedString::from(label_str.clone()))
            .label(label_str)
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

    fn render_content(&self, active: &ActiveView) -> impl IntoElement {
        div()
            .flex_1()
            .h_full()
            .p_6()
            .child(match active {
                ActiveView::Home => self.render_home(),
                ActiveView::Nodes => self.render_placeholder("Nodes", "Node list coming soon..."),
                ActiveView::Config => {
                    self.render_placeholder("Configuration", "Config import coming soon...")
                }
                ActiveView::Settings => {
                    self.render_placeholder("Settings", "Settings coming soon...")
                }
            })
    }

    fn render_home(&self) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("XTune Proxy Client"),
            )
            .child(
                div()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0x1f2940))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgb(0x00d4ff))
                            .child("Status: Disconnected"),
                    )
                    .child(div().text_sm().text_color(rgb(0x888888)).child(
                        "SOCKS5: 127.0.0.1:1080 | HTTP: 127.0.0.1:1087",
                    )),
            )
            .child(
                div()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0x1f2940))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Supported Protocols"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xaaaaaa))
                            .child("Shadowsocks • VMess • VLESS • TUIC v5 • Trojan • Hysteria2"),
                    ),
            )
    }

    fn render_placeholder(&self, title: &str, desc: &str) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_color(rgb(0x888888))
                    .child(desc.to_string()),
            )
    }
}
