# Findings

224-cell run. Medians in Melem/s, matching the tables in the shape pages. The machine was not idle during this run (a browser was consuming roughly one core of fourteen), which matters for the oversubscribed rows and not for the 1×1 rows.

## Batching is the largest single effect in the matrix

Median gain over the same implementation's single-item path: fibre 3.78x across 76 cells, flume 1.39x across 66, tokio's `recv_many` 1.10x across 12.

The six fastest cells in the run are all fibre batched: SPSC sync 1024 1×1 at 285.5, MPSC sync 1024 1×1 at 253.3, MPMC sync unbounded 1×1 at 218.1, MPSC async 1024 1×1 at 196.4, MPSC sync unbounded 1×1 at 193.1, MPMC sync 1024 1×1 at 179.7.

Batching is not free everywhere: it loses in 20 of 66 flume cells (down to 0.53x), 3 of 12 tokio cells (down to 0.41x), and 2 of 76 fibre cells (0.95x).

## Batching removes fibre's broadcast fan-out collapse

SPMC sync, capacity 1024, per item sent: 1×1 goes 10.4 to 59.5, 1×16 goes 0.073 to 10.3, 1×64 goes 0.005 to 3.60. The last is a 720x gain, the largest in the run.

Async capacity 1024: 1×1 11.4 to 95.6, 1×16 0.612 to 37.3, 1×64 0.173 to 15.3.

Single-item fan-out to 64 consumers is where fibre's SPMC is weakest and batching is where it stops being weak.

## fibre async MPSC throughput is independent of producer count

Capacity 128, at 1×1, 16×1 and 64×1: fibre 37.5, 37.6, 35.2. kanal 50.9, 9.15, 9.98. flume 28.7, 2.69, 0.78. tokio 20.3, 4.00, 1.75. async-channel 15.4, 2.58, 0.96.

kanal leads at 1×1. fibre leads both contended rows by 3.5x or more.

## fibre bounded sync MPSC does not hold under producers

Capacity 1024, at 1×1, 16×1 and 64×1: fibre 135.3, 2.66, 0.67. crossbeam 44.9, 8.44, 7.37. std 118.4, 0.27, 0.18.

fibre has the fastest 1×1 cell and one of the slowest 64×1 cells in the same row.

## Unbounded capacity removes most of that collapse

MPSC sync 64×1: bounded 1024 gives 0.67, unbounded gives 14.6, a 22x difference on the same shape and thread count. At 16×1 it is 2.66 against 10.7.

Unbounded MPSC sync 1×1 reaches 152.3, the fastest single-item cell in the run.

## fibre's unbounded async does not hold flat, unlike its bounded async

MPSC async unbounded, at 1×1, 16×1 and 64×1: fibre 73.4, 10.6, 10.4. flume 38.9, 19.1, 19.4. kanal 68.1, 11.6, 23.2.

fibre leads unbounded async at 1×1 and trails flume and kanal at both contended rows, which is the reverse of the bounded async result above.

## fibre sync MPSC at 1×1 exceeds fibre sync SPSC at 1×1

Capacity 1024, one producer and one consumer: MPSC 135.3, SPSC 23.4.

Async does not invert: SPSC 102.4, MPSC 49.2.

## fibre sync at capacity 1 trails by an order of magnitude

SPSC sync, capacity 1: fibre 0.226, std 0.222, flume 0.583, crossbeam 5.14, kanal 5.20.

## fibre leads most MPMC rows once either side is loaded

MPMC sync capacity 1024, across all seven pairings: fibre takes 16×1 (18.5 against crossbeam 7.55), 1×16 (20.7 against 7.37), 1×64 (14.3 against 7.64), 16×16 (37.0 against 25.4) and 64×64 (28.0 against 4.26). crossbeam takes 1×1 (41.9 against 32.4) and 64×1 (10.3 against 8.82).

MPMC async capacity 128: fibre takes the same five, kanal takes 1×1 (52.7 against 39.4) and 64×1 (12.7 against 6.07).

Skew direction makes little difference to fibre: 16×1 and 1×16 land within 12% of each other at capacity 1024.

## std::sync::mpsc leads the single-producer sync rows

MPSC sync 1024 1×1: 118.4, second only to fibre's 135.3 and ahead of crossbeam's 44.9. At 16×1 and 64×1 the same channel gives 0.27 and 0.18.

## kanal leads the uncontended async rows

MPSC async capacity 128 1×1: 50.9 against fibre 37.5. MPMC async capacity 128 1×1: 52.7 against fibre 39.4. Its advantage does not survive contention in MPSC (9.15 at 16×1) but does in MPMC 64×1 (12.7).

## Reproducibility

1×1 rows reproduce within a few percent. Contended rows do not. Four consecutive runs of MPSC/sync/cap-128/64×1: fibre 0.579-0.601, kanal 0.359-0.640, crossbeam 8.43-11.9, with crossbeam's per-sample spread inside one run reaching 1.24-18.6. Across two full runs kept during development, crossbeam at MPSC/sync/cap-1024/64×1 read 14.0 and 1.88.

Claims above rest on 1×1 rows or on gaps of 3x or more.
