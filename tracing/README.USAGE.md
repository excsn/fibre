# Usage Guide: fibre_tracing

How to collect the spans of one unit of work, read them back while it is running or after it has finished, keep the recent ones and hand them to an exporter.

## Table of Contents

* [Core Concepts](#core-concepts)
* [Quick Start](#quick-start)
* [Marking a Root](#marking-a-root)
* [Reading the Trace in Flight](#reading-the-trace-in-flight)
* [Reading Finished Traces](#reading-finished-traces)
* [Sizing the Ring](#sizing-the-ring)
* [Recording an Outcome](#recording-an-outcome)
* [Composing Beside a Log Sink](#composing-beside-a-log-sink)
* [Exporting, and Tail Sampling](#exporting-and-tail-sampling)
* [Printing a Waterfall](#printing-a-waterfall)
* [Why a Collector Rather Than a Subscriber](#why-a-collector-rather-than-a-subscriber)
* [Error Handling](#error-handling)

## Core Concepts

* **Root**: a span carrying `fibre.root = true`. It opens a trace; everything under it joins.
* **Trace**: one unit of work, being the root and every span opened inside it, in the order they opened.
* **Span**: one step, with its name, depth, when it started relative to the trace, how long it was open and its fields.
* **Unit of work**: whatever a root means to the caller. A request, a job, a compilation. This crate does not know.
* **Ring**: the finished traces kept in memory, newest last, oldest dropped.
* **Listener**: a function called with each trace as its root closes, before the ring sees it.
* **Outcome**: the `fibre.outcome` field, which is what a dashboard groups by rather than a raw status.
* **In flight**: a trace whose root has not closed. Readable, with the spans that have opened so far.
* **Layer**: what this crate is. It never sets a global subscriber, so the caller composes it.
* **Depth**: how far a span nests, so a waterfall prints without walking parents.
* **Tail sampling**: deciding what to export after seeing the whole trace, which nothing sampling at span start can do.

## Quick Start

```rust
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn main() {
  let (layer, traces) = fibre_tracing::layer();
  tracing_subscriber::registry().with(layer).init();

  work();

  for trace in traces.recent(10) {
    println!("trace {} took {:?}", trace.id, trace.duration);
  }
}

#[tracing::instrument(fields(fibre.root = true))]
fn work() {
  tracing::info_span!("step", n = 1).in_scope(|| {});
}
```

## Marking a Root

Either spelling works. The attribute is for a whole function, the macro for a scope inside one.

```rust
#[tracing::instrument(fields(fibre.root = true))]
async fn handle(path: &str) -> Response { ... }
```

```rust
let root = tracing::info_span!("request", fibre.root = true, path = "/cart");
let answered = handle().instrument(root).await;
```

A span opened outside any root is collected by nothing, which is what keeps a library's own spans out of your traces.

## Reading the Trace in Flight

`current` answers with the spans that have opened so far, which is what an error page shows.

```rust
if let Some(trace) = traces.current() {
  for span in &trace.spans {
    eprintln!("{}{} {:?}", "  ".repeat(span.depth), span.name, span.duration);
  }
}
```

It is `None` outside a root. Holding a span id instead of being inside it, use `of_span`:

```rust
let trace = traces.of_span(id.into_u64());
```

## Reading Finished Traces

```rust
let last = traces.recent(1).pop();       // the most recent
let page = traces.recent(50);            // newest last
traces.clear();                          // what a test calls between cases
```

## Sizing the Ring

The default keeps 256. A process that only exports wants none, which costs nothing and still calls listeners.

```rust
let (layer, traces) = fibre_tracing::layer();           // 256
let (layer, traces) = fibre_tracing::with_ring(2_000);  // more
let (layer, traces) = fibre_tracing::with_ring(0);      // keep nothing
```

## Recording an Outcome

Declare the field empty when the span opens and record it when the answer is known.

```rust
let span = tracing::info_span!("call", service = "fleet", fibre.outcome = tracing::field::Empty);
let answered = call().instrument(span.clone()).await;
span.record("fibre.outcome", if answered.is_ok() { "ok" } else { "failed" });
```

```rust
let failed: Vec<_> = trace.spans.iter().filter(|s| s.outcome() == Some("failed")).collect();
```

## Composing Beside a Log Sink

This crate sets nothing global, so a log sink and a collector sit on one registry.

```rust
let (logging, guard) = fibre_logging::init::layer_from_file(&config)?;
let (traces_layer, traces) = fibre_tracing::layer();
let composed = tracing_subscriber::registry().with(logging).with(traces_layer);
tracing::subscriber::set_global_default(composed)?;
```

Use `set_global_default` rather than `init` when the other layer installs the `log` bridge itself, since `init` tries to take it a second time and fails.

## Exporting, and Tail Sampling

Two shapes. As siblings, each layer sees every span and nothing forwards:

```rust
tracing_subscriber::registry().with(fibre_tracing_layer).with(otel_layer)
```

Through the listener, where the trace is finished and the decision can depend on it:

```rust
traces.on_finish(move |trace| {
  let slow = trace.duration > Duration::from_millis(500);
  let failed = trace.spans.iter().any(|s| s.outcome().is_some_and(|o| o != "ok"));
  if slow || failed {
    exporter.send(trace);
  }
});
```

Pick the sibling when you export everything. Pick the listener when you want to keep only the interesting traces, which nothing sampling at span start can decide.

## Printing a Waterfall

`depth` and `at` are on every span, so this needs no tree walk.

```rust
for span in &trace.spans {
  println!(
    "{:>7.2}ms {}{} {:.2}ms {}",
    span.at.as_secs_f64() * 1000.0,
    "  ".repeat(span.depth),
    span.name,
    span.duration.as_secs_f64() * 1000.0,
    span.outcome().unwrap_or(""),
  );
}
```

## Why a Collector Rather Than a Subscriber

`tracing` is fire and forget: a span reaches the subscriber and nothing comes back. That is right for logs, whose destination is a file or a socket, and wrong for three readers that want the work whole. An error page needs what this request did before it failed. A test needs what a body actually called, against the same shape production records, so the two cannot drift. A `Server-Timing` header needs per-step cost with no collector running anywhere.

None of those are export. OpenTelemetry has an in-memory exporter, but it is a test harness rather than a per-unit accessor, and it holds no ring. So the collector is its own thing and the exporter sits beside it.

## Error Handling

This crate returns no errors. A layer cannot fail: a span outside a root is not collected, an unknown field is kept as it was recorded, and a full ring drops its oldest trace.

Two things fail in the calling code instead.

```rust
// Already set: something else owns the dispatcher and nothing is collected.
if let Err(e) = tracing::subscriber::set_global_default(composed) {
  eprintln!("no collector: {e}");
}
```

```rust
// Outside a root, which is not a failure: there is no trace to read.
match traces.current() {
  Some(trace) => show(&trace),
  None => show_nothing(),
}
```
