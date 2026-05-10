// Hide the Windows console window for the GUI app.
// The in-app Logs panel captures all output instead.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod components;
mod log_buffer;
mod views;

use gpui::*;
use gpui_component::*;
use std::borrow::Cow;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Asset source that embeds the icons directory at compile time.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        // Strip leading slash if present
        let path = path.trim_start_matches('/');
        // Walk the statically embedded directory map
        match ICON_ASSETS.iter().find(|(p, _)| *p == path) {
            Some((_, data)) => Ok(Some(Cow::Borrowed(data))),
            None => Ok(None),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(ICON_ASSETS
            .iter()
            .filter_map(|(p, _)| {
                let p = *p;
                if p.starts_with(prefix) {
                    Some(SharedString::from(p))
                } else {
                    None
                }
            })
            .collect())
    }
}

/// Statically embedded icon assets.
static ICON_ASSETS: &[(&str, &[u8])] = &[
    (
        "icons/window-close.svg",
        include_bytes!("../assets/icons/window-close.svg"),
    ),
    (
        "icons/window-minimize.svg",
        include_bytes!("../assets/icons/window-minimize.svg"),
    ),
    (
        "icons/window-maximize.svg",
        include_bytes!("../assets/icons/window-maximize.svg"),
    ),
    (
        "icons/window-restore.svg",
        include_bytes!("../assets/icons/window-restore.svg"),
    ),
    (
        "icons/layout-dashboard.svg",
        include_bytes!("../assets/icons/layout-dashboard.svg"),
    ),
    (
        "icons/globe.svg",
        include_bytes!("../assets/icons/globe.svg"),
    ),
    (
        "icons/settings-2.svg",
        include_bytes!("../assets/icons/settings-2.svg"),
    ),
    ("icons/map.svg", include_bytes!("../assets/icons/map.svg")),
    (
        "icons/settings.svg",
        include_bytes!("../assets/icons/settings.svg"),
    ),
    (
        "icons/square-terminal.svg",
        include_bytes!("../assets/icons/square-terminal.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/check.svg",
        include_bytes!("../assets/icons/check.svg"),
    ),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/minus.svg",
        include_bytes!("../assets/icons/minus.svg"),
    ),
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    (
        "icons/loader.svg",
        include_bytes!("../assets/icons/loader.svg"),
    ),
    (
        "icons/circle.svg",
        include_bytes!("../assets/icons/circle.svg"),
    ),
    (
        "icons/circle-check.svg",
        include_bytes!("../assets/icons/circle-check.svg"),
    ),
    (
        "icons/circle-x.svg",
        include_bytes!("../assets/icons/circle-x.svg"),
    ),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    (
        "icons/alert-triangle.svg",
        include_bytes!("../assets/icons/alert-triangle.svg"),
    ),
    (
        "icons/trash-2.svg",
        include_bytes!("../assets/icons/trash-2.svg"),
    ),
    ("icons/edit.svg", include_bytes!("../assets/icons/edit.svg")),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    (
        "icons/clipboard.svg",
        include_bytes!("../assets/icons/clipboard.svg"),
    ),
    ("icons/eye.svg", include_bytes!("../assets/icons/eye.svg")),
    (
        "icons/eye-off.svg",
        include_bytes!("../assets/icons/eye-off.svg"),
    ),
    ("logo-icon.svg", include_bytes!("../assets/logo-icon.svg")),
];

/// Best-effort cleanup: clear system proxy and restore TUN routes so the OS
/// doesn't keep routing traffic to a dead local proxy after the app exits.
fn cleanup_on_exit() {
    // 1. Clear system proxy (retry once on failure)
    for attempt in 0..2 {
        match vex_core::clear_system_proxy() {
            Ok(()) => break,
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("not supported") {
                    break;
                }
                if attempt == 0 {
                    // Brief pause before retry
                    std::thread::sleep(std::time::Duration::from_millis(100));
                } else {
                    eprintln!("cleanup: failed to clear system proxy: {}", e);
                }
            }
        }
    }

    // 2. Restore TUN routes if they were active
    vex_core::emergency_restore_routes();
}

fn main() {
    // Set up tracing with both stdout and in-memory capture
    let log_buf = log_buffer::new_log_buffer();
    let capture_layer = log_buffer::LogCaptureLayer::new(log_buf.clone());

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(capture_layer)
        .init();

    // Register panic hook to clear system proxy even on panic
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        cleanup_on_exit();
        // Write crash log to %APPDATA%\vex\crash.log for diagnosis
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let log_path = std::path::PathBuf::from(appdata).join("vex").join("crash.log");
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let msg = format!("{}\n", info);
            let _ = std::fs::write(&log_path, &msg);
        }
        default_panic(info);
    }));

    // Create tokio runtime for async proxy/network operations
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("Failed to create tokio runtime");
    let tokio_handle = rt.handle().clone();

    // Register Ctrl+C handler to clean up system proxy
    let ctrlc_handle = tokio_handle.clone();
    std::thread::spawn(move || {
        ctrlc_handle.block_on(async {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                cleanup_on_exit();
                std::process::exit(0);
            }
        });
    });

    // On Windows, register SetConsoleCtrlHandler for CTRL_CLOSE_EVENT,
    // CTRL_LOGOFF_EVENT, and CTRL_SHUTDOWN_EVENT. These are NOT caught
    // by Ctrl+C handlers and would otherwise skip cleanup.
    #[cfg(target_os = "windows")]
    {
        unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
            // CTRL_CLOSE_EVENT=2, CTRL_LOGOFF_EVENT=5, CTRL_SHUTDOWN_EVENT=6
            if ctrl_type == 2 || ctrl_type == 5 || ctrl_type == 6 {
                // Run cleanup inline — we have ~5s before Windows kills us
                cleanup_on_exit();
                return 1; // handled
            }
            0 // not handled
        }

        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: unsafe extern "system" fn(u32) -> i32,
                add: i32,
            ) -> i32;
        }

        unsafe { SetConsoleCtrlHandler(console_ctrl_handler, 1) };
    }

    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        // Force dark mode to match our fixed dark color scheme
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        // Register keyboard shortcuts
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-c", app::ToggleProxy, None),
            KeyBinding::new("ctrl-1", app::SwitchToHome, None),
            KeyBinding::new("ctrl-2", app::SwitchToNodes, None),
            KeyBinding::new("ctrl-3", app::SwitchToConfig, None),
            KeyBinding::new("ctrl-4", app::SwitchToRules, None),
            KeyBinding::new("ctrl-5", app::SwitchToLogs, None),
            KeyBinding::new("ctrl-6", app::SwitchToSettings, None),
            KeyBinding::new("ctrl-t", app::TestAllLatency, None),
            KeyBinding::new("ctrl-m", app::CycleProxyMode, None),
        ]);

        let handle = tokio_handle.clone();
        let buf = log_buf.clone();
        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.0), px(100.0)),
                    size(px(960.0), px(640.0)),
                ))),
                window_min_size: Some(size(px(680.0), px(480.0))),
                titlebar: Some(app::AppState::titlebar_options()),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| app::AppState::new(window, cx, handle, buf));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });

    // App exited — always clean up system proxy
    cleanup_on_exit();
}
