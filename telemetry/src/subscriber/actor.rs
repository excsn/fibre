// src/subscriber/actor.rs
// Defines the simple, non-tracing-aware components for an appender.

use crate::encoders::EventFormatter;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tracing_core::{metadata::LevelFilter, Metadata};

/// A struct holding the custom filtering rules for a single appender.
///
/// It stores a map of target prefixes to their required log level, and a
/// default level for events that don't match any specific target.
pub(crate) struct PerAppenderFilter {
  /// A map where the key is a target prefix (e.g., "my_app::api") and the
  /// value is the minimum level required for that target.
  rules: HashMap<String, LevelFilter>,
  /// The default minimum level for any event not matching a specific rule.
  /// This is often derived from the "root" logger's level for that appender.
  default_level: LevelFilter,
}

impl PerAppenderFilter {
  /// Creates a new filter with a given set of rules and a default level.
  /// This will be called from `init.rs` for each appender.
  pub(crate) fn new(rules: HashMap<String, LevelFilter>, default_level: LevelFilter) -> Self {
    Self {
      rules,
      default_level,
    }
  }

  /// Checks if an event, based on its metadata, should be processed by the
  /// appender associated with this filter.
  ///
  /// The logic finds the most specific matching rule for the event's target.
  pub(crate) fn enabled(&self, metadata: &Metadata<'_>) -> bool {
    let event_level = *metadata.level();
    let event_target = metadata.target();

    // Find the most specific matching rule for the event's target.
    // For example, a rule for "my_app::api" is more specific than "my_app".
    let most_specific_rule = self
      .rules
      .iter()
      .filter(|(target, _)| event_target.starts_with(*target))
      .max_by_key(|(target, _)| target.len());

    let effective_level = if let Some((_target, level)) = most_specific_rule {
      // A specific rule was found.
      *level
    } else {
      // No specific rule matched, use the default level for this appender.
      self.default_level
    };

    // The event is enabled if its level is at or below the effective level.
    // (e.g., INFO is below DEBUG). LevelFilter implements `PartialOrd`.
    event_level <= effective_level
  }
}

/// A self-contained "actor" that represents a single configured appender.
/// It holds the name, filter, formatter, and writer, and has no generic parameters.
pub(crate) struct AppenderActor {
  pub(crate) name: String,
  pub(crate) filter: PerAppenderFilter,
  pub(crate) formatter: Box<dyn EventFormatter>,
  pub(crate) writer: Arc<Mutex<dyn Write + Send + Sync + 'static>>,
}
