# fibre_miri — the dedicated miri suite

This crate is the **only** place miri runs. The main `fibre` crate and its
integration tests carry no `cfg(miri)` flags at all; if you find yourself
adding one there, the test belongs here instead.

## Running

```sh
# The whole suite (the normal invocation):
cargo +nightly miri test -p fibre_miri

# One channel type:
cargo +nightly miri test -p fibre_miri --test mpsc_unbounded

# Explore more thread interleavings on the concurrency tests (slower):
MIRIFLAGS="-Zmiri-many-seeds=0..16" cargo +nightly miri test -p fibre_miri
```

The tests also compile and pass under plain `cargo test -p fibre_miri` — they
are ordinary fast native tests when not interpreted, which doubles as a cheap
smoke suite. That is a side effect, not the purpose; behavioral coverage lives
in `channels/tests/`.

## What this crate is for

Miri detects **undefined behavior**: stacked/tree-borrows violations in the
unsafe cores, data races (miri has a real race detector), double-drops, leaks,
and invalid atomics usage. It is not a behavior-regression suite. Every test
here is shaped for the interpreter:

- **Small counts.** Miri is 100-1000x slower than native. Use
  `ITEMS_SMALL`/`ITEMS_CROSSING` from `fibre_miri`'s lib; `ITEMS_CROSSING`
  (300) is sized to cross every internal chunk boundary in the crate at least
  twice (bounded mpsc recycle chunks = 64, unbounded mpsc slabs = 128).
  If you change one of those internal sizes past ~150, bump `ITEMS_CROSSING`.
- **No tokio.** Runtimes need timers/epoll, which miri can't provide. Async
  paths are exercised two ways instead:
  - `poll_once(pin!(fut))` — one manual poll with a no-op waker. Poll to
    `Pending` (drives waiter *registration*), then drop the future (drives the
    cancel-safety `Drop`/unregister path). This reaches the lost-wakeup-shaped
    code that tokio-based tests can never reach under miri.
  - `block_on` (re-exported from `futures`) — for cross-thread completion;
    its park/unpark waker works under miri.
- **No sleeps, no timing assumptions.** A blocking `recv()` on one thread
  woken by a `send()` on another is fine (miri supports park/unpark); a test
  that only passes if thread A runs before thread B is not.
- **Ownership checks via `DropCounter`.** Fill a channel, drop it with items
  in flight, assert the count. Miri escalates any double-drop or leak into a
  hard failure.

## Suite layout

One file per channel family, mirroring `fibre`'s module structure:

| File | Covers |
| :-- | :-- |
| `tests/spsc.rs` | owned-index ring (wraparound, non-pow2 caps), waiter protocol, sync<->async conversion |
| `tests/mpsc_bounded.rs` | Vyukov publish, node pool + chunk recycling, capacity park/wake |
| `tests/mpsc_unbounded.rs` | v3 slab lifecycle (seal, refcount, partial slabs), v1 smoke |
| `tests/mpmc.rs` | mpmc_v2 bounded/unbounded under producer+consumer races |
| `tests/spmc.rs` | broadcast ring with per-consumer cursors |
| `tests/oneshot.rs` | single-shot ownership transfer, competing senders |
| `tests/rendezvous.rs` | zero-capacity direct handoff (spsc/mpsc/mpmc variants) |

Each file follows the same template: single-threaded smoke → drop/ownership
checks → manual-poll register/cancel paths → a small 2-4-thread race test for
miri's race detector.

## Adding a test

1. Pick the channel file (or add a new one for a new channel type).
2. Target an *unsafe mechanism*, not a behavior: slab seal at partial fill,
   ring wraparound at a non-power-of-two capacity, a future dropped while
   registered, a handle dropped mid-operation.
3. Keep it deterministic and small; if it needs more than a few hundred items
   or more than ~4 threads, it's probably a stress test and belongs in
   `channels/tests/` instead.
