// src/subscriber/visitor.rs
use crate::model::{LogValue, TelemetryEvent};
use std::collections::HashMap;
use tracing::field::{Field, Visit};

pub(crate) struct TelemetryEventFieldVisitor<'a> {
  fields: &'a mut HashMap<String, LogValue>,
  message: &'a mut Option<String>,
  // To handle duplicate "message" keys
  message_field_count: u32,
}

impl<'a> TelemetryEventFieldVisitor<'a> {
  pub(crate) fn new(event: &'a mut TelemetryEvent) -> Self {
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

impl<'a> Visit for TelemetryEventFieldVisitor<'a> {
  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
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
    self.record_potential_message(field.name(), value.to_string());
  }

  fn record_i64(&mut self, field: &Field, value: i64) {
    self.fields.insert(field.name().to_string(), LogValue::Int(value));
  }

  fn record_u64(&mut self, field: &Field, value: u64) {
    if value <= i64::MAX as u64 {
      // Fits in i64
      self.fields.insert(field.name().to_string(), LogValue::Int(value as i64));
    } else {
      // Does not fit, store as string
      self.fields.insert(field.name().to_string(), LogValue::String(value.to_string()));
    }
  }

  fn record_f64(&mut self, field: &Field, value: f64) {
    self.fields.insert(field.name().to_string(), LogValue::Float(value));
  }

  fn record_bool(&mut self, field: &Field, value: bool) {
    self.fields.insert(field.name().to_string(), LogValue::Bool(value));
  }
}