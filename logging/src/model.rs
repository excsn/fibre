use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tracing::Level;

/// Represents a loggable value, part of a `LogEvent`'s fields.
#[derive(Debug, Clone, PartialEq)]
pub enum LogValue {
  String(String),
  Int(i64),
  Float(f64),
  Bool(bool),
  Debug(String),
}

/// The internal structured representation of a log event within `fibre_logging`.
#[derive(Debug, Clone)]
pub struct LogEvent {
  /// Timestamp of when the event was observed or generated.
  pub timestamp: DateTime<Utc>,
  /// The severity level of the event.
  pub level: Level,
  /// The target of the event (e.g., module path or a custom target string).
  pub target: String,
  /// The name of the span emitting the event, or the event's "name" if explicitly set.
  pub name: String,
  /// The primary message associated with the event, often from a "message" field.
  pub message: Option<String>,
  /// Key-value pairs of structured data associated with the event.
  pub fields: HashMap<String, LogValue>,
  /// ID of the current span, if any.
  pub span_id: Option<String>,
  /// ID of the parent span, if any.
  pub parent_id: Option<String>,
  /// Thread ID where the event originated.
  pub thread_id: Option<String>,
  /// Thread name where the event originated.
  pub thread_name: Option<String>,
}

impl LogEvent {
  /// Creates a new `LogEvent` with minimal required fields.
  /// Accepts any string-like type for target, name, and message.
  pub fn new<S1, S2>(
    level: Level,
    target: S1,
    name: S2,
    message: Option<String>,
  ) -> Self
  where
    S1: Into<String>,
    S2: Into<String>,
  {
    LogEvent {
      timestamp: Utc::now(),
      level,
      target: target.into(),
      name: name.into(),
      message,
      fields: HashMap::new(),
      span_id: None,
      parent_id: None,
      thread_id: None,
      thread_name: None,
    }
  }
}