# Fibre Benchmark: `MpscAsync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedAsync`)

`MpscUnboundedAsync/Prod-{Prod}_Items-{Items}`

| Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 100000 | 772.77 µs / 773.85 µs / 776.03 µs | 128.86 Melem/s / 129.22 Melem/s / 129.41 Melem/s |
| 1 | 1000000 | 8.1088 ms / 8.2165 ms / 8.3176 ms | 120.23 Melem/s / 121.71 Melem/s / 123.32 Melem/s |
| 1 | 10000000 | 81.548 ms / 81.799 ms / 82.232 ms | 121.61 Melem/s / 122.25 Melem/s / 122.63 Melem/s |
| 4 | 100000 | 5.4942 ms / 5.8235 ms / 6.1898 ms | 16.156 Melem/s / 17.172 Melem/s / 18.201 Melem/s |
| 4 | 1000000 | 36.689 ms / 37.412 ms / 39.224 ms | 25.495 Melem/s / 26.729 Melem/s / 27.256 Melem/s |
| 4 | 10000000 | 368.06 ms / 373.84 ms / 380.26 ms | 26.298 Melem/s / 26.749 Melem/s / 27.170 Melem/s |
| 14 | 100000 | 7.2790 ms / 7.3105 ms / 7.3447 ms | 13.615 Melem/s / 13.679 Melem/s / 13.738 Melem/s |
| 14 | 1000000 | 76.616 ms / 76.796 ms / 77.035 ms | 12.981 Melem/s / 13.021 Melem/s / 13.052 Melem/s |
| 14 | 10000000 | 773.45 ms / 775.82 ms / 778.17 ms | 12.851 Melem/s / 12.890 Melem/s / 12.929 Melem/s |

---

## Bounded Results (`MpscBoundedAsync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 11.005 ms / 11.201 ms / 11.543 ms | 8.6629 Melem/s / 8.9274 Melem/s / 9.0870 Melem/s |
| 1 | 1 | 1000000 | 110.73 ms / 111.74 ms / 112.76 ms | 8.8685 Melem/s / 8.9494 Melem/s / 9.0308 Melem/s |
| 1 | 1 | 10000000 | 1.0804 s / 1.0903 s / 1.1008 s | 9.0840 Melem/s / 9.1719 Melem/s / 9.2556 Melem/s |
| 1 | 4 | 100000 | 11.190 ms / 11.275 ms / 11.397 ms | 8.7742 Melem/s / 8.8690 Melem/s / 8.9369 Melem/s |
| 1 | 4 | 1000000 | 110.42 ms / 111.70 ms / 112.75 ms | 8.8694 Melem/s / 8.9524 Melem/s / 9.0564 Melem/s |
| 1 | 4 | 10000000 | 1.0715 s / 1.0830 s / 1.0949 s | 9.1332 Melem/s / 9.2333 Melem/s / 9.3327 Melem/s |
| 1 | 14 | 100000 | 11.469 ms / 11.520 ms / 11.597 ms | 8.6228 Melem/s / 8.6808 Melem/s / 8.7188 Melem/s |
| 1 | 14 | 1000000 | 111.47 ms / 113.61 ms / 115.57 ms | 8.6525 Melem/s / 8.8023 Melem/s / 8.9709 Melem/s |
| 1 | 14 | 10000000 | 1.1055 s / 1.1197 s / 1.1364 s | 8.7994 Melem/s / 8.9312 Melem/s / 9.0459 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 4 | 1 | 100000 | 3.9980 ms / 4.0503 ms / 4.1093 ms | 24.335 Melem/s / 24.689 Melem/s / 25.012 Melem/s |
| 4 | 1 | 1000000 | 40.192 ms / 40.471 ms / 40.760 ms | 24.534 Melem/s / 24.709 Melem/s / 24.880 Melem/s |
| 4 | 1 | 10000000 | 405.10 ms / 410.06 ms / 415.99 ms | 24.039 Melem/s / 24.387 Melem/s / 24.685 Melem/s |
| 4 | 4 | 100000 | 4.1518 ms / 4.2606 ms / 4.4230 ms | 22.609 Melem/s / 23.471 Melem/s / 24.086 Melem/s |
| 4 | 4 | 1000000 | 41.207 ms / 41.951 ms / 42.854 ms | 23.335 Melem/s / 23.837 Melem/s / 24.268 Melem/s |
| 4 | 4 | 10000000 | 407.64 ms / 409.53 ms / 411.28 ms | 24.314 Melem/s / 24.418 Melem/s / 24.532 Melem/s |
| 4 | 14 | 100000 | 4.3193 ms / 4.3700 ms / 4.4388 ms | 22.529 Melem/s / 22.883 Melem/s / 23.152 Melem/s |
| 4 | 14 | 1000000 | 42.644 ms / 42.749 ms / 42.875 ms | 23.323 Melem/s / 23.392 Melem/s / 23.450 Melem/s |
| 4 | 14 | 10000000 | 426.91 ms / 433.24 ms / 441.11 ms | 22.670 Melem/s / 23.082 Melem/s / 23.424 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 128 | 1 | 100000 | 1.4710 ms / 1.4780 ms / 1.4888 ms | 67.169 Melem/s / 67.659 Melem/s / 67.983 Melem/s |
| 128 | 1 | 1000000 | 14.909 ms / 14.982 ms / 15.071 ms | 66.354 Melem/s / 66.748 Melem/s / 67.073 Melem/s |
| 128 | 1 | 10000000 | 150.31 ms / 151.57 ms / 152.63 ms | 65.519 Melem/s / 65.976 Melem/s / 66.528 Melem/s |
| 128 | 4 | 100000 | 2.0229 ms / 2.0762 ms / 2.1369 ms | 46.797 Melem/s / 48.166 Melem/s / 49.433 Melem/s |
| 128 | 4 | 1000000 | 16.282 ms / 16.609 ms / 17.184 ms | 58.193 Melem/s / 60.209 Melem/s / 61.418 Melem/s |
| 128 | 4 | 10000000 | 150.84 ms / 158.72 ms / 179.09 ms | 55.837 Melem/s / 63.002 Melem/s / 66.295 Melem/s |
| 128 | 14 | 100000 | 2.0897 ms / 2.1169 ms / 2.1627 ms | 46.239 Melem/s / 47.238 Melem/s / 47.853 Melem/s |
| 128 | 14 | 1000000 | 15.603 ms / 15.771 ms / 15.881 ms | 62.967 Melem/s / 63.406 Melem/s / 64.092 Melem/s |
| 128 | 14 | 10000000 | 148.33 ms / 150.66 ms / 152.95 ms | 65.379 Melem/s / 66.376 Melem/s / 67.418 Melem/s |

## Bounded Batch Results (`MpscBoundedAsyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut().await`,
the consumer drains via `recv_batch_mut().await`, both reusing a caller-owned buffer.
Cells are **median throughput (Melem/s)** over the batch-size axis {8, 64, 512}._

### Capacity: 1 (`Cap-1`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 8.73 | 8.37 | 7.16 |
| 1 | 1M | 8.87 | 7.96 | 7.12 |
| 1 | 10M | 8.91 | 8.53 | 6.90 |
| 4 | 100k | 8.67 | 8.19 | 6.78 |
| 4 | 1M | 8.82 | 8.40 | 6.78 |
| 4 | 10M | 8.83 | 8.44 | 6.68 |
| 14 | 100k | 8.50 | 8.22 | 6.13 |
| 14 | 1M | 8.62 | 8.27 | 6.08 |
| 14 | 10M | 8.72 | 8.26 | 6.05 |

### Capacity: 4 (`Cap-4`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 31.4 | 30.8 | 26.3 |
| 1 | 1M | 32.2 | 30.8 | 26.5 |
| 1 | 10M | 32.7 | 31.3 | 25.8 |
| 4 | 100k | 31.1 | 30.2 | 25.1 |
| 4 | 1M | 30.9 | 30.3 | 24.4 |
| 4 | 10M | 31.2 | 30.0 | 23.5 |
| 14 | 100k | 30.2 | 29.1 | 22.0 |
| 14 | 1M | 30.5 | 29.1 | 22.3 |
| 14 | 10M | 30.6 | 29.4 | 22.6 |

### Capacity: 128 (`Cap-128`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 125 | 205 | 219 |
| 1 | 1M | 127 | 211 | 225 |
| 1 | 10M | 122 | 212 | 223 |
| 4 | 100k | 114 | 201 | 217 |
| 4 | 1M | 117 | 210 | 221 |
| 4 | 10M | 117 | 210 | 222 |
| 14 | 100k | 108 | 194 | 208 |
| 14 | 1M | 116 | 209 | 220 |
| 14 | 10M | 118 | 204 | 217 |

**Notes.** At `Cap-128` throughput rises with batch size: 108-127 Melem/s at `Batch-8`, 194-212 at `Batch-64` and 208-225 at `Batch-512`, close to flat across 1, 4 and 14 producers. At `Cap-1` and `Cap-4`, `Batch-512` is 18-30% below `Batch-8`.
