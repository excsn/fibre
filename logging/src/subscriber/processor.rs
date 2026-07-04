use crate::{
  config::processed::OverflowPolicy,
  error_handling::{InternalErrorReport, InternalErrorSource},
  model::LogEvent,
  subscriber::actor::{ActorAction, AppenderActor},
};
use fibre::{
  error::TrySendError as FibreTrySendError, mpsc::BoundedSyncSender as FibreMpscBoundedSender,
};
use tracing_core::{metadata::LevelFilter, Metadata};

/// The central processor for all log events, from both `log` and `tracing`.
/// It owns the appenders and contains the core dispatch logic.
pub(crate) struct EventProcessor {
  actors: Vec<AppenderActor>,
  error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
  max_level: LevelFilter,
}

impl EventProcessor {
  pub(crate) fn new(
    actors: Vec<AppenderActor>,
    error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
  ) -> Self {
    let max_level = actors
      .iter()
      .map(|a| a.filter.max_level())
      .max()
      .unwrap_or(LevelFilter::OFF);
    Self {
      actors,
      error_tx,
      max_level,
    }
  }

  /// The most permissive level any appender can accept. Used as the global
  /// fast-path filter for both `tracing` and the `log` bridge.
  pub(crate) fn max_level(&self) -> LevelFilter {
    self.max_level
  }

  /// Fast pre-filter: is any appender interested in this metadata at all?
  pub(crate) fn event_enabled(&self, metadata: &Metadata<'_>) -> bool {
    self.actors.iter().any(|a| a.filter.enabled(metadata))
  }

  /// The single entry point for processing any log event.
  ///
  /// Filtering semantics: each appender evaluates the event against its own
  /// most specific logger rule (or its root-derived default). Additivity is
  /// decided by the most specific logger matching the event across ALL
  /// loggers: if that logger is non-additive, only appenders wired to that
  /// same logger receive the event.
  pub(crate) fn process_event(&self, event: LogEvent, metadata: &Metadata<'_>) {
    let event_level = *metadata.level();

    // One rule lookup per actor, reused for gating and level checks.
    let rules: Vec<Option<(&str, &(LevelFilter, bool))>> = self
      .actors
      .iter()
      .map(|actor| actor.filter.find_most_specific_rule(metadata))
      .collect();

    // Find the globally most specific matching logger.
    let mut winner: Option<(&str, bool)> = None;
    for (prefix, (_, additive)) in rules.iter().flatten() {
      if winner.map_or(true, |(wp, _)| prefix.len() > wp.len()) {
        winner = Some((*prefix, *additive));
      }
    }
    let non_additive_gate: Option<&str> = match winner {
      Some((prefix, false)) => Some(prefix),
      _ => None,
    };

    // Deliver to byte appenders immediately; collect event-stream appenders
    // so the LogEvent is cloned once per extra consumer, not once per send.
    let mut event_senders: Vec<(&AppenderActor, &FibreMpscBoundedSender<LogEvent>)> = Vec::new();

    for (actor, rule) in self.actors.iter().zip(&rules) {
      if let Some(gate_prefix) = non_additive_gate {
        let wired_to_gate = rule.map_or(false, |(prefix, _)| prefix == gate_prefix);
        if !wired_to_gate {
          continue;
        }
      }

      let enabled = match rule {
        Some((_, (level_filter, _))) => event_level <= *level_filter,
        None => event_level <= actor.filter.default_level,
      };
      if !enabled {
        continue;
      }

      match &actor.action {
        ActorAction::SendBytes(sender) => match actor.formatter.format_event(&event) {
          Ok(formatted_bytes) => self.send_bytes(actor, sender, formatted_bytes),
          Err(e) => {
            self.send_error_report(
              InternalErrorSource::EventFormatting {
                appender_name: actor.name.clone(),
              },
              e,
              Some(format!("Event target: {}", event.target)),
            );
          }
        },
        ActorAction::SendEvent(sender) => event_senders.push((actor, sender)),
      }
    }

    let last = event_senders.len().saturating_sub(1);
    let mut event = Some(event);
    for (i, (actor, sender)) in event_senders.into_iter().enumerate() {
      let to_send = if i == last {
        event.take().expect("event consumed before last sender")
      } else {
        event.as_ref().expect("event consumed early").clone()
      };
      self.send_event(actor, sender, to_send);
    }
  }

  fn send_bytes(
    &self,
    actor: &AppenderActor,
    sender: &FibreMpscBoundedSender<Vec<u8>>,
    bytes: Vec<u8>,
  ) {
    match actor.overflow {
      OverflowPolicy::Block => {
        // Guaranteed delivery: block the caller until there is room.
        // A send error means the appender task is gone (shutdown); drop then.
        let _ = sender.send(bytes);
      }
      OverflowPolicy::DropNewest => {
        if let Err(FibreTrySendError::Full(_)) = sender.try_send(bytes) {
          actor.drops.record(&actor.name);
        }
      }
    }
  }

  fn send_event(
    &self,
    actor: &AppenderActor,
    sender: &FibreMpscBoundedSender<LogEvent>,
    event: LogEvent,
  ) {
    match actor.overflow {
      OverflowPolicy::Block => {
        let _ = sender.send(event);
      }
      OverflowPolicy::DropNewest => {
        if let Err(FibreTrySendError::Full(_)) = sender.try_send(event) {
          actor.drops.record(&actor.name);
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
        eprintln!("[fibre_logging:ERROR] Internal error channel full. Dropping error report.");
      }
    } else {
      eprintln!("[fibre_logging:ERROR] {}: {}", source, error);
    }
  }
}
