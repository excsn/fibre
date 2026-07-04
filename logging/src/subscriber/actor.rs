// Defines the simple, non-tracing-aware components for an appender.

use crate::{config::processed::OverflowPolicy, encoders::EventFormatter, LogEvent};
use fibre::mpsc::BoundedSyncSender;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
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
      .filter(|(target_prefix, _)| target_matches_prefix(event_target, target_prefix))
      .max_by_key(|(target_prefix, _)| target_prefix.len())
      .map(|(k, v)| (k.as_str(), v))
  }

  /// The most permissive level this filter can ever accept, across all rules
  /// and the default. Used to derive global max-level hints.
  pub(crate) fn max_level(&self) -> LevelFilter {
    self
      .rules
      .values()
      .map(|(level, _)| *level)
      .max()
      .map_or(self.default_level, |m| m.max(self.default_level))
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

/// A logger name matches an event target if it is the target itself or a
/// module-path ancestor of it: `my_app` matches `my_app` and `my_app::db`,
/// but not `my_apple`.
fn target_matches_prefix(target: &str, prefix: &str) -> bool {
  target
    .strip_prefix(prefix)
    .map_or(false, |rest| rest.is_empty() || rest.starts_with("::"))
}

static PROCESS_START: Lazy<Instant> = Lazy::new(Instant::now);

/// Counts dropped messages per appender and reports them to stderr at most
/// once per second, instead of one line per dropped message.
#[derive(Default)]
pub(crate) struct DropCounter {
  dropped: AtomicU64,
  reported: AtomicU64,
  last_report_ms: AtomicU64,
}

impl DropCounter {
  const REPORT_INTERVAL_MS: u64 = 1000;

  pub(crate) fn record(&self, appender_name: &str) {
    let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
    let now_ms = PROCESS_START.elapsed().as_millis() as u64;
    let last = self.last_report_ms.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) >= Self::REPORT_INTERVAL_MS
      && self
        .last_report_ms
        .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
      let previously_reported = self.reported.swap(total, Ordering::Relaxed);
      eprintln!(
        "[fibre_logging:WARN] Appender '{}' dropped {} message(s) since last report ({} total). Channel is full.",
        appender_name,
        total - previously_reported,
        total
      );
    }
  }

  #[cfg(test)]
  pub(crate) fn dropped_count(&self) -> u64 {
    self.dropped.load(Ordering::Relaxed)
  }
}

// The distinction between a "console" or "file" appender now lives entirely
// in the background task that is spawned during initialization.
pub(crate) enum ActorAction {
  /// Send formatted bytes to a dedicated appender task.
  SendBytes(BoundedSyncSender<Vec<u8>>),
  /// Send a structured LogEvent to a custom application stream.
  SendEvent(BoundedSyncSender<LogEvent>),
}

/// A self-contained "actor" that represents a single configured appender.
/// It holds the name, filter, formatter, and the channel to its background task.
pub(crate) struct AppenderActor {
  pub(crate) name: String,
  pub(crate) filter: PerAppenderFilter,
  pub(crate) formatter: Box<dyn EventFormatter>,
  pub(crate) action: ActorAction,
  pub(crate) overflow: OverflowPolicy,
  pub(crate) drops: DropCounter,
}

#[cfg(test)]
mod tests {
  use super::*;
  use tracing_core::{callsite, field, subscriber, Kind, Level, Metadata};

  struct MockCallsite;
  impl callsite::Callsite for MockCallsite {
    fn set_interest(&self, _interest: subscriber::Interest) {}
    fn metadata(&self) -> &Metadata<'_> {
      unimplemented!()
    }
  }
  static MOCK_CALLSITE: MockCallsite = MockCallsite;

  fn mock_metadata(level: Level, target: &'static str) -> Metadata<'static> {
    Metadata::new(
      "test_event",
      target,
      level,
      None,
      None,
      None,
      field::FieldSet::new(&[], callsite::Identifier(&MOCK_CALLSITE)),
      Kind::EVENT,
    )
  }

  fn filter_with_rule(prefix: &str, level: LevelFilter) -> PerAppenderFilter {
    let mut rules = HashMap::new();
    rules.insert(prefix.to_string(), (level, true));
    PerAppenderFilter::new(rules, LevelFilter::OFF)
  }

  #[test]
  fn prefix_match_requires_module_boundary() {
    let filter = filter_with_rule("my_app", LevelFilter::INFO);

    assert!(filter.enabled(&mock_metadata(Level::INFO, "my_app")));
    assert!(filter.enabled(&mock_metadata(Level::INFO, "my_app::db")));
    assert!(filter.enabled(&mock_metadata(Level::INFO, "my_app::db::queries")));

    assert!(!filter.enabled(&mock_metadata(Level::INFO, "my_apple")));
    assert!(!filter.enabled(&mock_metadata(Level::INFO, "my_apple::mod")));
    assert!(!filter.enabled(&mock_metadata(Level::INFO, "my_ap")));
  }

  #[test]
  fn longest_prefix_still_wins_with_boundary_matching() {
    let mut rules = HashMap::new();
    rules.insert("my_app".to_string(), (LevelFilter::WARN, true));
    rules.insert("my_app::db".to_string(), (LevelFilter::DEBUG, true));
    let filter = PerAppenderFilter::new(rules, LevelFilter::OFF);

    let (prefix, _) = filter
      .find_most_specific_rule(&mock_metadata(Level::INFO, "my_app::db::queries"))
      .unwrap();
    assert_eq!(prefix, "my_app::db");

    let (prefix, _) = filter
      .find_most_specific_rule(&mock_metadata(Level::INFO, "my_app::api"))
      .unwrap();
    assert_eq!(prefix, "my_app");
  }

  #[test]
  fn max_level_is_most_permissive_of_rules_and_default() {
    let mut rules = HashMap::new();
    rules.insert("a".to_string(), (LevelFilter::WARN, true));
    rules.insert("b".to_string(), (LevelFilter::TRACE, true));
    let filter = PerAppenderFilter::new(rules, LevelFilter::INFO);
    assert_eq!(filter.max_level(), LevelFilter::TRACE);

    let filter = PerAppenderFilter::new(HashMap::new(), LevelFilter::ERROR);
    assert_eq!(filter.max_level(), LevelFilter::ERROR);
  }

  #[test]
  fn drop_counter_counts_accurately() {
    let counter = DropCounter::default();
    for _ in 0..100 {
      counter.record("test_appender");
    }
    assert_eq!(counter.dropped_count(), 100);
  }
}