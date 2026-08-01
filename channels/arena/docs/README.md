# Channel Arena
**Test Machine:** MacBook M4 Pro (14 cores)

fibre against tokio, crossbeam, flume, kanal, async-channel and std, through one workload driver. Re-run with `cargo run --release` in `channels/arena`.

## Results

- [SPSC](./spsc.md) - one producer, one consumer
- [MPSC](./mpsc.md) - many producers, one consumer
- [SPMC](./spmc.md) - one producer broadcasting to many consumers (every consumer receives every item)
- [MPMC](./mpmc.md) - many producers, many consumers, work-sharing

Interpretation: [FINDINGS.md](./FINDINGS.md).

Raw per-cell data: [raw/results.tsv](./raw/results.tsv).

## Method

Each cell moves `u64` items through one channel with P producer threads (or tasks) and C consumers. Producers send an equal share each and drop their handle; consumers receive until the channel disconnects. Item count is calibrated per cell to a wall-clock target.

Timing starts once every worker has reached a barrier, excluding spawn and channel construction. Item accounting is verified per run; a cell whose counts do not balance is dropped. Sample rounds are interleaved across implementations.

Pairings are clamped to what each shape allows, so only MPMC shows all seven. A general-purpose MPMC library is measured under the SPSC, MPSC and MPMC shapes; fibre uses its specialized channel per shape.

Capacities are rendezvous, 1, 128, 1024 and unbounded, with a lower item ceiling on unbounded cells. Batched cells run at capacity 128 and above, for implementations with a batch API.

## Reproducibility

1×1 rows reproduce within a few percent. Rows at 16 and 64 threads oversubscribe a 14-core machine and do not: over four runs of MPSC/sync/cap-128/64×1, fibre held 0.579-0.601 Melem/s, kanal 0.359-0.640, crossbeam 8.43-11.9, with crossbeam's spread inside one run reaching 1.24-18.6. Per-cell min and max are in [raw/results.tsv](./raw/results.tsv).
