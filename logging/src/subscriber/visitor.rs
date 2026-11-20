use crate::model::{LogValue, LogEvent};

use std::collections::HashMap;
use tracing::field::{Field, Visit};

pub(crate) struct LogEventFieldVisitor<'a> {
  fields: &'a mut HashMap<String, LogValue>,
  message: &'a mut Option<String>,
  // To handle duplicate "message" keys
  message_field_count: u32,
}

impl<'a> LogEventFieldVisitor<'a> {
  pub(crate) fn new(event: &'a mut LogEvent) -> Self {
    Self {
      fields: &mut event.fields,
      message: &mut event.message,
      message_field_count: 0,
    }
  }

  // Helper to handle fields that could be the primary message
  fn record_potential_message(&mut self, field_name: &str, value_str: String) {
    if field_name == "message" {
      if self.message.is_none() {
        *self.message = Some(value_str);
      } else {
        // Message is already set, store this one in fields with a disambiguated key
        self.message_field_count += 1;
        let disambiguated_key = format!("message.{}", self.message_field_count);
        self.fields.insert(disambiguated_key, LogValue::String(value_str));
      }
    } else {
      // Not a "message" field, or "message" field but not being used as primary
      self.fields.insert(field_name.to_string(), LogValue::String(value_str));
    }
  }
}

/// Fields tracing-log adds to carry a `log` record's place, which the event already holds.
fn is_log_place(name: &str) -> bool {
  name.starts_with("log.")
}

impl<'a> Visit for LogEventFieldVisitor<'a> {
  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
    if is_log_place(field.name()) {
      return;
    }
    let value_str = format!("{:?}", value);
    // For Debug, always treat "message" field specially, but other debug fields go to LogValue::Debug
    if field.name() == "message" {
        if self.message.is_none() {
            *self.message = Some(value_str);
        } else {
            self.message_field_count += 1;
            let disambiguated_key = format!("message.{}", self.message_field_count);
            self.fields.insert(disambiguated_key, LogValue::Debug(value_str));
        }
    } else {
        self.fields.insert(field.name().to_string(), LogValue::Debug(value_str));
    }
  }

  fn record_str(&mut self, field: &Field, value: &str) {
    if is_log_place(field.name()) {
      return;
    }
    self.record_potential_message(field.name(), value.to_string());
  }

  fn record_i64(&mut self, field: &Field, value: i64) {
    self.fields.insert(field.name().to_string(), LogValue::Int(value));
  }

  fn record_u64(&mut self, field: &Field, value: u64) {
    if is_log_place(field.name()) {
      return;
    }
    let value = i64::try_from(value).map_or(LogValue::UInt(value), LogValue::Int);
    self.fields.insert(field.name().to_string(), value);
  }

  fn record_u128(&mut self, field: &Field, value: u128) {
    self.fields.insert(field.name().to_string(), LogValue::U128(value));
  }

  fn record_i128(&mut self, field: &Field, value: i128) {
    let value = i64::try_from(value).map_or_else(|_| LogValue::String(value.to_string()), LogValue::Int);
    self.fields.insert(field.name().to_string(), value);
  }

  fn record_bytes(&mut self, field: &Field, value: &[u8]) {
    self.fields.insert(field.name().to_string(), LogValue::Bytes(value.to_vec()));
  }

  fn record_f64(&mut self, field: &Field, value: f64) {
    self.fields.insert(field.name().to_string(), LogValue::Float(value));
  }

  fn record_bool(&mut self, field: &Field, value: bool) {
    self.fields.insert(field.name().to_string(), LogValue::Bool(value));
  }
}