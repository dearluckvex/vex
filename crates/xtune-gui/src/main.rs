mod app;
mod components;
mod log_buffer;
mod views;

use gpui::*;
use gpui_component::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Best-effort cleanup: clear system proxy so the OS doesn't keep routing
/// traffic to a dead local proxy after the app exits.
fn cleanup_on_exit() {
    if let Err(e) = xtune_core::clear_system_proxy() {
        // Only log if it was a real error (not "unsupported platform")
        let msg = format!("{}", e);
        if !msg.contains("not supported") {
            eprintln!("cleanup: failed to clear system proxy: {}", e);
        }
    }
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
