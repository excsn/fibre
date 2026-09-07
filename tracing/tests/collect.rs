use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn under<T>(traces: &fibre_tracing::Traces, layer: fibre_tracing::TraceLayer, f: impl FnOnce() -> T) -> T {
  let _ = traces;
  let subscriber = tracing_subscriber::registry().with(layer);
  tracing::subscriber::with_default(subscriber, f)
}

#[test]
fn a_root_collects_everything_under_it_and_nothing_outside() {
  let (layer, traces) = fibre_tracing::layer();
  under(&traces, layer, || {
    tracing::info_span!("loose", n = 1).in_scope(|| {});

    let root = tracing::info_span!("request", fibre.root = true, path = "/cart");
    root.in_scope(|| {
      tracing::info_span!("source", id = "cart", owner = "lowered").in_scope(|| {
        tracing::info_span!("call", service = "shopping", method = "listProducts").in_scope(|| {});
      });
      tracing::info_span!("render", nodes = 42u64).in_scope(|| {});
    });
  });

  let held = traces.recent(10);
  assert_eq!(held.len(), 1, "one root, one trace; the loose span is in none");
  let trace = &held[0];
  let names: Vec<&str> = trace.spans.iter().map(|s| s.name).collect();
  assert_eq!(names, vec!["request", "source", "call", "render"], "in the order they opened");

  let depths: Vec<usize> = trace.spans.iter().map(|s| s.depth).collect();
  assert_eq!(depths, vec![0, 1, 2, 1], "a call nests under the source that made it");
  assert_eq!(trace.spans[2].parent, Some(1), "and names it by index");

  let call = trace.named("call").next().expect("the call span");
  assert_eq!(call.fields.get("service").map(|v| v.to_string()), Some("shopping".to_owned()));
  assert_eq!(trace.root().unwrap().fields.get("path").map(|v| v.to_string()), Some("/cart".to_owned()));
}

#[test]
fn the_trace_is_readable_from_inside_before_it_finishes() {
  let (layer, traces) = fibre_tracing::layer();
  let seen: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
  let inside = seen.clone();
  let reader = traces.clone();
  under(&traces, layer, || {
    tracing::info_span!("job", fibre.root = true).in_scope(|| {
      tracing::info_span!("step").in_scope(|| {
        let now = reader.current().expect("inside a root, the trace so far");
        *inside.lock() = now.spans.iter().map(|s| s.name).collect();
      });
    });
  });
  assert_eq!(*seen.lock(), vec!["job", "step"], "what an error page would show");
  assert!(traces.current().is_none(), "and nothing outside a root");
}

#[test]
fn a_finished_trace_reaches_a_listener_before_the_ring() {
  let (layer, traces) = fibre_tracing::layer();
  let caught: Arc<Mutex<Vec<(u64, usize)>>> = Arc::new(Mutex::new(Vec::new()));
  let into = caught.clone();
  traces.on_finish(move |trace| into.lock().push((trace.id, trace.spans.len())));

  under(&traces, layer, || {
    for i in 0..3u64 {
      tracing::info_span!("request", fibre.root = true, n = i).in_scope(|| {
        tracing::info_span!("source").in_scope(|| {});
      });
    }
  });

  let caught = caught.lock();
  assert_eq!(caught.len(), 3, "one call per finished trace, which is where an exporter sits");
  assert_eq!(caught.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![2, 2, 2]);
  assert_eq!(caught.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 2, 3], "ids run in order");
}

#[test]
fn the_ring_keeps_the_last_n_and_a_zero_ring_keeps_none() {
  let (layer, traces) = fibre_tracing::with_ring(2);
  under(&traces, layer, || {
    for i in 0..5u64 {
      tracing::info_span!("request", fibre.root = true, n = i).in_scope(|| {});
    }
  });
  let held = traces.recent(10);
  assert_eq!(held.len(), 2, "the ring is two deep");
  let ns: Vec<String> = held.iter().map(|t| t.root().unwrap().fields["n"].to_string()).collect();
  assert_eq!(ns, vec!["3", "4"], "and holds the newest");

  let (layer, none) = fibre_tracing::with_ring(0);
  let counted = Arc::new(Mutex::new(0usize));
  let into = counted.clone();
  none.on_finish(move |_| *into.lock() += 1);
  under(&none, layer, || {
    tracing::info_span!("request", fibre.root = true).in_scope(|| {});
  });
  assert!(none.is_empty(), "a zero ring keeps nothing");
  assert_eq!(*counted.lock(), 1, "and still offers the trace to a listener");
}

#[test]
fn a_span_records_how_long_it_was_open() {
  let (layer, traces) = fibre_tracing::layer();
  under(&traces, layer, || {
    tracing::info_span!("request", fibre.root = true).in_scope(|| {
      tracing::info_span!("slow").in_scope(|| std::thread::sleep(Duration::from_millis(12)));
    });
  });
  let trace = traces.recent(1).pop().expect("the trace");
  let slow = trace.named("slow").next().expect("the span");
  assert!(slow.duration >= Duration::from_millis(10), "it was open for the sleep: {:?}", slow.duration);
  assert!(trace.duration >= slow.duration, "and the root covers it: {:?}", trace.duration);
}

#[test]
fn a_field_recorded_after_the_span_opened_lands_on_it() {
  let (layer, traces) = fibre_tracing::layer();
  under(&traces, layer, || {
    tracing::info_span!("request", fibre.root = true).in_scope(|| {
      let span = tracing::info_span!("call", service = "shopping", fibre.outcome = tracing::field::Empty);
      span.in_scope(|| {});
      span.record("fibre.outcome", "not_found");
    });
  });
  let trace = traces.recent(1).pop().expect("the trace");
  let call = trace.named("call").next().expect("the span");
  assert_eq!(call.outcome(), Some("not_found"), "which is what a dashboard groups by");
}
