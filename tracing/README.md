# Fibre Telemetry: Tracing

[![Crates.io](https://img.shields.io/crates/v/fibre_tracing.svg)](https://crates.io/crates/fibre_tracing)
[![Docs.rs](https://docs.rs/fibre_tracing/badge.svg)](https://docs.rs/fibre_tracing)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

Span collection for the `tracing` ecosystem: one unit of work becomes a trace your application can read back while it runs, keep in memory and hand to an exporter.

`fibre_tracing` solves the problem that `tracing` is write-only. A span goes to the subscriber and nothing comes back, which is right for logs, whose destination is a file or a socket, and wrong for the readers that want a unit of work whole: an error page showing what a request did before it failed, a test asserting what a body actually called, and a timing header reporting per-step cost with no collector running anywhere. This library keeps the spans so those readers can exist.

## Key Features

### One Unit of Work, Read Back Whole
A span carrying `fibre.root = true` opens a trace and everything under it joins. Ask for it from inside while it is still running, or from the ring once it has finished. The library itself knows nothing about requests: a web server calls its root a request, a queue worker calls it a job, a build calls it a compilation.

### A Ring of Recent Traces
Finished traces are kept in memory, newest last, oldest dropped, sized to whatever the application wants. That is enough to serve a "last fifty" view or a development error page with no collector, no exporter and no infrastructure of any kind.

### Composes Rather Than Captures
Nothing global is set for you. The layer goes onto a registry beside whatever else is listening, so a log sink such as [`fibre_logging`](https://crates.io/crates/fibre_logging) keeps taking events out to its appenders while this one keeps the spans. Its level hint never clamps what the collector sees.

### Tail Sampling
A listener is called with each trace as its root closes, holding the finished work. That is the only place a decision such as "export the ones that failed or ran long" can be made, since anything sampling at span start has not seen the trace yet.

### Beside OpenTelemetry, Not Instead
`tracing-opentelemetry` exports and this collects. Compose them as siblings and each sees every span, or route the exporter through the listener when you want the finished trace to decide what leaves the process.

### A Waterfall Without a Tree Walk
Every span carries its depth, when it opened relative to the trace and how long it was open, so printing a nested timing view is a loop rather than a traversal.

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
fibre_tracing = "0.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["registry"] }
```

## Getting Started

1.  Build the layer and install it on a registry at the start of `main`.
2.  Mark the top of each unit of work as a root.

```rust
// in main.rs
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn main() {
  let (layer, traces) = fibre_tracing::layer();
  tracing_subscriber::registry().with(layer).init();

  work();

  // Everything the last unit of work did, in the order it did it.
  if let Some(trace) = traces.recent(1).pop() {
    for span in &trace.spans {
      println!("{}{} {:?}", "  ".repeat(span.depth), span.name, span.duration);
    }
  }
}

#[tracing::instrument(fields(fibre.root = true))]
fn work() {
  tracing::info_span!("step", n = 1).in_scope(|| {});
}
```

For composing beside a log sink, recording outcomes, sizing the ring and wiring an exporter, please see the **[Usage Guide (README.USAGE.md)](README.USAGE.md)**.

The full API surface is in **[API_REFERENCE.md](API_REFERENCE.md)** and at **[docs.rs/fibre_tracing](https://docs.rs/fibre_tracing/)**.

## License

This library is distributed under the terms of the **Mozilla Public License Version 2.0 (MPL-2.0)**.
You can find a copy of the license in the [LICENSE](./LICENSE) file or at [https://opensource.org/licenses/MPL-2.0](https://opensource.org/licenses/MPL-2.0).
