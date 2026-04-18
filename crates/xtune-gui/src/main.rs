mod app;
mod components;
mod log_buffer;
mod views;

use gpui::*;
use gpui_component::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Best-effort cleanup: clear system proxy and restore TUN routes so the OS
/// doesn't keep routing traffic to a dead local proxy after the app exits.
fn cleanup_on_exit() {
    // 1. Clear system proxy (retry once on failure)
    for attempt in 0..2 {
        match xtune_core::clear_system_proxy() {
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
    xtune_core::emergency_restore_routes();
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

        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: unsafe extern "system" fn(u32) -> i32,
                add: i32,
            ) -> i32;
        }

        unsafe { SetConsoleCtrlHandler(console_ctrl_handler, 1) };
    }

    let app = Application::new();

    app.run(move |cx| {
        gpui_component::init(cx);

        let handle = tokio_handle.clone();
        let buf = log_buf.clone();
        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.0), px(100.0)),
                    size(px(960.0), px(640.0)),
                ))),
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
