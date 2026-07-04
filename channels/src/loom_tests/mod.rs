//! Central loom model checks, one file per migrated channel.
//!
//! Gated once in `lib.rs` (`#[cfg(all(test, loom))]`) — no cfg in this tree.
//! Run: `channels/scripts/loom.sh [filter] [max-preemptions]`.
//!
//! Ground rules (loom is exponential in operations × threads):
//! - Tiny models ONLY: capacity 1–2, 1–2 items per thread, ≤2 spawned threads
//!   (loom's default max is 4 including main).
//! - Drive channels through the public API (`crate::mpsc::bounded(..)` etc.);
//!   the channel's internals must have been migrated to `internal::sync` first,
//!   or its blocking calls will deadlock the model (see internal/sync.rs).
//! - No timeout APIs, no sleeps — the mocked primitives panic on them.
//! - A lost wakeup shows up as a loom "deadlock" report; a protocol bug as an
//!   assert/panic with the exact interleaving trace.

mod mpmc_bounded;
mod mpmc_rendezvous;
mod mpmc_unbounded;
mod mpsc_bounded;
mod mpsc_unbounded;
mod spmc_bounded;
mod spsc_bounded;
mod spsc_rendezvous;
