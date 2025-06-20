// Defines the simple, non-tracing-aware components for an appender.

use crate::{encoders::EventFormatter, LogEvent};
use fibre::mpsc::BoundedSender;
use std::collections::HashMap;
use tracing_core::{metadata::LevelFilter, Metadata};

/// A struct holding the custom filtering rules for a single appender.
///
/// It stores a map of target prefixes to their required log level, and a
/// default level for events that don't match any specific target.
pub(crate) struct PerAppenderFilter {
  /// A map where the key is a target prefix (e.g., "my_app::api") and the
  /// value is a tuple of the minimum level required and the additive flag.
  pub(crate) rules: HashMap<String, (LevelFilter, bool)>,
  /// The default minimum level for any event not matching a specific rule.
  /// This is often derived from the "root" logger's level for that appender.
  pub(crate) default_level: LevelFilter,
}

impl PerAppenderFilter {
  /// Creates a new filter with a given set of rules and a default level.
  pub(crate) fn new(rules: HashMap<String, (LevelFilter, bool)>, default_level: LevelFilter) -> Self {
    Self {
      rules,
      default_level,
    }
  }

  /// Finds the most specific matching rule for the given metadata.
  pub(crate) fn find_most_specific_rule(
    &self,
    metadata: &Metadata<'_>,
  ) -> Option<(&str, &(LevelFilter, bool))> {
    let event_target = metadata.target();
    self
      .rules
      .iter()
      .filter(|(target_prefix, _)| event_target.starts_with(*target_prefix))
      .max_by_key(|(target_prefix, _)| target_prefix.len())
      .map(|(k, v)| (k.as_str(), v))
  }

  /// Checks if an event, based on its metadata, should be processed by the
  /// appender associated with this filter.
  pub(crate) fn enabled(&self, metadata: &Metadata<'_>) -> bool {
    let event_level = *metadata.level();

    if let Some((_target, (level_filter, _))) = self.find_most_specific_rule(metadata) {
      // A specific rule was found. Use its level.
      return event_level <= *level_filter;
    }

    // No specific rule, use the appender's default level (from root).
    event_level <= self.default_level
  }
}

// REVISED: ActorAction is now simplified. It only holds a sender for raw bytes.
// The distinction between a "console" or "file" appender now lives entirely
// in the background task that is spawned during initialization.
pub(crate) enum ActorAction {
  /// Send formatted bytes to a dedicated appender task.
  SendBytes(BoundedSender<Vec<u8>>),
  /// Send a structured LogEvent to a custom application stream.
  SendEvent(BoundedSender<LogEvent>),
}

/// A self-contained "actor" that represents a single configured appender.
/// It holds the name, filter, formatter, and the channel to its background task.
pub(crate) struct AppenderActor {
  pub(crate) name: String,
  pub(crate) filter: PerAppenderFilter,
  pub(crate) formatter: Box<dyn EventFormatter>,
  pub(crate) action: ActorAction,
}