# Channel Arena: ONESHOT
**Test Machine:** MacBook M4 Pro (14 cores)

one value, one time, a fresh channel per op

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported.

Two stages, because a oneshot's channel is consumed by the op rather than standing across the run. `full-cycle` creates, sends and receives on one thread, so construction is part of the op and allocation shows. `handoff` pre-creates every pair before the barrier and times a sending thread against a receiving one, which is the same accounting the other pages use.

fibre appears four times: `fibre` is the clonable `oneshot()`, `fibre-exclusive` the single-sender `exclusive()`, and `fibre-pool` and `fibre-pool-host` take their slots from a standing pool instead of allocating. kanal, flume, crossbeam, async-channel and std have no oneshot and are absent rather than approximated with a `bounded(1)`, which is a standing channel that happens to hold one item. `futures` and `async-oneshot` have no blocking receive and `sync-oneshot` has no `Future`, so each of those runs one mode only.

Handoff rows carry a lower item ceiling than the rest of the arena, since every op needs its own pre-created channel and the item count is also the resident channel count.

## sync

| Stage | fibre | fibre-exclusive | fibre-pool | fibre-pool-host | lite-sync | oneshot | sync-oneshot | tokio |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| full-cycle | 41.9 | 47.0 | **118.6** | 114.4 | 65.7 | 48.7 | 44.6 | 31.8 |
| handoff | 82.8 | 103.2 | 189.7 | **190.5** | 97.7 | 97.5 | 80.6 | 50.1 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: fibre-exclusive at handoff (2.8x, 37.6 to 104.5), lite-sync at handoff (2.8x, 77.3 to 214.0). Those medians do not support a comparison.

## async

| Stage | fibre | fibre-exclusive | fibre-pool | fibre-pool-host | async-oneshot | futures | lite-sync | oneshot | tokio |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| full-cycle | 42.9 | 52.4 | **120.4** | 110.6 | 53.2 | 37.7 | 59.9 | 45.6 | 39.0 |
| handoff | 55.3 | 57.9 | 129.6 | 102.1 | 77.2 | 60.8 | **184.2** | 79.1 | 42.5 |

Interpretation: [FINDINGS.md](./FINDINGS.md).
