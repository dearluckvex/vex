use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing_subscriber::Layer;

/// Maximum number of log entries to keep in the ring buffer.
const MAX_LOG_ENTRIES: usize = 500;

/// A single captured log entry.
#[derive(Clone)]
pub struct LogEntry {
    pub level: Level,
    pub target: String,
    pub message: String,
}

impl fmt::Display for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lvl = match self.level {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN ",
            Level::INFO => "INFO ",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
        write!(f, "[{}] {}: {}", lvl, self.target, self.message)
    }
}

/// Shared log buffer accessible by the GUI.
pub type SharedLogBuffer = Arc<Mutex<VecDeque<LogEntry>>>;

/// Create a new shared log buffer.
pub fn new_log_buffer() -> SharedLogBuffer {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_ENTRIES)))
}

/// A tracing layer that captures events into a shared ring buffer.
pub struct LogCaptureLayer {
    buffer: SharedLogBuffer,
}

impl LogCaptureLayer {
    pub fn new(buffer: SharedLogBuffer) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for LogCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = *metadata.level();

        // Only capture INFO and above for the GUI log viewer
        if level > Level::DEBUG {
            return;
        }

        let target = metadata.target().to_string();

        // Extract message from the event fields
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let message = visitor.0;

        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= MAX_LOG_ENTRIES {
                buf.pop_front();
            }
            buf.push_back(LogEntry {
                level,
                target,
                message,
            });
        }
    }
}

/// Visitor that extracts the `message` field from tracing events.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        } else if self.0.is_empty() {
            self.0 = format!("{} = {:?}", field.name(), value);
        } else {
            self.0
                .push_str(&format!(", {} = {:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        } else if self.0.is_empty() {
            self.0 = format!("{} = {}", field.name(), value);
        } else {
            self.0.push_str(&format!(", {} = {}", field.name(), value));
        }
    }
}
