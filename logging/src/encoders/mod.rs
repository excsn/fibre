// Defines strategies for formatting LogEvents into byte streams.

use crate::config::processed::EncoderInternal;
use crate::error::Result; // Using crate::error::Result for formatting errors potentially
use crate::model::LogEvent; // To construct based on config

pub mod json;
pub mod pattern;
pub mod util;

/// Trait for types that can format a `LogEvent` into a byte vector.
/// This will be implemented by specific formatters like PatternFormatter or JsonLinesFormatter.
pub trait EventFormatter: Send + Sync + 'static {
  /// Formats the given `LogEvent` into a `Vec<u8>`.
  /// The output `Vec<u8>` should typically include a newline if it's line-based.
  fn format_event(&self, event: &LogEvent) -> Result<Vec<u8>>;
}

/// Creates an `EventFormatter` instance based on the processed encoder configuration.

pub(crate) fn new_event_formatter(config: &EncoderInternal) -> Box<dyn EventFormatter> {
  match config {
    EncoderInternal::Pattern(pattern_conf) => {
      Box::new(pattern::PatternFormatter::new(&pattern_conf.pattern_string))
    }
    EncoderInternal::JsonLines(json_conf) => {
      Box::new(json::JsonLinesFormatter::new(json_conf.clone()))
    }
  }
}
