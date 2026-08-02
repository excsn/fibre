# Findings

228-cell run, every cell from the same run. Medians in Melem/s, matching the tables in the shape pages. High power mode, load average 1.70 at the start; the 7.83 at the end is the matrix's own 64×64 cells, which run 128 threads.

## Batching is the largest single effect in the matrix

Median gain over the same implementation's single-item path: fibre 3.50x across 76 cells, flume 1.50x across 66, tokio's `recv_many` 1.33x across 12.

The six fastest cells in the run are all fibre batched: SPSC sync 1024 1×1 at 470.4, MPSC sync 1024 1×1 at 370.3, MPSC sync unbounded 1×1 at 326.0, MPSC async 1024 1×1 at 295.5, MPSC async 1024 64×1 at 273.5, MPSC async 1024 16×1 at 268.8.

Batching is not free everywhere: it loses in 14 of 66 flume cells (down to 0.61x), 4 of 12 tokio cells (down to 0.34x), and 3 of 76 fibre cells (down to 0.82x).

## Batching removes fibre's broadcast fan-out collapse

SPMC sync, capacity 1024, per item sent: 1×1 goes 17.7 to 81.6, 1×16 goes 0.029 to 13.6, 1×64 goes 0.007 to 4.10. The last is a 585x gain, the largest in the run.

Async capacity 1024: 1×1 20.4 to 142.9, 1×16 0.759 to 58.2, 1×64 0.212 to 23.6.

Single-item fan-out to 64 consumers is where fibre's SPMC is weakest and batching is where it stops being weak.

## fibre async MPSC throughput is independent of producer count

Capacity 128, at 1×1, 16×1 and 64×1: fibre 65.6, 64.5, 63.8. kanal 82.4, 14.7, 19.5. flume 39.0, 3.33, 0.922. tokio 33.7, 5.66, 2.53. async-channel 24.6, 3.42, 1.25.

kanal leads at 1×1. fibre leads both contended rows by 3.3x or more, and is the only implementation whose throughput does not move with producer count.

## fibre bounded sync MPSC does not hold under producers

Capacity 1024, at 1×1, 16×1 and 64×1: fibre 176.0, 3.34, 0.863. crossbeam 62.4, 14.5, 21.0. std 117.4, 0.416, 0.216.

fibre has the fastest 1×1 cell and one of the slowest 64×1 cells in the same row.

## Unbounded capacity removes most of that collapse

MPSC sync 64×1: bounded 1024 gives 0.863, unbounded gives 19.3, a 22x difference on the same shape and thread count. At 16×1 it is 3.34 against 13.0.

Unbounded MPSC sync 1×1 reaches 231.5, the fastest single-item cell in the run.

## fibre's unbounded async does not hold flat, unlike its bounded async

MPSC async unbounded, at 1×1, 16×1 and 64×1: fibre 111.6, 13.0, 12.4. flume 53.8, 25.0, 26.4. kanal 105.6, 44.5, 24.8.

fibre leads unbounded async at 1×1 and trails flume and kanal at both contended rows, which is the reverse of the bounded async result above.

## fibre sync MPSC at 1×1 exceeds fibre sync SPSC at 1×1

Capacity 1024, one producer and one consumer: MPSC 176.0, SPSC 46.6.

Async inverts it: SPSC 154.9, MPSC 75.4.

## fibre sync at capacity 1 trails by an order of magnitude

SPSC sync, capacity 1: fibre 0.263, std 0.281, flume 0.675, crossbeam 7.90, kanal 8.57.

## fibre takes six of seven MPMC sync rows

MPMC sync capacity 1024: fibre takes 16×1 (24.2 against crossbeam 17.5), 1×16 (27.5 against 12.9), 1×64 (20.4 against 12.8), 64×1 (19.0 against 6.02), 64×64 (36.0 against 16.1) and 16×16 (43.9 against 43.1, close enough to be a tie). crossbeam takes 1×1 (63.0 against 52.9).

Skew direction makes little difference to fibre: 16×1 and 1×16 land within 14% of each other.

## MPMC async splits by consumer count

kanal takes 1×1 (76.3 against fibre 57.0), 16×16 (45.8 against 23.7), 16×1 (22.3 against 18.5) and 64×1 (30.6 against 7.06). fibre takes 64×64 (21.3 against 9.72), 1×16 (20.3 against 14.2) and 1×64 (10.3 against 2.09).

Every row fibre takes has 16 or more consumers; kanal's 64×1 lead is 4.3x and fibre's 1×64 lead is 4.9x.

## std::sync::mpsc holds the single-producer sync rows

MPSC sync 1024 1×1: 117.4, second only to fibre's 176.0 and ahead of crossbeam's 62.4. At 16×1 and 64×1 the same channel gives 0.416 and 0.216.

## Pooling is worth 2.8x on a oneshot

Full cycle, sync: fibre-pool 118.6 against fibre 41.9. Async: 120.4 against 42.9. Same transfer engine either way, differing only in whether the slot is allocated or taken from a standing pool.

fibre-pool or fibre-pool-host takes three of the four oneshot cells, and is 1.8x clear of the next implementation on both full-cycle cells.

## The two oneshot stages separate allocation cost from transfer cost

Going from full cycle to handoff removes the per-op construction. Async: fibre 42.9 to 55.3, async-oneshot 53.2 to 77.2, the `oneshot` crate 45.6 to 79.1, fibre-pool 120.4 to 129.6.

The implementations that allocate gain up to 73%. fibre-pool gains 8%, because a pooled create is a freelist pop and there is little construction there to remove.

## A blocking receive beats an async one on oneshot handoff

fibre-pool reads 189.7 sync against 129.6 async, fibre-pool-host 190.5 against 102.1, fibre-exclusive 103.2 against 57.9. Same channel and same slots in each pair, so the difference is the task machinery around the await, not the transfer.

Full cycle does not invert: there the per-op construction dominates and the two modes land within 2%.

## The pair pool and the host pool separate only under async handoff

Sync handoff puts them within 1% (189.7 against 190.5) and full cycle within 9%. Async handoff separates them by 27%: 129.6 against 102.1.

Their record-carrying difference, which is the reason the host pool exists, is not measured here; it is in fibre's own [oneshot pool bench](../../docs/benches/oneshot_pool.md).

## lite-sync takes async oneshot handoff and cannot be ranked on sync handoff

It wins async handoff outright at 184.2 against fibre-pool's 129.6, with a tight spread this run. Its full-cycle cells hold too, at 65.7 sync and 59.9 async, second only to the pool.

Its sync handoff does not hold: 77.3 to 214.0 across one cell's samples, so read that 97.7 median as a coin flip. Across four separate runs its async handoff cell has read 103.8, 119.6, 188.2 and 184.2, so the win is real but its size is not settled.

## Reproducibility

1×1 rows reproduce within a few percent, and this run flags the six that did not: kanal at SPSC sync unbounded (2.0x), flume at MPSC async 1024 (2.0x), fibre and flume at MPMC sync unbounded (2.8x and 2.3x), and fibre-exclusive and lite-sync at oneshot sync handoff (2.8x each). Those carry a note under their table and do not support a comparison.

Contended rows do not reproduce and are not flagged individually, because nearly all of them would be. The widest here: crossbeam at MPSC sync 128 64×1 spread 0.166 to 14.5 (87x), crossbeam at MPMC sync 128 64×1 spread 0.165 to 14.0 (85x), fibre at SPMC sync 1024 1×64 spread 0.006 to 0.362 (60x).

Claims above rest on 1×1 rows or on gaps of 3x or more.
