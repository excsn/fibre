// telemetry/src/subscriber/processor.rs

use crate::{
  error_handling::{InternalErrorReport, InternalErrorSource},
  model::TelemetryEvent,
  subscriber::actor::{ActorAction, AppenderActor},
};
use fibre::{
  error::TrySendError as FibreTrySendError, mpsc::BoundedSender as FibreMpscBoundedSender,
};
use tracing_core::Metadata;

/// The central processor for all telemetry events, from both `log` and `tracing`.
/// It owns the appenders and contains the core dispatch logic.
pub(crate) struct EventProcessor {
  actors: Vec<AppenderActor>,
  error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
}

impl EventProcessor {
  pub(crate) fn new(
    actors: Vec<AppenderActor>,
    error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
  ) -> Self {
    Self { actors, error_tx }
  }

  /// The single entry point for processing any telemetry event.
  ///
  /// This function contains the filtering and dispatching logic.
  pub(crate) fn process_event(&self, event: TelemetryEvent, metadata: &Metadata<'_>) {
    // 1. Determine if this event is matched by any non-additive logger.
    let mut non_additive_match = false;
    for actor in &self.actors {
      if let Some((_, (_, additive))) = actor.filter.find_most_specific_rule(metadata) {
        if !*additive {
          non_additive_match = true;
          break;
        }
      }
    }

    // 2. Loop through each configured actor.
    for actor in &self.actors {
      let rule = actor.filter.find_most_specific_rule(metadata);

      // 3. Decide if we should skip this appender due to non-additive logic.
      if non_additive_match {
        let is_appender_for_non_additive = rule.map_or(false, |(_, (_, additive))| !*additive);

        if !is_appender_for_non_additive {
          continue;
        }
      }

      // 4. Check if the event is enabled for this actor's specific filter.
      if actor.filter.enabled(metadata) {
        // 5. Perform the action defined for this actor (SendBytes or SendEvent).
        match &actor.action {
          ActorAction::SendBytes(sender) => {
            // This is a standard appender (console, file, etc.)
            // We must format the event into bytes first.
            match actor.formatter.format_event(&event) {
              Ok(formatted_bytes) => {
                if let Err(FibreTrySendError::Full(_)) = sender.try_send(formatted_bytes) {
                  // The channel to the appender actor is full. This indicates that
                  // the appender (e.g., file writer) is back-pressured.
                  // We drop the message to avoid blocking the application.
                  eprintln!(
                    "[fibre_telemetry:WARN] Channel for appender '{}' is full. Dropping message.",
                    actor.name
                  );
                }
              }
              Err(e) => {
                // This is an error during the formatting process itself.
                self.send_error_report(
                  InternalErrorSource::EventFormatting {
                    appender_name: actor.name.clone(),
                  },
                  e,
                  Some(format!("Event target: {}", event.target)),
                );
              }
            }
          }
          ActorAction::SendEvent(sender) => {
            // This is a "custom" appender that wants the raw TelemetryEvent.
            // No formatting is needed.
            if let Err(FibreTrySendError::Full(_)) = sender.try_send(event.clone()) {
              eprintln!(
                "[fibre_telemetry:WARN] Custom appender stream for '{}' is full. Dropping event.",
                actor.name
              );
            }
          }
        }
      }
    }
  }

  /// Helper to send an internal error report.
  fn send_error_report<E: std::error::Error + 'static>(
    &self,
    source: InternalErrorSource,
    error: E,
    context: Option<String>,
  ) {
    if let Some(tx) = &self.error_tx {
      let report = InternalErrorReport::new(source, error, context);
      if let Err(FibreTrySendError::Full(_report)) = tx.try_send(report) {
        eprintln!("[fibre_telemetry:ERROR] Internal error channel full. Dropping error report.");
      }
    } else {
      eprintln!("[fibre_telemetry:ERROR] {}: {}", source, error);
    }
  }
}
