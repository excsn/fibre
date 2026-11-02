//! A log line written inside a trace names it, so a line and its trace can be found from each other.

use std::path::Path;

use fibre_logging::{LogEvent, LogValue};
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

/// Runs `body` under a logging layer and, when asked, a tracing layer, in either order, and hands back what was logged with what was traced.
fn logged(traced: bool, tracing_first: bool, body: impl FnOnce()) -> (Vec<LogEvent>, Vec<fibre_tracing::Trace>) {
  let dir = tempfile::tempdir().unwrap();
  let config: &Path = &dir.path().join("fibre_logging.yaml");
  std::fs::write(config, CONFIG).unwrap();
  let (logging, mut init) = fibre_logging::init::layer_from_file(config).unwrap();
  let events = init.custom_streams.remove("stream").unwrap();
  let (traces_layer, traces) = fibre_tracing::layer();
  if !traced {
    tracing::subscriber::with_default(tracing_subscriber::registry().with(logging), body);
  } else if tracing_first {
    tracing::subscriber::with_default(tracing_subscriber::registry().with(traces_layer).with(logging), body);
  } else {
    tracing::subscriber::with_default(tracing_subscriber::registry().with(logging).with(traces_layer), body);
  }
  let mut out = Vec::new();
  while let Ok(event) = events.try_recv() {
    out.push(event);
  }
  (out, traces.recent(10))
}

fn trace_id_of(event: &LogEvent) -> Option<String> {
  match event.fields.get("trace_id") {
    Some(LogValue::String(text)) => Some(text.clone()),
    _ => None,
  }
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn root_and_child() {
  let root = tracing::info_span!("root", fibre.root = true);
  let _in_root = root.enter();
  tracing::info!("in the root");
  let child = tracing::info_span!("child");
  let _in_child = child.enter();
  tracing::info!("in a child");
}

#[test]
fn a_line_inside_a_trace_names_it_at_any_depth() {
  let (events, traces) = logged(true, true, root_and_child);
  assert_eq!(traces.len(), 1);
  let expected = hex(&traces[0].trace_id);
  assert_eq!(events.len(), 2);
  for event in &events {
    assert_eq!(trace_id_of(event).as_deref(), Some(expected.as_str()), "{:?}", event.message);
  }
}

#[test]
fn the_order_of_the_layers_does_not_matter() {
  let (events, traces) = logged(true, false, root_and_child);
  let expected = hex(&traces[0].trace_id);
  assert!(events.iter().all(|event| trace_id_of(event).as_deref() == Some(expected.as_str())), "{events:?}");
}

#[test]
fn a_trace_continued_from_another_process_keeps_that_id() {
  let (events, _) = logged(true, true, || {
    let root = tracing::info_span!("root", fibre.root = true, fibre.trace_id = "00112233445566778899aabbccddeeff");
    let _in_root = root.enter();
    tracing::info!("continued");
  });
  assert_eq!(trace_id_of(&events[0]).as_deref(), Some("00112233445566778899aabbccddeeff"));
}

#[test]
fn a_line_outside_any_trace_names_none() {
  let (events, _) = logged(true, true, || {
    tracing::info!("no span");
    let plain = tracing::info_span!("not a root");
    let _in_plain = plain.enter();
    tracing::info!("in a span with no root");
  });
  assert_eq!(events.len(), 2);
  assert!(events.iter().all(|event| trace_id_of(event).is_none()), "{events:?}");
}

#[test]
fn two_traces_keep_their_own_lines() {
  let (events, traces) = logged(true, true, || {
    for name in ["first", "second"] {
      let root = tracing::info_span!("root", fibre.root = true);
      let _in_root = root.enter();
      tracing::info!(name, "line");
    }
  });
  assert_eq!(traces.len(), 2);
  let ids: Vec<String> = events.iter().map(|event| trace_id_of(event).unwrap()).collect();
  assert_ne!(ids[0], ids[1]);
  assert_eq!(ids[0], hex(&traces[0].trace_id));
  assert_eq!(ids[1], hex(&traces[1].trace_id));
}

#[test]
fn a_trace_id_the_line_set_itself_stands() {
  let (events, _) = logged(true, true, || {
    let root = tracing::info_span!("root", fibre.root = true);
    let _in_root = root.enter();
    tracing::info!(trace_id = "its own", "line");
  });
  assert_eq!(trace_id_of(&events[0]).as_deref(), Some("its own"));
}

#[test]
fn with_no_tracing_layer_a_line_names_none() {
  let (events, _) = logged(false, true, root_and_child);
  assert_eq!(events.len(), 2);
  assert!(events.iter().all(|event| trace_id_of(event).is_none()), "{events:?}");
}
