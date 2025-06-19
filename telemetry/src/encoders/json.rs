// src/encoders/json.rs
use super::{util, EventFormatter};
use crate::error::{Error, Result};
use crate::model::{LogValue, TelemetryEvent};
use serde_json::Value; // Using serde_json::Value for flexibility
use std::collections::BTreeMap; // For consistent field order in JSON output from TelemetryEvent

pub struct JsonLinesFormatter {
  // Later: options like pretty_print, field overrides/filtering
}

impl JsonLinesFormatter {
  pub fn new() -> Self {
    Self {}
  }
}

impl EventFormatter for JsonLinesFormatter {
  fn format_event(&self, event: &TelemetryEvent) -> Result<Vec<u8>> {
    let mut json_map = BTreeMap::new(); // Use BTreeMap for consistent top-level key order

    let mut ts_buf = String::new();
    util::write_timestamp(&mut ts_buf, &event.timestamp);
    json_map.insert("timestamp".to_string(), Value::String(ts_buf));
    json_map.insert("level".to_string(), Value::String(event.level.to_string()));
    json_map.insert("target".to_string(), Value::String(event.target.clone()));

    if let Some(msg) = &event.message {
      json_map.insert("message".to_string(), Value::String(msg.clone()));
    }

    json_map.insert("name".to_string(), Value::String(event.name.clone()));
    if let Some(span_id) = &event.span_id {
      json_map.insert("span_id".to_string(), Value::String(span_id.clone()));
    }
    if let Some(parent_id) = &event.parent_id {
      json_map.insert("parent_id".to_string(), Value::String(parent_id.clone()));
    }
    if let Some(thread_id) = &event.thread_id {
      json_map.insert("thread_id".to_string(), Value::String(thread_id.clone()));
    }
    if let Some(thread_name) = &event.thread_name {
      json_map.insert(
        "thread_name".to_string(),
        Value::String(thread_name.clone()),
      );
    }

    if !event.fields.is_empty() {
      // event.fields is HashMap, but for consistent JSON output of this sub-object,
      // we can collect it into a BTreeMap before serializing to Value::Object.
      let custom_fields_map: BTreeMap<String, Value> = event
        .fields
        .iter()
        .map(|(key, log_value)| {
          let json_value = match log_value {
            LogValue::String(s) => Value::String(s.clone()),
            LogValue::Int(i) => Value::Number((*i).into()),
            LogValue::Float(f) => serde_json::Number::from_f64(*f)
              .map(Value::Number)
              .unwrap_or(Value::Null), // CORRECTED: Fallback to Value::Null for f64
            LogValue::Bool(b) => Value::Bool(*b),
            LogValue::Debug(d) => Value::String(d.clone()),
          };
          (key.clone(), json_value)
        })
        .collect();
      json_map.insert(
        "fields".to_string(),
        Value::Object(custom_fields_map.into_iter().collect()),
      );
    }

    let json_string = serde_json::to_string(&json_map)
      .map_err(|e| Error::Internal(format!("JSON serialization failed: {}", e)))?;

    Ok(format!("{}\n", json_string).into_bytes())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model::TelemetryEvent;
  use chrono::Utc;
  use std::collections::HashMap;
  use tracing::Level;

  #[test]
  fn format_basic_json_event() {
    let formatter = JsonLinesFormatter::new();
    let mut event = TelemetryEvent::new(
      Level::INFO,
      "my_target".to_string(),
      "event_name".to_string(),
      Some("Hello, JSON world!".to_string()),
    );
    event.timestamp = Utc::now(); // Ensure timestamp is set for consistent testing
    let mut fields = HashMap::new();
    fields.insert("key1".to_string(), LogValue::String("value1".to_string()));
    fields.insert("key2".to_string(), LogValue::Int(123));
    event.fields = fields;
    event.span_id = Some("span123".to_string());

    let formatted_bytes = formatter.format_event(&event).unwrap();
    let formatted_str = String::from_utf8(formatted_bytes).unwrap();

    println!("Formatted JSON: {}", formatted_str);

    // Deserialize back to check (basic check, not exhaustive field validation for MVP)
    let value: serde_json::Value = serde_json::from_str(formatted_str.trim_end()).unwrap();

    assert_eq!(value.get("level").unwrap().as_str().unwrap(), "INFO");
    assert_eq!(value.get("target").unwrap().as_str().unwrap(), "my_target");
    assert_eq!(
      value.get("message").unwrap().as_str().unwrap(),
      "Hello, JSON world!"
    );
    assert_eq!(value.get("name").unwrap().as_str().unwrap(), "event_name");
    assert_eq!(value.get("span_id").unwrap().as_str().unwrap(), "span123");

    let event_fields = value.get("fields").unwrap().as_object().unwrap();
    assert_eq!(
      event_fields.get("key1").unwrap().as_str().unwrap(),
      "value1"
    );
    assert_eq!(event_fields.get("key2").unwrap().as_i64().unwrap(), 123);
    assert!(formatted_str.ends_with('\n'));
  }

  #[test]
  fn format_json_event_no_message_no_fields() {
    let formatter = JsonLinesFormatter::new();
    let event = TelemetryEvent::new(
      Level::DEBUG,
      "another_target".to_string(),
      "simple_event".to_string(),
      None, // No message
    );
    // event.fields is empty by default

    let formatted_bytes = formatter.format_event(&event).unwrap();
    let formatted_str = String::from_utf8(formatted_bytes).unwrap();
    println!("Formatted JSON (no message/fields): {}", formatted_str);

    let value: serde_json::Value = serde_json::from_str(formatted_str.trim_end()).unwrap();
    assert_eq!(value.get("level").unwrap().as_str().unwrap(), "DEBUG");
    assert!(value.get("message").is_none());
    assert!(value.get("fields").is_none()); // "fields" key should not exist if event.fields is empty
    assert!(formatted_str.ends_with('\n'));
  }
}
