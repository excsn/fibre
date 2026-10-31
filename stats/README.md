# fibre_stats

[![Crates.io](https://img.shields.io/crates/v/fibre_stats.svg)](https://crates.io/crates/fibre_stats)
[![Docs.rs](https://docs.rs/fibre_stats/badge.svg)](https://docs.rs/fibre_stats)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

Metrics for applications and libraries. A metric is defined once with `metric!`, which fixes its name, kind, unit, description and tags and registers it automatically, so every metric a program records is listed without the program doing anything. Recordings are aggregated in process to one point per series per second and handed to a sink in batches. It runs natively and on wasm, including `wasm32-unknown-unknown` with or without `wasm-bindgen`. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

```toml
[dependencies]
fibre_stats = "0" # Replace with the latest version
```

| Feature | Adds |
|---|---|
| `high_res` | `#[high_res("1ms")]` keeps exact samples beside the per-second aggregate |
| `wasm-bindgen` | `SystemClock` and timers on `wasm*-unknown-unknown`, read from the browser through `web-time` |
| `collect-process` | `collect::process(pid)`, the `process.*` metrics through `sysinfo`. Not on wasm |
| `collect-host` | `collect::host()`, the `host.*` metrics through `sysinfo`. Not on wasm |
| `collect-tokio` | `collect::tokio(handle)`, the `tokio.*` metrics from stable `Handle::metrics`. Not on wasm |
| `xs_foundation` | `XsClock`, which reads time from an `xs_foundation` clock |

## What to reach for

| Problem | Piece |
|---|---|
| Define a metric and keep its facts beside the code that records it | `metric!` |
| Count things | a `Counter` and `incr` or `add` |
| Report a running total something else keeps | `Counter::total` |
| Report a level | a `Gauge` and `set` |
| Time a block, a scope or a duration you already have | a `Timer` and `time`, `guard` or `record` |
| Report which of a few states something is in | `state_enum!` and a `State` |
| Record a metric whose name is only known at runtime | `RecorderExt` on any recorder |
| Aggregate and flush | `Stats`, `StatsConfig` and `Sink` |
| Record from many places without passing a recorder around | `set_global` |
| Add the same tags to everything from one component | `Context` or `Stats::context` |
| Read a value on a schedule | `Collector` and `Stats::add_collector` |
| Run in a browser or any host without threads | `set_sink`, `flush` and `collect_due` from the host's timer |
| Process, host or tokio runtime statistics | the `collect-*` features |
| Read percentiles from a window | `Histogram::quantile` |
| List every metric the program can emit | `registry()` |
