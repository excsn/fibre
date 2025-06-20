// src/subscriber/dispatch.rs
// Defines the primary Layer that dispatches events to AppenderActors.

use crate::{
  model::LogEvent,
  subscriber::{processor::EventProcessor, visitor::LogEventFieldVisitor},
};
use std::sync::Arc;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
  layer::{Context, Layer},
  registry::LookupSpan,
};

/// The primary Layer for fibre_logging.
///
/// This layer is a thin adapter that receives events from the `tracing`
/// system, converts them to an internal `LogEvent`, and then passes
/// that event to the central `EventProcessor`.
pub(crate) struct DispatchLayer {
  processor: Arc<EventProcessor>,
}

impl DispatchLayer {
  /// Creates a new dispatch layer with a shared reference to the event processor.
  pub(crate) fn new(processor: Arc<EventProcessor>) -> Self {
    Self { processor }
  }

  /// Converts a `tracing::Event` into our internal `LogEvent` format.
  /// This is the single point of translation between the two models.
  fn build_log_event<S>(&self, event: &Event<'_>, ctx: Context<'_, S>) -> LogEvent
  where
    S: Subscriber + for<'span> LookupSpan<'span>,
  {
    let metadata = event.metadata();
    let mut log_event = LogEvent::new(
      *metadata.level(),
      metadata.target().to_string(),
      metadata.name().to_string(),
      None,
    );

    let mut visitor = LogEventFieldVisitor::new(&mut log_event);
    event.record(&mut visitor);

    if let Some(span_ref) = ctx.lookup_current() {
      log_event.span_id = Some(format!("{:?}", span_ref.id()));
      if let Some(parent_ref) = span_ref.parent() {
        log_event.parent_id = Some(format!("{:?}", parent_ref.id()));
      }
    }

    let current_thread = std::thread::current();
    // Extract the raw ID from the `Debug` output of `ThreadId`.
    let debug_id = format!("{:?}", current_thread.id());
    log_event.thread_id = Some(
      debug_id
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(&debug_id) // Fallback to the full debug string if parsing fails
        .to_string(),
    );
    if let Some(name) = current_thread.name() {
      log_event.thread_name = Some(name.to_string());
    }

    log_event
  }
}

impl<S> Layer<S> for DispatchLayer
where
  S: Subscriber + for<'span> LookupSpan<'span>,
{
  fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    // 1. Build the LogEvent.
    let log_event = self.build_log_event(event, ctx);
    // 2. Pass it to the central processor.
    self
      .processor
      .process_event(log_event, event.metadata());
  }

  // We keep this `enabled` check as a fast path to avoid building the
  // log event if no appender could possibly be interested.
  fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
    // The layer is enabled if ANY of its actors are enabled for this metadata.
    // This is a broad check; the fine-grained filtering happens in `on_event`.
    // self
    //   .actors
    //   .iter()
    //   .any(|actor| actor.filter.enabled(metadata))
    true // For now, let the processor handle all filtering.
  }
}
