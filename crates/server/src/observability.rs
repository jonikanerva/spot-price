//! Structured logging with a field allowlist (STACK.md §8).
//!
//! Each event becomes one line on standard output: JSON in production, or
//! readable text in local development. Only allowlisted field values are
//! written. Other field values are dropped, and only their names appear in
//! `dropped_fields`, so a new log call cannot leak a key, a URL, or a body.

use std::io::Write as _;

use jiff::Timestamp;
use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::config::LogFormat;

/// The fields whose values may appear in the log. A value of these fields
/// must never hold an API key, a password, a token, an e-mail address, an IP
/// address, a request body, or free text from a user.
pub const ALLOWED_FIELDS: [&str; 29] = [
    "message",
    "request_id",
    "method",
    "route",
    "status",
    "elapsed_ms",
    "code",
    "stage",
    "bind",
    "mode",
    "location",
    "job",
    "date",
    "stored",
    "skipped",
    "pruned",
    "vintage_stored",
    "vintage_pruned",
    "daily_stored",
    "daily_failed",
    "failed_points",
    "point",
    "reason",
    "attempt",
    "attempts",
    "areas",
    "tomorrow_available",
    "applied",
    "user_id",
];

/// Installs the global subscriber. Events of this service at INFO and above
/// and events of dependencies at WARN and above are written.
///
/// # Errors
///
/// Returns an error when a global subscriber already exists.
pub fn init(format: LogFormat) -> Result<(), tracing_subscriber::util::TryInitError> {
    let filter = Targets::new()
        .with_default(Level::WARN)
        .with_target("spot_price", Level::INFO)
        .with_target("spot_price_server", Level::INFO);
    tracing_subscriber::registry()
        .with(AllowlistLayer { format })
        .with(filter)
        .try_init()
}

/// Logs a panic with its source location only. The panic message can
/// contain data, so it is not logged.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or_else(
            || "unknown".to_owned(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        tracing::error!(location, "panic");
    }));
}

/// The layer that formats and filters the events.
#[derive(Debug, Clone, Copy)]
pub struct AllowlistLayer {
    pub format: LogFormat,
}

impl<S: Subscriber> Layer<S> for AllowlistLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        let line = match self.format {
            LogFormat::Json => json_line(event, fields),
            LogFormat::Text => text_line(event, &fields),
        };
        let mut stdout = std::io::stdout().lock();
        let written = stdout
            .write_all(line.as_bytes())
            .and_then(|()| stdout.flush());
        // A failed write to standard output has no better destination.
        written.unwrap_or(());
    }
}

/// The allowed fields of one event and the names of the dropped fields.
#[derive(Debug, Default)]
pub struct FieldCollector {
    pub allowed: Map<String, Value>,
    pub dropped: Vec<&'static str>,
}

impl FieldCollector {
    fn insert(&mut self, field: &Field, value: Value) {
        if ALLOWED_FIELDS.contains(&field.name()) {
            self.allowed.insert(field.name().to_owned(), value);
        } else {
            self.dropped.push(field.name());
        }
    }
}

impl Visit for FieldCollector {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::from(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Format only allowed fields, so a dropped value is never rendered.
        if ALLOWED_FIELDS.contains(&field.name()) {
            self.insert(field, Value::from(format!("{value:?}")));
        } else {
            self.dropped.push(field.name());
        }
    }
}

fn json_line(event: &Event<'_>, fields: FieldCollector) -> String {
    let metadata = event.metadata();
    let mut object = Map::new();
    object.insert(
        "timestamp".to_owned(),
        Value::from(Timestamp::now().to_string()),
    );
    object.insert("level".to_owned(), Value::from(metadata.level().as_str()));
    object.insert("target".to_owned(), Value::from(metadata.target()));
    object.extend(fields.allowed);
    if !fields.dropped.is_empty() {
        object.insert("dropped_fields".to_owned(), Value::from(fields.dropped));
    }
    let mut line = Value::Object(object).to_string();
    line.push('\n');
    line
}

fn text_line(event: &Event<'_>, fields: &FieldCollector) -> String {
    let metadata = event.metadata();
    let mut parts = vec![format!(
        "{} {:>5} {}",
        Timestamp::now(),
        metadata.level(),
        metadata.target()
    )];
    if let Some(Value::String(message)) = fields.allowed.get("message") {
        parts.push(format!("- {message}"));
    }
    parts.extend(
        fields
            .allowed
            .iter()
            .filter(|(name, _)| *name != "message")
            .map(|(name, value)| format!("{name}={value}")),
    );
    if !fields.dropped.is_empty() {
        parts.push(format!("dropped_fields={}", fields.dropped.join(",")));
    }
    let mut line = parts.join(" ");
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Collects the formatted lines instead of writing them.
    struct Capture(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut fields = FieldCollector::default();
            event.record(&mut fields);
            self.0.lock().unwrap().push(json_line(event, fields));
        }
    }

    #[test]
    fn writes_allowed_values_and_only_the_names_of_other_fields() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(lines.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let api_key = "secret-key";
            let url = "https://api.openweathermap.org/data/3.0/onecall?appid=secret-key";
            tracing::warn!(
                job = "weather",
                api_key,
                url,
                stored = 5_u64,
                "weather fetch degraded"
            );
        });
        let lines = lines.lock().unwrap();
        let line: Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(!lines[0].contains("secret-key"));
        assert_eq!(line["message"], "weather fetch degraded");
        assert_eq!(line["job"], "weather");
        assert_eq!(line["stored"], 5);
        assert_eq!(line["level"], "WARN");
        assert_eq!(
            line["dropped_fields"],
            serde_json::json!(["api_key", "url"])
        );
    }
}
