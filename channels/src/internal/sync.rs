//! Crate-wide primitive switch for loom model checking.
//!
//! Migrated channels import their sync primitives from here instead of
//! `std`/`parking_lot`. In normal builds these are zero-cost re-exports
//! ([`real`]); under `RUSTFLAGS="--cfg loom"` they swap to loom's modeled
//! types ([`mocked`]) so `loom::model` can explore every interleaving. Loom
//! tests live centrally in `src/loom_tests/`; run via `channels/scripts/loom.sh`.
//!
//! This selector and the single gate in `lib.rs` are the ONLY `cfg(loom)`
//! sites in the source tree - `real.rs` and `mocked.rs` are cfg-free, channel
//! files carry one plain import and no cfg at all.
//!
//! Migration rules:
//! - Atomics left on `std` in un-migrated modules merely lose loom coverage
//!   (loom can't explore orderings it can't see) - they still compile and run.
//! - Any BLOCKING primitive reached by a loom test (`Mutex`, `thread::park`)
//!   MUST come from here: loom runs threads one at a time, so a real OS block
//!   inside a model stalls the only running thread and deadlocks the run.
//! - Payload `UnsafeCell`s currently stay on `std` (loom's closure-API cell
//!   would be invasive churn); protocol/ordering bugs are still caught via the
//!   atomics, and payload races remain miri's job (`scripts/miri.sh`).
//! - Time is not modeled: `mocked` provides panicking `thread::sleep` /
//!   `thread::park_timeout` so everything compiles, but loom tests must not
//!   exercise timeout paths.

#[cfg(not(loom))]
mod real;
#[cfg(loom)]
mod mocked;

#[cfg(not(loom))]
pub(crate) use real::*;
#[cfg(loom)]
pub(crate) use mocked::*;
