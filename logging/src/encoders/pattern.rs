// src/encoders/pattern.rs
use super::{util, EventFormatter};
use crate::error::Result;
use crate::model::LogEvent;
use once_cell::sync::Lazy;
use regex::Regex;
use std::fmt::Write;

// Regex now uses a main capture group for the specifier part, and an alternative for `%%`.
// This makes it easy to distinguish which one matched.
static PATTERN_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"(?P<specifier>%(?P<padding>-?\d+)?(?P<converter>[a-zA-Z])(?:\{(?P<options>[^}]+)\})?)|(?P<escaped>%%)")
    .expect("Pattern regex should be valid")
});

/// Represents a single piece of a parsed logging pattern.
#[derive(Debug)]
enum Segment {
  Literal(String),
  Specifier(PatternSpecifier),
}

/// The internal representation of a conversion specifier like `%-5p`.
#[derive(Debug)]
struct PatternSpecifier {
  converter: char,
  padding: Option<i32>,
  options: Option<String>,
}

pub struct PatternFormatter {
  segments: Vec<Segment>,
}

impl PatternFormatter {
  pub fn new(pattern_string: &str) -> Self {
    let segments = Self::parse(pattern_string);
    Self { segments }
  }

  /// Parses a pattern string into a sequence of `Segment`s using a robust method.
  fn parse(pattern: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut last_end = 0;

    for caps in PATTERN_REGEX.captures_iter(pattern) {
      // Get the full match position.
      let mat = caps.get(0).unwrap();

      // Add the literal text between the last match and this one.
      if mat.start() > last_end {
        segments.push(Segment::Literal(pattern[last_end..mat.start()].to_string()));
      }

      // Check WHICH part of the regex matched.
      if caps.name("specifier").is_some() {
        // It's a real specifier like %p or %d{...}
        let converter = caps
          .name("converter")
          .unwrap()
          .as_str()
          .chars()
          .next()
          .unwrap();
        let padding = caps.name("padding").and_then(|m| m.as_str().parse().ok());
        let options = caps.name("options").map(|m| m.as_str().to_string());
        segments.push(Segment::Specifier(PatternSpecifier {
          converter,
          padding,
          options,
        }));
      } else if caps.name("escaped").is_some() {
        // It's an escaped '%%'
        segments.push(Segment::Literal("%".to_string()));
      }

      last_end = mat.end();
    }

    // Add any remaining literal text after the last match.
    if last_end < pattern.len() {
      segments.push(Segment::Literal(pattern[last_end..].to_string()));
    }

    segments
  }

  /// Handles formatting for a single specifier, including options and padding.
  fn format_specifier(&self, buf: &mut String, spec: &PatternSpecifier, event: &LogEvent) {
    // Create a temporary string to hold the raw content before padding.
    // This is necessary because the padding logic needs the final content.
    let mut content = String::with_capacity(64);

    let needs_padding = spec.padding.is_some();

    // If we don't need padding, we can write directly to the final buffer.
    // Otherwise, write to the temporary `content` buffer.
    let target_buf: &mut String = if needs_padding { &mut content } else { buf };

    match spec.converter {
      // Date/Time
      'd' => {
        if let Some(format_str) = &spec.options {
          util::write_timestamp_with_format(target_buf, &event.timestamp, format_str);
        } else {
          util::write_timestamp(target_buf, &event.timestamp);
        }
      }
      // Level
      'p' | 'l' => {
        let _ = write!(target_buf, "{}", event.level);
      }
      // Target
      't' => target_buf.push_str(&event.target),
      // Message
      'm' => {
        if let Some(msg) = &event.message {
          target_buf.push_str(msg);
        }
      }
      // Thread Name
      'T' => {
        if let Some(name) = &event.thread_name {
          target_buf.push_str(name);
        }
      }
      // Newline - does not support padding and writes directly to the final buffer.
      'n' => {
        buf.push('\n');
        return; // Early return as padding is not applicable.
      }
      'X' => {
        if let Some(field_name) = &spec.options {
          // This handles the %X{field_name} case.
          if let Some(log_value) = event.fields.get(field_name) {
            let _ = write!(target_buf, "{}", log_value);
          }
        } else {
          // This handles the %X case (print all fields).
          if !event.fields.is_empty() {
            target_buf.push('{');

            // Sort keys for consistent output order.
            let mut sorted_keys: Vec<_> = event.fields.keys().collect();
            sorted_keys.sort();

            for (i, key) in sorted_keys.iter().enumerate() {
              if let Some(value) = event.fields.get(*key) {
                // We don't print the "message" field here as it's
                // already handled by the %m specifier.
                if key.as_str() != "message" {
                  let _ = write!(target_buf, "{}={}", key, value);
                  if i < sorted_keys.len() - 1 {
                    target_buf.push_str(", ");
                  }
                }
              }
            }
            target_buf.push('}');
          }
        }
      }
      // Unknown specifiers are ignored.
      _ => {}
    }

    // If padding was specified, apply it to the content we just generated.
    if let Some(padding) = spec.padding {
      self.apply_padding(buf, &content, padding);
    }
  }

  /// Applies left or right padding to the given content.
  fn apply_padding(&self, buf: &mut String, content: &str, padding: i32) {
    let width = padding.abs() as usize;
    if content.len() >= width {
      buf.push_str(content);
      return;
    }

    if padding > 0 {
      // Right-align (pad with spaces on the left)
      let _ = write!(buf, "{:>width$}", content, width = width);
    } else {
      // Left-align (pad with spaces on the right)
      let _ = write!(buf, "{:<width$}", content, width = width);
    }
  }
}

impl EventFormatter for PatternFormatter {
  fn format_event(&self, event: &LogEvent) -> Result<Vec<u8>> {
    let mut output = String::with_capacity(256);

    for segment in &self.segments {
      match segment {
        Segment::Literal(text) => {
          output.push_str(text);
        }
        Segment::Specifier(spec) => {
          self.format_specifier(&mut output, spec, event);
        }
      }
    }

    if !output.ends_with('\n') {
      output.push('\n');
    }

    Ok(output.into_bytes())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model::LogEvent;
  use chrono::{TimeZone, Utc};
  use tracing::Level;

  fn create_test_event() -> LogEvent {
    let mut event = LogEvent::new(
      Level::INFO,
      "test_target",
      "test_event",
      Some("This is a test message.".to_string()),
    );
    event.timestamp = Utc.with_ymd_and_hms(2023, 1, 1, 1, 1, 1).unwrap();
    event.thread_name = Some("main".to_string());
    event
  }

  #[test]
  fn format_with_standard_pattern() {
    let formatter = PatternFormatter::new("[%d] %p %t - %m%n");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    assert_eq!(
      formatted_str,
      "[2023-01-01T01:01:01.000Z] INFO test_target - This is a test message.\n"
    );
  }

  #[test]
  fn format_with_custom_date_format() {
    let formatter = PatternFormatter::new("[%d{%Y-%m-%d %H:%M}] %p - %m");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    assert_eq!(
      formatted_str.trim(),
      "[2023-01-01 01:01] INFO - This is a test message."
    );
  }

  #[test]
  fn format_with_padding() {
    let formatter = PatternFormatter::new("[%5p] %-8t [%-6T] %m");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    assert_eq!(
      formatted_str.trim(),
      "[ INFO] test_target [main  ] This is a test message."
    );
  }

  #[test]
  fn format_handles_escaped_percent() {
    let formatter = PatternFormatter::new("100%% %p");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    assert_eq!(formatted_str.trim(), "100% INFO");
  }

  #[test]
  fn parse_handles_literals_and_specifiers() {
    let formatter = PatternFormatter::new("LITERAL %-10p AND %m");
    assert_eq!(formatter.segments.len(), 4);
    assert!(matches!(&formatter.segments[0], Segment::Literal(s) if s == "LITERAL "));
    assert!(
      matches!(&formatter.segments[1], Segment::Specifier(spec) if spec.converter == 'p' && spec.padding == Some(-10))
    );
    assert!(matches!(&formatter.segments[2], Segment::Literal(s) if s == " AND "));
    assert!(
      matches!(&formatter.segments[3], Segment::Specifier(spec) if spec.converter == 'm' && spec.padding.is_none())
    );
  }
}
