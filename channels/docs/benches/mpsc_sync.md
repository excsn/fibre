# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

`MpscUnboundedSync/Prod-{Prod}_Items-{Items}`

| Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 100000 | 508.31 µs / 544.11 µs / 587.66 µs | 170.17 Melem/s / 183.79 Melem/s / 196.73 Melem/s |
| 1 | 1000000 | 4.2894 ms / 4.3468 ms / 4.4086 ms | 226.83 Melem/s / 230.05 Melem/s / 233.13 Melem/s |
| 1 | 10000000 | 42.778 ms / 43.375 ms / 44.269 ms | 225.89 Melem/s / 230.55 Melem/s / 233.76 Melem/s |
| 4 | 100000 | 5.0459 ms / 5.4986 ms / 5.9513 ms | 16.803 Melem/s / 18.187 Melem/s / 19.818 Melem/s |
| 4 | 1000000 | 49.570 ms / 52.043 ms / 56.072 ms | 17.834 Melem/s / 19.215 Melem/s / 20.173 Melem/s |
| 4 | 10000000 | 478.82 ms / 501.97 ms / 528.37 ms | 18.926 Melem/s / 19.921 Melem/s / 20.884 Melem/s |
| 14 | 100000 | 7.1477 ms / 7.1614 ms / 7.1730 ms | 13.941 Melem/s / 13.964 Melem/s / 13.991 Melem/s |
| 14 | 1000000 | 76.632 ms / 76.831 ms / 76.953 ms | 12.995 Melem/s / 13.016 Melem/s / 13.049 Melem/s |
| 14 | 10000000 | 771.19 ms / 773.42 ms / 775.56 ms | 12.894 Melem/s / 12.930 Melem/s / 12.967 Melem/s |

## Bounded Results (`MpscBoundedSync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 13.946 ms / 14.172 ms / 14.365 ms | 6.9612 Melem/s / 7.0560 Melem/s / 7.1705 Melem/s |
| 1 | 1 | 1000000 | 140.12 ms / 152.45 ms / 167.84 ms | 5.9581 Melem/s / 6.5593 Melem/s / 7.1368 Melem/s |
| 1 | 1 | 10000000 | 1.4355 s / 1.4971 s / 1.5662 s | 6.3848 Melem/s / 6.6795 Melem/s / 6.9663 Melem/s |
| 1 | 4 | 100000 | 36.692 ms / 40.691 ms / 47.295 ms | 2.1144 Melem/s / 2.4575 Melem/s / 2.7254 Melem/s |
| 1 | 4 | 1000000 | 370.86 ms / 389.37 ms / 415.61 ms | 2.4061 Melem/s / 2.5682 Melem/s / 2.6965 Melem/s |
| 1 | 4 | 10000000 | 3.9009 s / 4.0279 s / 4.1747 s | 2.3954 Melem/s / 2.4827 Melem/s / 2.5635 Melem/s |
| 1 | 14 | 100000 | 82.520 ms / 82.844 ms / 83.111 ms | 1.2032 Melem/s / 1.2071 Melem/s / 1.2118 Melem/s |
| 1 | 14 | 1000000 | 822.65 ms / 826.78 ms / 830.71 ms | 1.2038 Melem/s / 1.2095 Melem/s / 1.2156 Melem/s |
| 1 | 14 | 10000000 | 8.2672 s / 8.2838 s / 8.2990 s | 1.2050 Melem/s / 1.2072 Melem/s / 1.2096 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 4 | 1 | 100000 | 6.9281 ms / 7.0169 ms / 7.1189 ms | 14.047 Melem/s / 14.251 Melem/s / 14.434 Melem/s |
| 4 | 1 | 1000000 | 68.764 ms / 70.970 ms / 72.617 ms | 13.771 Melem/s / 14.090 Melem/s / 14.542 Melem/s |
| 4 | 1 | 10000000 | 676.28 ms / 721.02 ms / 772.14 ms | 12.951 Melem/s / 13.869 Melem/s / 14.787 Melem/s |
| 4 | 4 | 100000 | 12.652 ms / 13.080 ms / 13.699 ms | 7.2995 Melem/s / 7.6451 Melem/s / 7.9041 Melem/s |
| 4 | 4 | 1000000 | 136.71 ms / 144.99 ms / 153.87 ms | 6.4988 Melem/s / 6.8972 Melem/s / 7.3145 Melem/s |
| 4 | 4 | 10000000 | 1.3160 s / 1.3495 s / 1.3862 s | 7.2141 Melem/s / 7.4101 Melem/s / 7.5990 Melem/s |
| 4 | 14 | 100000 | 30.554 ms / 31.037 ms / 31.546 ms | 3.1699 Melem/s / 3.2220 Melem/s / 3.2729 Melem/s |
| 4 | 14 | 1000000 | 313.43 ms / 315.24 ms / 317.48 ms | 3.1498 Melem/s / 3.1722 Melem/s / 3.1905 Melem/s |
| 4 | 14 | 10000000 | 3.0650 s / 3.1155 s / 3.1536 s | 3.1710 Melem/s / 3.2098 Melem/s / 3.2627 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 128 | 1 | 100000 | 1.3388 ms / 1.3854 ms / 1.4165 ms | 70.598 Melem/s / 72.183 Melem/s / 74.694 Melem/s |
| 128 | 1 | 1000000 | 13.487 ms / 14.129 ms / 14.858 ms | 67.304 Melem/s / 70.779 Melem/s / 74.145 Melem/s |
| 128 | 1 | 10000000 | 134.32 ms / 134.94 ms / 135.56 ms | 73.768 Melem/s / 74.108 Melem/s / 74.447 Melem/s |
| 128 | 4 | 100000 | 2.4480 ms / 2.4725 ms / 2.4955 ms | 40.071 Melem/s / 40.445 Melem/s / 40.850 Melem/s |
| 128 | 4 | 1000000 | 25.146 ms / 26.376 ms / 29.170 ms | 34.282 Melem/s / 37.913 Melem/s / 39.768 Melem/s |
| 128 | 4 | 10000000 | 243.12 ms / 248.91 ms / 255.37 ms | 39.158 Melem/s / 40.174 Melem/s / 41.131 Melem/s |
| 128 | 14 | 100000 | 2.7181 ms / 2.7862 ms / 2.8978 ms | 34.509 Melem/s / 35.891 Melem/s / 36.790 Melem/s |
| 128 | 14 | 1000000 | 26.262 ms / 26.720 ms / 27.432 ms | 36.454 Melem/s / 37.425 Melem/s / 38.078 Melem/s |
| 128 | 14 | 10000000 | 260.68 ms / 282.49 ms / 314.19 ms | 31.828 Melem/s / 35.399 Melem/s / 38.361 Melem/s |

## Bounded Batch Results (`MpscBoundedSyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut`, the
consumer drains via `recv_batch_mut`, both reusing a caller-owned buffer (allocation-free
after the drain-in-place `send_batch_mut`). Cells are **median throughput (Melem/s)** over
the batch-size axis {8, 64, 512}._

### Capacity: 1 (`Cap-1`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 3.66 | 3.58 | 3.73 |
| 1 | 1M | 3.66 | 3.70 | 3.58 |
| 1 | 10M | 3.71 | 3.72 | 3.74 |
| 4 | 100k | 0.640 | 0.645 | 0.646 |
| 4 | 1M | 0.647 | 0.640 | 0.636 |
| 4 | 10M | 0.635 | 0.616 | 0.637 |
| 14 | 100k | 0.579 | 0.565 | 0.575 |
| 14 | 1M | 0.550 | 0.571 | 0.536 |
| 14 | 10M | 0.548 | 0.563 | 0.581 |

### Capacity: 4 (`Cap-4`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 5.43 | 5.11 | 5.18 |
| 1 | 1M | 5.57 | 5.04 | 5.21 |
| 1 | 10M | 5.47 | 5.12 | 5.04 |
| 4 | 100k | 1.79 | 1.72 | 1.69 |
| 4 | 1M | 1.79 | 1.67 | 1.65 |
| 4 | 10M | 1.73 | 1.63 | 1.61 |
| 14 | 100k | 1.76 | 1.56 | 1.53 |
| 14 | 1M | 1.72 | 1.51 | 1.52 |
| 14 | 10M | 1.70 | 1.50 | 1.48 |

### Capacity: 128 (`Cap-128`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 61.3 | 59.4 | 29.8 |
| 1 | 1M | 56.0 | 57.0 | 28.8 |
| 1 | 10M | 58.7 | 54.5 | 29.9 |
| 4 | 100k | 48.1 | 53.8 | 29.1 |
| 4 | 1M | 57.3 | 53.7 | 29.9 |
| 4 | 10M | 54.5 | 53.4 | 29.5 |
| 14 | 100k | 52.8 | 48.7 | 28.3 |
| 14 | 1M | 57.1 | 56.3 | 29.4 |
| 14 | 10M | 56.3 | 52.9 | 29.1 |

**Notes.** At `Cap-128`, `Batch-8` and `Batch-64` run at 48-61 Melem/s for 1, 4 and 14 producers. `Batch-512` runs at 28-30 Melem/s. `Cap-4` runs at 5.0-5.6 Melem/s with 1 producer and 1.5-1.8 with 4 or 14. `Cap-1` runs at 3.6-3.7 Melem/s with 1 producer and 0.54-0.65 with 4 or 14.
