use locald_core::ipc::{LogEntry, LogStream};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::Sender;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

#[derive(Debug)]
pub struct BroadcastLayer {
    pub sender: Sender<LogEntry>,
}

impl<S> Layer<S> for BroadcastLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let entry = LogEntry {
            service: "locald".to_string(),
            message: visitor.message,
            stream: LogStream::Stdout, // Treat all daemon logs as stdout for now
            timestamp,
        };

        // Ignore errors (no receivers)
        let _ = self.sender.send(entry);
    }
}

struct MessageVisitor {
    message: String,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}
