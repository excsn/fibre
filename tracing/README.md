# fibre_tracing

[![Crates.io](https://img.shields.io/crates/v/fibre_tracing.svg)](https://crates.io/crates/fibre_tracing)
[![Docs.rs](https://docs.rs/fibre_tracing/badge.svg)](https://docs.rs/fibre_tracing)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

A `tracing` layer that keeps the spans of one unit of work as a trace your application can read back while it runs, hold in a ring and hand to an exporter. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

```toml
[dependencies]
fibre_tracing = "0" # Replace with the latest version
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["registry"] }
```

The crate has no features.

## What to reach for

| Problem | Piece |
|---|---|
| Install the collector | `layer()` |
| Choose how many finished traces to keep | `with_ring(n)` |
| Mark the top of a unit of work | a span with `fibre.root = true` |
| Show what this request did before it failed | `Traces::current` |
| Read a trace from a span id you hold | `Traces::of_span` |
| A "last fifty" view or a test assertion | `Traces::recent` |
| Export only the slow or failed traces | `Traces::on_finish` |
| Group spans by result | the `fibre.outcome` field and `Span::outcome` |
| Print a nested timing view | `Span::depth` and `Span::at` |
| Run beside a log sink or OpenTelemetry | compose the layer on one registry |
