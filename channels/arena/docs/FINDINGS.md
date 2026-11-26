# Findings

Medians in Melem/s, matching the tables in the shape pages. The fibre rows (fibre, fibre-exclusive, fibre-pool and fibre-pool-host) come from a 228-cell run on 2026-11-25 in high power mode, load average 5.69 at the start and 16.05 at the end, most of it the matrix's own 64×64 cells, which run 128 threads. The other libraries' rows come from earlier runs and were not re-measured, so a comparison between fibre and another library spans two sittings.

## Batching is the largest single effect in the matrix

Median gain over the same implementation's single-item path: fibre 3.86x across 76 cells, flume 1.46x across 66, tokio's `recv_many` 1.19x across 12.

The six fastest streaming cells are all fibre batched: SPSC sync 1024 1×1 at 465.8, MPMC sync unbounded 1×1 at 332.7, MPSC sync unbounded 1×1 at 332.0, MPSC async 1024 1×1 at 302.5, MPSC async 1024 64×1 at 294.3, MPSC async 1024 16×1 at 292.4.

Batching is not free everywhere: it loses in 14 of 66 flume cells (down to 0.61x), 5 of 12 tokio cells (down to 0.37x) and 4 of 76 fibre cells (down to 0.35x, at MPSC sync 128 1×1).

## Batching removes fibre's broadcast fan-out collapse

SPMC sync, capacity 1024, per item sent: 1×1 goes 21.5 to 98.2, 1×16 goes 0.043 to 15.0, 1×64 goes 0.024 to 4.64. The 1×16 gain is 350x, the largest in the run.

Async capacity 1024: 1×1 18.3 to 146.0, 1×16 0.814 to 52.3, 1×64 0.213 to 22.0.

Single-item fan-out to 64 consumers is where fibre's SPMC is weakest and batching is where it stops being weak.

## fibre async MPSC throughput is independent of producer count

Capacity 128, at 1×1, 16×1 and 64×1: fibre 63.3, 63.4, 65.7. kanal 78.9, 9.13, 8.67. crossfire 69.8, 11.4, 7.31. flume 38.3, 4.02, 1.05. tokio 33.2, 6.07, 2.49. async-channel 24.0, 3.66, 1.34.

kanal leads at 1×1. fibre leads both contended rows by 5.6x or more. It is the only implementation whose throughput does not move with producer count.

## fibre bounded sync MPSC drops 6x under producers and still leads

Capacity 1024, at 1×1, 16×1 and 64×1: fibre 151.9, 25.8, 25.9. std 144.0, 0.247, 0.224. crossbeam 65.5, 13.6, 0.969. crossfire 27.8, 14.3, 11.7.

fibre has the fastest cell in all three rows. Its 16×1 and 64×1 cells are a sixth of its 1×1.

## Bounded capacity 1024 beats unbounded under producers

MPSC sync 64×1: bounded 1024 gives 25.9, unbounded gives 18.1. At 16×1 it is 25.8 against 13.1.

Unbounded MPSC sync 1×1 reaches 232.0, the fastest fibre single-item streaming cell in the run and second only to crossfire's MPMC sync 1024 1×1 at 238.0.

## fibre's unbounded async does not hold flat, unlike its bounded async

MPSC async unbounded, at 1×1, 16×1 and 64×1: fibre 108.6, 13.2, 12.6. crossfire 109.2, 48.0, 17.0. kanal 105.2, 45.2, 22.5. flume 60.5, 31.4, 32.2.

fibre and crossfire tie at 1×1. fibre trails crossfire, kanal and flume at both contended rows, which is the reverse of the bounded async result above.

## fibre sync MPSC at 1×1 exceeds fibre sync SPSC at 1×1

Capacity 1024, one producer and one consumer: MPSC 151.9, SPSC 53.4.

Async inverts it: SPSC 164.6, MPSC 75.9.

## fibre sync at capacity 1 trails by an order of magnitude

SPSC sync, capacity 1: fibre 0.255, std 0.281, flume 0.675, crossbeam 7.90, kanal 8.57.

## crossfire takes every MPMC sync row at capacity 1024

MPMC sync capacity 1024: crossfire takes 1×1 (238.0 against crossbeam 63.0), 16×16 (66.7 against fibre 43.2), 64×64 (49.4 against fibre 30.0), 16×1 (31.9 against fibre 24.2), 1×16 (69.1 against fibre 27.1), 64×1 (25.6 against fibre 14.3) and 1×64 (31.2 against fibre 15.2). fibre is second in every row but 1×1, where crossbeam is second and fibre third at 53.8.

Skew direction makes little difference to fibre: 16×1 and 1×16 land within 12% of each other.

## MPMC async splits by consumer count

Capacity 128: kanal takes 1×1 (76.3 against fibre 70.0), 16×16 (45.8 against 25.3) and 64×1 (30.6 against 12.0). fibre takes 64×64 (28.9 against flume 13.1), 1×16 (23.4 against kanal 14.2) and 1×64 (11.3 against crossfire 9.43). fibre and kanal tie at 16×1 (22.8 against 22.3).

Every row fibre takes has 16 or more consumers. kanal's 64×1 lead is 2.6x and fibre's 1×64 lead over kanal is 5.4x.

At capacity 1024 fibre takes all six contended rows and trails crossfire and kanal at 1×1 (70.7 against 85.1 and 81.2).

## std::sync::mpsc holds the single-producer sync rows

MPSC sync 1024 1×1: 144.0, second only to fibre's 151.9 and ahead of crossbeam's 65.5. At 16×1 and 64×1 the same channel gives 0.247 and 0.224.

## Pooling is worth 3.6x to 3.9x on a oneshot

Full cycle, sync: fibre-pool 152.3 against fibre 39.1. Async: 134.2 against 37.7. Same transfer engine either way, differing only in whether the slot is allocated or taken from a standing pool.

fibre-pool or fibre-pool-host takes three of the four oneshot cells. On both full-cycle cells it is 2.2x clear of the next implementation.

## The two oneshot stages separate allocation cost from transfer cost

Going from full cycle to handoff removes the per-op construction. Async: fibre 37.7 to 56.3, async-oneshot 53.2 to 77.2, the `oneshot` crate 45.6 to 79.1, fibre-pool 134.2 to 173.5.

The implementations that allocate gain up to 73%. fibre-pool gains 29%: a pooled create is a freelist pop, so there is less construction to remove.

## A blocking receive beats an async one on oneshot handoff

fibre-pool reads 336.0 sync against 173.5 async, fibre-pool-host 309.1 against 171.4, fibre-exclusive 99.3 against 74.9. Same channel and same slots in each pair, so the difference is the task machinery around the await, not the transfer.

Full cycle does not invert: the per-op construction takes a larger share there and the two modes land within 14%, within 2% for fibre-exclusive.

## The pair pool and the host pool land within 9% of each other

Sync handoff separates them most, by 9% (336.0 against 309.1). Async handoff puts them within 2% (173.5 against 171.4) and full cycle within 3%.

Their record-carrying difference, which is the reason the host pool exists, is not measured here; it is in fibre's own [oneshot pool bench](../../docs/benches/oneshot_pool.md).

## lite-sync takes async oneshot handoff and cannot be ranked on sync handoff

It wins async handoff outright at 184.2 against fibre-pool's 173.5. Its full-cycle cells hold too, at 65.7 sync and 59.9 async, second only to the pool.

Its sync handoff does not hold: 77.3 to 214.0 across one cell's samples, so read that 97.7 median as a coin flip. Across four separate runs its async handoff cell has read 103.8, 119.6, 188.2 and 184.2, so the win is real but its size is not settled.

## Reproducibility

1×1 rows reproduce within a few percent. Nine did not. Each carries a note under its table and does not support a comparison:

- fibre at SPSC sync 128 batched (4.1x), SPMC sync 1 (3.5x) and MPMC sync 1024 batched (6.9x)
- crossfire at MPSC async 1 (2.7x), MPSC async 1024 (2.4x) and MPMC async 1 (3.3x)
- kanal at SPSC sync unbounded (2.0x)
- flume at MPMC sync unbounded (2.3x)
- lite-sync at oneshot sync handoff (2.8x)

Contended rows do not reproduce and are not flagged individually, because nearly all of them would be. The widest here: crossbeam at MPSC sync 128 64×1 spread 0.161 to 19.1 (119x), fibre at MPMC sync unbounded 1×16 spread 0.484 to 48.6 (101x), crossbeam at MPMC sync 128 64×1 spread 0.165 to 14.0 (85x).

Claims above rest on 1×1 rows or on gaps of 3x or more.
