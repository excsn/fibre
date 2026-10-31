# fibre

[![crates.io](https://img.shields.io/crates/v/fibre.svg)](https://crates.io/crates/fibre)
[![docs.rs](https://docs.rs/fibre/badge.svg)](https://docs.rs/fibre)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

`fibre` provides a suite of high-performance, memory-efficient sync/async channels for Rust, with a specialized implementation for each concurrency pattern rather than a single general-purpose one, plus locks that blocking threads and async tasks can share. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

Add Fibre to your project by including it in your `Cargo.toml`:

```toml
[dependencies]
fibre = "0" # Replace with the latest version
```

Or by using the command line:

```sh
cargo add fibre
```

There are no system prerequisites other than a standard Rust toolchain.

| Feature | Default | What it does |
|---|---|---|
| `topic` | yes | Enables `spmc::topic`; pulls in `papaya` |
| `diagnostics` | no | Enables `fibre::telemetry` recording; pulls in `tokio` |

## What to reach for

| Problem | Piece |
|---|---|
| Maximum throughput between one producer and one consumer | `fibre::spsc` |
| Many threads or tasks sending work to one processor | `fibre::mpsc` |
| Delivering every message to every consumer with backpressure | `fibre::spmc` |
| Publishing to named topics, dropping for slow consumers | `fibre::spmc::topic` |
| Many producers and many consumers, with counts that change | `fibre::mpmc` |
| Handing each item directly from sender to receiver with no buffer | `rendezvous` in `spsc`, `mpsc` or `mpmc` |
| Sending a single value once | `fibre::oneshot` |
| Request/response without allocating a channel each time | `oneshot::pair_pool()`, `oneshot::OneshotHostPool` |
| A sync thread and an async task on the same channel | `to_sync()` and `to_async()` on the handles |
| A lock that blocking threads and async tasks share | `fibre::sync::HybridMutex`, `HybridRwLock` |
| A read-heavy lock where readers must not share a written word | `fibre::sync::v2::HybridRwLock` |

## Notable Users

[Hi Stakes Markets Game](https://www.histakesgame.com) -  The worlds most advanced financial simulator, available on iPhone and Android.
