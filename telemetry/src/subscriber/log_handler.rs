// src/subscriber/log_handler.rs

use crate::{
  model::TelemetryEvent,
  subscriber::{processor::EventProcessor, visitor::TelemetryEventFieldVisitor},
};
use log::{Level, LevelFilter as LogLevelFilter, Metadata as LogMetadata, Record};
use std::sync::Arc;
use tracing_core::{
  callsite, field,
  metadata::{Kind, LevelFilter},
  subscriber::Interest,
  Metadata,
};

// A mock callsite for events originating from the `log` crate.
struct LogCallsite;
impl callsite::Callsite for LogCallsite {
  fn set_interest(&self, _interest: Interest) {}
  fn metadata(&self) -> &Metadata<'_> {
    // This is complex to implement fully. We create a dummy one, as the
    // important metadata is constructed dynamically in `log`.
    static DUMMY_METADATA: Metadata<'static> = Metadata::new(
      "log_event",
      "log",
      tracing_core::Level::TRACE,
      None,
      None,
      None,
      field::FieldSet::new(&[], callsite::Identifier(&LOG_CALLSITE)),
      Kind::EVENT,
    );
    &DUMMY_METADATA
  }
}
static LOG_CALLSITE: LogCallsite = LogCallsite;

/// A `log::Log` implementation that forwards records to the central `EventProcessor`.
pub(crate) struct LogHandler {
  processor: Arc<EventProcessor>,
}

impl LogHandler {
  pub(crate) fn new(processor: Arc<EventProcessor>) -> Self {
    Self { processor }
  }

  /// Converts a `log::Record` into our internal `TelemetryEvent` format.
  fn build_telemetry_event(&self, record: &Record<'_>) -> TelemetryEvent {
    let mut telemetry_event = TelemetryEvent::new(
      match record.level() {
        Level::Error => tracing_core::Level::ERROR,
        Level::Warn => tracing_core::Level::WARN,
        Level::Info => tracing_core::Level::INFO,
        Level::Debug => tracing_core::Level::DEBUG,
        Level::Trace => tracing_core::Level::TRACE,
      },
      record.target().to_string(),
      record.target().to_string(), // For logs, name and target are the same
      Some(format!("{}", record.args())),
    );

    // Add module path, file, and line as fields if available.
    if let Some(path) = record.module_path() {
      let _ = telemetry_event.fields.insert(
        "module_path".to_string(),
        crate::model::LogValue::String(path.to_string()),
      );
    }
    if let Some(file) = record.file() {
      let _ = telemetry_event.fields.insert(
        "file".to_string(),
        crate::model::LogValue::String(file.to_string()),
      );
    }
    if let Some(line) = record.line() {
      let _ = telemetry_event
        .fields
        .insert("line".to_string(), crate::model::LogValue::Int(line as i64));
    }

    // Since log records don't have tracing spans, we can't fill in span/parent IDs.
    // Thread info can be added similarly to the tracing layer if needed,
    // but we'll keep it simple for now.

    telemetry_event
  }
}

impl log::Log for LogHandler {
  fn enabled(&self, metadata: &LogMetadata) -> bool {
    // The `log` crate already performs a global check against log::max_level().
    // We can rely on that and simplify our handler to assume any record it
    // receives is already enabled.
    metadata.level() <= log::max_level()
  }

  fn log(&self, record: &Record) {
    // The redundant check `if !self.enabled(...)` is no longer necessary,
    // as the `log` crate has already done it. We can proceed directly.
    if record.level() > log::max_level() {
      return;
    }

    // Convert the log record to our internal format.
    let telemetry_event = self.build_telemetry_event(record);

    // Create the required `tracing::Metadata` on the fly.
    let level = telemetry_event.level;
    let metadata = Metadata::new(
      "log event", // Event name
      record.target(),
      level,
      record.file(),
      record.line(),
      record.module_path(),
      field::FieldSet::new(&["message"], callsite::Identifier(&LOG_CALLSITE)),
      Kind::EVENT,
    );

    // Pass both to the central processor.
    self.processor.process_event(telemetry_event, &metadata);
  }

  fn flush(&self) {
    // Flushing is handled by the appender guards.
  }
}
