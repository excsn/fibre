// src/encoders/pattern.rs
use super::{util, EventFormatter};
use crate::error::Result;
use crate::model::TelemetryEvent;
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
enum Segment {
  Literal(String),
  Specifier(PatternSpecifier),
}

/// The internal representation of a conversion specifier like `%-5p`.
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
}

impl EventFormatter for PatternFormatter {
  fn format_event(&self, event: &TelemetryEvent) -> Result<Vec<u8>> {
    let mut output = String::with_capacity(256);

    for segment in &self.segments {
      match segment {
        Segment::Literal(text) => {
          output.push_str(text);
        }
        Segment::Specifier(spec) => match spec.converter {
          'd' => util::write_timestamp(&mut output, &event.timestamp),
          'p' | 'l' => {
            let _ = write!(output, "{}", event.level);
          }
          't' => output.push_str(&event.target),
          'm' => {
            if let Some(msg) = &event.message {
              output.push_str(msg);
            }
          }
          'n' => output.push('\n'),
          _ => {}
        },
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
  use crate::model::TelemetryEvent;
  use chrono::Utc;
  use tracing::Level;

  fn create_test_event() -> TelemetryEvent {
    TelemetryEvent::new(
      Level::INFO,
      "test_target",
      "test_event",
      Some("This is a test message.".to_string()),
    )
  }

  #[test]
  fn format_with_standard_pattern() {
    let formatter = PatternFormatter::new("[%d] %p %t - %m%n");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    assert!(formatted_str.contains("] INFO test_target - This is a test message.\n"));
  }

  #[test]
  fn parse_handles_literals_and_specifiers() {
    let formatter = PatternFormatter::new("LITERAL %p AND %m");
    assert_eq!(formatter.segments.len(), 4);
    assert!(matches!(&formatter.segments[0], Segment::Literal(s) if s == "LITERAL "));
    assert!(matches!(&formatter.segments[1], Segment::Specifier(spec) if spec.converter == 'p'));
    assert!(matches!(&formatter.segments[2], Segment::Literal(s) if s == " AND "));
    assert!(matches!(&formatter.segments[3], Segment::Specifier(spec) if spec.converter == 'm'));
  }

  #[test]
  fn format_handles_escaped_percent() {
    let formatter = PatternFormatter::new("%%LITERAL %p");
    let event = create_test_event();
    let formatted_str = String::from_utf8(formatter.format_event(&event).unwrap()).unwrap();
    // The `.trim()` handles the trailing newline we add by default.
    assert_eq!(formatted_str.trim(), "%LITERAL INFO");
  }
}
