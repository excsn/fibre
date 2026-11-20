//! `log` records reach whichever subscriber is current, and building a layer can be done any number of times in one process.

use std::path::Path;

use fibre_logging::LogEvent;
use tracing_subscriber::layer::SubscriberExt;

const CONFIG: &str = r#"
version: 1
appenders:
  stream:
    kind: custom
    buffer_size: 64
loggers:
  root:
    level: info
    appenders: [stream]
"#;

fn layer() -> (fibre_logging::subscriber::DispatchLayer, fibre_logging::InitResult, fibre_logging::CustomEventReceiver) {
  let dir = tempfile::tempdir().unwrap();
  let config: &Path = &dir.path().join("fibre_logging.yaml");
  std::fs::write(config, CONFIG).unwrap();
  let (logging, mut init) = fibre_logging::init::layer_from_file(config).unwrap();
  let events = init.custom_streams.remove("stream").unwrap();
  (logging, init, events)
}

fn drained(events: &fibre_logging::CustomEventReceiver) -> Vec<LogEvent> {
  let mut out = Vec::new();
  while let Ok(event) = events.try_recv() {
    out.push(event);
  }
  out
}

#[test]
fn a_layer_can_be_built_more_than_once_in_a_process() {
  let first = layer();
  let second = layer();
  drop((first, second));
}

#[test]
fn a_log_record_reaches_the_layer_of_the_current_subscriber() {
  let (_earlier, _earlier_init, earlier_events) = layer();
  let (logging, _init, events) = layer();
  tracing::subscriber::with_default(tracing_subscriber::registry().with(logging), || {
    log::info!(target: "bridge_check", "from the log crate");
    tracing::info!(target: "bridge_check", "from tracing");
  });
  let seen: Vec<String> = drained(&events).into_iter().filter(|event| event.target == "bridge_check").filter_map(|event| event.message).collect();
  assert_eq!(seen, vec!["from the log crate".to_string(), "from tracing".to_string()]);
  assert!(drained(&earlier_events).iter().all(|event| event.target != "bridge_check"), "a layer that is not part of the current subscriber hears nothing");
}
