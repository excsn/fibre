// src/subscriber/dispatch.rs
// Defines the primary Layer that dispatches events to AppenderActors.

use crate::{
  error_handling::{InternalErrorReport, InternalErrorSource},
  model::TelemetryEvent,
  subscriber::actor::AppenderActor,
  subscriber::visitor::TelemetryEventFieldVisitor,
};
use fibre::{
  error::TrySendError as FibreTrySendError, mpsc::BoundedSender as FibreMpscBoundedSender,
};
use std::io::Write;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
  layer::{Context, Layer},
  registry::LookupSpan,
};

/// The primary Layer for fibre_telemetry.
///
/// This layer is the key to supporting a dynamic number of appenders from the
/// configuration file. It receives all events from the `tracing` system,
/// converts them once to an internal `TelemetryEvent`, and then dispatches
/// that event to all configured `AppenderActor`s, each of which will apply
/// its own filtering, formatting, and writing logic.
pub(crate) struct DispatchLayer {
  actors: Vec<AppenderActor>,
  error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
}

impl DispatchLayer {
  /// Creates a new dispatch layer with its collection of appender actors.
  pub(crate) fn new(
    actors: Vec<AppenderActor>,
    error_tx: Option<FibreMpscBoundedSender<InternalErrorReport>>,
  ) -> Self {
    Self { actors, error_tx }
  }

  /// Converts a `tracing::Event` into our internal `TelemetryEvent` format.
  /// This is the single point of translation between the two models.
  fn build_telemetry_event<S>(&self, event: &Event<'_>, ctx: Context<'_, S>) -> TelemetryEvent
  where
    S: Subscriber + for<'span> LookupSpan<'span>,
  {
    let metadata = event.metadata();
    let mut telemetry_event = TelemetryEvent::new(
      *metadata.level(),
      metadata.target().to_string(),
      metadata.name().to_string(),
      None,
    );

    let mut visitor = TelemetryEventFieldVisitor::new(&mut telemetry_event);
    event.record(&mut visitor);

    if let Some(span_ref) = ctx.lookup_current() {
      telemetry_event.span_id = Some(format!("{:?}", span_ref.id()));
      if let Some(parent_ref) = span_ref.parent() {
        telemetry_event.parent_id = Some(format!("{:?}", parent_ref.id()));
      }
    }

    let current_thread = std::thread::current();
    telemetry_event.thread_id = Some(format!("{:?}", current_thread.id()));
    if let Some(name) = current_thread.name() {
      telemetry_event.thread_name = Some(name.to_string());
    }

    telemetry_event
  }

  /// Helper to send an internal error report. Falls back to eprintln if channel is disabled/full.
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

impl<S> Layer<S> for DispatchLayer
where
  S: Subscriber + for<'span> LookupSpan<'span>,
{
  fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    let telemetry_event = self.build_telemetry_event(event, ctx);

    for actor in &self.actors {
      if actor.filter.enabled(event.metadata()) {
        match actor.formatter.format_event(&telemetry_event) {
          Ok(formatted_bytes) => {
            let mut writer_guard = actor.writer.lock();
            if let Err(e) = writer_guard.write_all(&formatted_bytes) {
              self.send_error_report(
                InternalErrorSource::AppenderWrite {
                  appender_name: actor.name.clone(),
                },
                e,
                Some(format!("Event target: {}", telemetry_event.target)),
              );
            }
          }
          Err(e) => {
            self.send_error_report(
              InternalErrorSource::EventFormatting {
                appender_name: actor.name.clone(),
              },
              e,
              Some(format!("Event target: {}", telemetry_event.target)),
            );
          }
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::encoders::EventFormatter;
  use crate::error::Result;
  use crate::model::LogValue;
  use crate::subscriber::actor::PerAppenderFilter;
  use parking_lot::Mutex;
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex as StdMutex};
  use tracing::{info, Level};
  use tracing_subscriber::{prelude::*, registry::Registry};

  /// A mock formatter that captures the event it was asked to format.
  #[derive(Clone)]
  struct CapturingFormatter {
    captured_event: Arc<StdMutex<Option<TelemetryEvent>>>,
  }
  impl CapturingFormatter {
    fn new() -> Self {
      Self {
        captured_event: Arc::new(StdMutex::new(None)),
      }
    }
    fn take_captured(&self) -> Option<TelemetryEvent> {
      self.captured_event.lock().unwrap().take()
    }
  }
  impl EventFormatter for CapturingFormatter {
    fn format_event(&self, event: &TelemetryEvent) -> Result<Vec<u8>> {
      *self.captured_event.lock().unwrap() = Some(event.clone());
      Ok(Vec::new())
    }
  }

  #[test]
  fn dispatch_layer_correctly_builds_telemetry_event() {
    let formatter = CapturingFormatter::new();
    let filter = PerAppenderFilter::new(HashMap::new(), tracing_core::metadata::LevelFilter::TRACE);

    let actor = AppenderActor {
      name: "test_appender".to_string(),
      filter,
      formatter: Box::new(formatter.clone()),
      writer: Arc::new(Mutex::new(std::io::sink())), // A writer that does nothing
    };

    let dispatch_layer = DispatchLayer::new(vec![actor], None);
    let subscriber = Registry::default().with(dispatch_layer);

    // Use `with_default` to run tracing code with our test subscriber
    tracing::subscriber::with_default(subscriber, || {
      // This is the event we will test
      info!(
        message = "hello world",
        user_id = 42_i64,
        is_premium = true,
        rate = 0.5_f64,
        big_num = u64::MAX,
        extra_msg = "another message"
      );
    });

    // Now, inspect the event that our mock formatter captured.
    let captured = formatter.take_captured().expect("No event was captured");

    // Verify the primary message was extracted correctly
    assert_eq!(captured.message.as_deref(), Some("hello world"));

    // Verify all other fields were captured by the visitor
    assert_eq!(captured.fields.get("user_id"), Some(&LogValue::Int(42)));
    assert_eq!(
      captured.fields.get("is_premium"),
      Some(&LogValue::Bool(true))
    );
    assert_eq!(captured.fields.get("rate"), Some(&LogValue::Float(0.5)));
    assert_eq!(
      captured.fields.get("big_num"),
      Some(&LogValue::String(u64::MAX.to_string()))
    );

    // The "message" field itself should NOT be in the fields map, as it became the primary message.
    assert!(captured.fields.get("message").is_none());

    // Other string fields are present
    assert_eq!(
      captured.fields.get("extra_msg"),
      Some(&LogValue::String("another message".to_string()))
    );
  }
}
