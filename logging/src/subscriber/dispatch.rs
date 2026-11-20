// Defines the primary Layer that dispatches events to AppenderActors.

use tracing_log::NormalizeEvent;
use fibre_tracing::TraceIdentity;
use crate::model::LogValue;
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

/// The field a line written inside a trace names it under, the name the trace store filters on.
const TRACE_ID_FIELD: &str = "trace_id";

/// The primary Layer for fibre_logging.
///
/// This layer is a thin adapter that receives events from the `tracing`
/// system, converts them to an internal `LogEvent`, and then passes
/// that event to the central `EventProcessor`.
/// tracing-log's field naming a `log` record's target.
const LOG_TARGET_FIELD: &str = "log.target";

#[derive(Clone)]
pub struct DispatchLayer {
  processor: Arc<EventProcessor>,
}

impl DispatchLayer {
  /// Creates a new dispatch layer with a shared reference to the event processor.
  pub(crate) fn new(processor: Arc<EventProcessor>) -> Self {
    Self { processor }
  }

  /// Converts a `tracing::Event` into our internal `LogEvent` format.
  /// This is the single point of translation between the two models.
  fn build_log_event<S>(&self, event: &Event<'_>, metadata: &tracing::Metadata<'_>, ctx: Context<'_, S>) -> LogEvent
  where
    S: Subscriber + for<'span> LookupSpan<'span>,
  {
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

    if let Some(scope) = ctx.event_scope(event) {
      let identity = scope.into_iter().find_map(|span| span.extensions().get::<TraceIdentity>().map(|held| held.0.clone()));
      if let Some(identity) = identity {
        log_event.fields.entry(TRACE_ID_FIELD.to_string()).or_insert_with(|| LogValue::String(identity.to_string()));
      }
    }

    THREAD_INFO.with(|(id, name)| {
      log_event.thread_id = Some(id.clone());
      log_event.thread_name = name.clone();
    });

    log_event
  }
}

thread_local! {
  // Computed once per thread instead of Debug-formatting and re-parsing the
  // ThreadId on every event.
  static THREAD_INFO: (String, Option<String>) = {
    let current_thread = std::thread::current();
    // Extract the raw ID from the `Debug` output of `ThreadId`.
    let debug_id = format!("{:?}", current_thread.id());
    let id = debug_id
      .strip_prefix("ThreadId(")
      .and_then(|s| s.strip_suffix(')'))
      .unwrap_or(&debug_id) // Fallback to the full debug string if parsing fails
      .to_string();
    (id, current_thread.name().map(str::to_string))
  };
}

impl<S> Layer<S> for DispatchLayer
where
  S: Subscriber + for<'span> LookupSpan<'span>,
{
  fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    // A record from the `log` crate arrives through tracing-log's stand-in callsite; its own target and level are in the normalized metadata.
    let normalized = event.normalized_metadata();
    let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
    let log_event = self.build_log_event(event, metadata, ctx);
    self.processor.process_event(log_event, metadata);
  }

  // Fast path: avoid building the log event if no appender could possibly be
  // interested. The result is cached per callsite by the default
  // `register_callsite`, which is safe because filters are static after init.
  fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
    // Spans must stay enabled regardless of appender levels: events inside
    // them need the registry to know the span for span_id/parent_id capture.
    if !metadata.is_event() {
      return true;
    }
    // tracing-log's callsite stands for every `log` target, so only `on_event` can judge its records.
    if metadata.fields().field(LOG_TARGET_FIELD).is_some() {
      return true;
    }
    self.processor.event_enabled(metadata)
  }

  fn max_level_hint(&self) -> Option<tracing_core::metadata::LevelFilter> {
    Some(self.processor.max_level())
  }
}
