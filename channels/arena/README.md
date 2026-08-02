# channels_arena

Cross-implementation channel benchmarks: fibre against tokio, crossbeam-channel, flume, kanal, async-channel, futures, the `oneshot`, `async-oneshot`, `lite-sync` and `sync-oneshot` crates and `std::sync::mpsc`, through one shared workload harness.

Results live in [docs/](./docs/).

## Running

```sh
cd channels/arena
cargo run --release -- --machine "MacBook M4 Pro (14 cores)"
```

Useful flags (`--help` lists them all):

```sh
cargo run --release -- --list                        # print the matrix, run nothing
cargo run --release -- --flavor mpmc --mode async    # one slice
cargo run --release -- --library fibre,kanal         # head-to-head
cargo run --release -- --samples 15 --target-ms 200  # longer, quieter run
cargo run --release -- --rerender docs/raw/results.tsv   # rewrite pages, measure nothing
```

`--rerender` rebuilds the pages from a previous run's TSV, for editing page text without re-measuring.

Everything writes to `--out` (default `docs/`): one Markdown page per shape, an index, and `docs/raw/results.tsv`.

## The matrix

| Axis | Values |
| :--- | :--- |
| Shape | spsc, mpsc, spmc (broadcast), mpmc, oneshot |
| Mode | sync, async |
| Capacity | rendezvous, 1, 128, 1024, unbounded |
| Producers × consumers | 1×1, 16×16, 64×64, 16×1, 1×16, 64×1, 1×64, clamped to the shape |
| API | single item, batched (default 512 per call) |
| Stage | oneshot only: full-cycle, handoff |

Full matrix is 228 cells, +/- five minutes.

Pairings are clamped to what each shape allows, so the skewed entries collapse into the symmetric rows everywhere except MPMC, which shows all seven. A general-purpose MPMC library is measured under the SPSC, MPSC and MPMC shapes alike; fibre is measured through its specialized channel for each shape.

Batched cells run at capacity 128 and above, and only for implementations with a batch API. Unbounded cells carry a lower item ceiling, since producers can otherwise hold the whole run in memory at once.

An implementation that cannot serve a cell returns `None` from `build` and is reported as `-`.

Oneshot sits outside the axes above. Its channel is spent by one op, so capacity, pairing and batching each have a single legal value, and the axis left to vary is the stage: `full-cycle` puts channel construction inside the timed region, `handoff` pre-creates every pair before the barrier. Handoff carries a lower item ceiling than the rest of the matrix, since the item count is also the count of channels resident at once.

Sample rounds are interleaved across implementations within a cell.

## Before a run

`cargo test` pushes a few thousand items through the single and batch paths of every adapter and asserts the item accounting balances. Not a benchmark; finishes in seconds. `cargo run --release -- --list` prints the matrix and what each cell will run, measuring nothing.

**Nothing else should be running.** Not a compile, not the test suite, not a browser. A run claims `docs/.arena-running` and a second run refuses to start, but that guards only against another arena, not against the rest of the machine.

## Adding an implementation

An adapter is a `build`/`send`/`recv` triple; the workload, calibration, timing and reporting are shared. One macro invocation:

```rust
sync_adapter! {
  name: Sync_,
  sender: some_crate::Sender<Payload>,
  receiver: some_crate::Receiver<Payload>,
  senders: clone,        // or `single` for a non-cloneable handle
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(some_crate::bounded::<Payload>(0)),
    Capacity::Bounded(n) => Some(some_crate::bounded::<Payload>(n)),
    Capacity::Unbounded => Some(some_crate::unbounded::<Payload>()),
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}
```

Then register it in `src/registry.rs` against each shape it should be measured under. `async_adapter!` is identical except the `send`/`recv` bodies may `.await`. Return `None` from `build` for any capacity the library does not have.

A oneshot uses `oneshot_sync_adapter!` / `oneshot_async_adapter!` instead, since its handles are consumed by the operation rather than reused across the run. Those take a `store` clause for whatever must outlive the pairs, which is `()` for an implementation that allocates each channel and the pool itself for one that does not.

A second adapter is needed only when a capacity mode uses a *different handle type* (fibre's rendezvous and unbounded channels, `std`'s `channel` vs `sync_channel`). The two declare disjoint capacity support and report under one library name.

For a batch API, add the clauses for whichever sides it supports:

```rust
  batch: SendRecv,          // or `Recv` for a receive-only API
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
```

An omitted side keeps the trait's default, which loops the single-item call. `send_batch` must drain accepted items from the front of `items`. `recv_batch` appends to `out` and returns the count, `0` meaning disconnected.

## Layout

| File | Role |
| :--- | :--- |
| `src/spec.rs` | matrix vocabulary: shape, mode, capacity, pairing, api, stage, cell |
| `src/channel.rs` | the four adapter traits and the fan-out helpers |
| `src/driver.rs` | the workload: barrier, produce, consume, verify counts |
| `src/measure.rs` | calibration to a wall-clock budget, sampling, stats |
| `src/bench.rs` | type erasure so one registry can hold every adapter |
| `src/registry.rs` | which implementation is measured under which shape |
| `src/report.rs` | Markdown tables and TSV |
| `src/adapters/` | one file per library, plus `oneshot_ch.rs` for every oneshot |
| `tests/driver.rs` | item-accounting checks for the single and batch paths |
