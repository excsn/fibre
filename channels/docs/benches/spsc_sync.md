# Fibre Benchmark: `SpscSync`
**Test Machine:** MacBook M4 Pro

Measured with the real-concurrency harness: a spawned producer thread and a spawned
consumer thread per iteration (`benches/spsc_bounded.rs`). Numbers are not comparable
to older revisions of this file, which measured a same-thread send/recv loop with no
cross-thread synchronization.

## Results

`SpscSync/Cap-{Cap}_Items-{Items}`

| Cap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 1000 | 3.9490 ms / 3.9984 ms / 4.0953 ms | 244.18 Kelem/s / 250.10 Kelem/s / 253.23 Kelem/s |
| 1 | 100000 | 264.34 ms / 315.91 ms / 363.73 ms | 274.93 Kelem/s / 316.54 Kelem/s / 378.30 Kelem/s |
| 1 | 1000000 | 3.3348 s / 3.5054 s / 3.6700 s | 272.48 Kelem/s / 285.28 Kelem/s / 299.87 Kelem/s |
| 128 | 1000 | 65.912 µs / 67.380 µs / 69.718 µs | 14.344 Melem/s / 14.841 Melem/s / 15.172 Melem/s |
| 128 | 100000 | 3.8093 ms / 3.8405 ms / 3.8627 ms | 25.889 Melem/s / 26.039 Melem/s / 26.252 Melem/s |
| 128 | 1000000 | 36.526 ms / 37.015 ms / 37.578 ms | 26.611 Melem/s / 27.016 Melem/s / 27.378 Melem/s |
| 1024 | 1000 | 32.443 µs / 33.411 µs / 34.654 µs | 28.857 Melem/s / 29.930 Melem/s / 30.823 Melem/s |
| 1024 | 100000 | 2.2816 ms / 2.3488 ms / 2.3970 ms | 41.719 Melem/s / 42.576 Melem/s / 43.830 Melem/s |
| 1024 | 1000000 | 22.069 ms / 22.496 ms / 23.311 ms | 42.898 Melem/s / 44.453 Melem/s / 45.313 Melem/s |

## Batch Results (`SpscSyncBatch`)

`send_batch`/`recv_batch` with the same spawned producer/consumer harness. Axes:
capacity × total items × batch size.

`SpscSyncBatch/Cap-{Cap}_Items-{Items}_Batch-{Batch}`

| Cap | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1000 | 8 | 4.2135 ms / 4.2592 ms / 4.3070 ms | 232.18 Kelem/s / 234.78 Kelem/s / 237.33 Kelem/s |
| 1 | 1000 | 64 | 4.2476 ms / 4.2757 ms / 4.3073 ms | 232.17 Kelem/s / 233.88 Kelem/s / 235.43 Kelem/s |
| 1 | 1000 | 512 | 4.2345 ms / 4.2705 ms / 4.3079 ms | 232.13 Kelem/s / 234.16 Kelem/s / 236.16 Kelem/s |
| 1 | 100000 | 8 | 409.72 ms / 421.18 ms / 433.07 ms | 230.91 Kelem/s / 237.43 Kelem/s / 244.07 Kelem/s |
| 1 | 100000 | 64 | 427.30 ms / 435.97 ms / 445.42 ms | 224.51 Kelem/s / 229.37 Kelem/s / 234.03 Kelem/s |
| 1 | 100000 | 512 | 424.81 ms / 426.63 ms / 428.36 ms | 233.45 Kelem/s / 234.40 Kelem/s / 235.40 Kelem/s |
| 1 | 1000000 | 8 | 4.1052 s / 4.1238 s / 4.1414 s | 241.46 Kelem/s / 242.50 Kelem/s / 243.59 Kelem/s |
| 1 | 1000000 | 64 | 4.1928 s / 4.2145 s / 4.2379 s | 235.97 Kelem/s / 237.28 Kelem/s / 238.51 Kelem/s |
| 1 | 1000000 | 512 | 4.2372 s / 4.2849 s / 4.3478 s | 230.00 Kelem/s / 233.38 Kelem/s / 236.01 Kelem/s |
| 128 | 1000 | 8 | 58.470 µs / 59.268 µs / 59.774 µs | 16.730 Melem/s / 16.873 Melem/s / 17.103 Melem/s |
| 128 | 1000 | 64 | 60.444 µs / 60.836 µs / 61.172 µs | 16.347 Melem/s / 16.438 Melem/s / 16.544 Melem/s |
| 128 | 1000 | 512 | 60.118 µs / 60.390 µs / 60.803 µs | 16.447 Melem/s / 16.559 Melem/s / 16.634 Melem/s |
| 128 | 100000 | 8 | 3.6098 ms / 3.6568 ms / 3.7155 ms | 26.914 Melem/s / 27.346 Melem/s / 27.702 Melem/s |
| 128 | 100000 | 64 | 3.7375 ms / 3.8391 ms / 3.9487 ms | 25.325 Melem/s / 26.048 Melem/s / 26.756 Melem/s |
| 128 | 100000 | 512 | 4.1180 ms / 4.1473 ms / 4.1703 ms | 23.979 Melem/s / 24.112 Melem/s / 24.284 Melem/s |
| 128 | 1000000 | 8 | 34.625 ms / 35.617 ms / 36.211 ms | 27.616 Melem/s / 28.077 Melem/s / 28.881 Melem/s |
| 128 | 1000000 | 64 | 34.610 ms / 35.698 ms / 36.819 ms | 27.160 Melem/s / 28.013 Melem/s / 28.894 Melem/s |
| 128 | 1000000 | 512 | 40.594 ms / 40.959 ms / 41.172 ms | 24.289 Melem/s / 24.415 Melem/s / 24.634 Melem/s |
| 1024 | 1000 | 8 | 31.935 µs / 32.588 µs / 33.173 µs | 30.145 Melem/s / 30.687 Melem/s / 31.314 Melem/s |
| 1024 | 1000 | 64 | 30.610 µs / 30.973 µs / 31.453 µs | 31.793 Melem/s / 32.286 Melem/s / 32.669 Melem/s |
| 1024 | 1000 | 512 | 29.052 µs / 30.010 µs / 31.133 µs | 32.120 Melem/s / 33.323 Melem/s / 34.422 Melem/s |
| 1024 | 100000 | 8 | 673.89 µs / 693.39 µs / 715.80 µs | 139.70 Melem/s / 144.22 Melem/s / 148.39 Melem/s |
| 1024 | 100000 | 64 | 661.33 µs / 693.14 µs / 733.48 µs | 136.34 Melem/s / 144.27 Melem/s / 151.21 Melem/s |
| 1024 | 100000 | 512 | 533.60 µs / 540.68 µs / 545.85 µs | 183.20 Melem/s / 184.95 Melem/s / 187.41 Melem/s |
| 1024 | 1000000 | 8 | 6.5033 ms / 6.5859 ms / 6.6786 ms | 149.73 Melem/s / 151.84 Melem/s / 153.77 Melem/s |
| 1024 | 1000000 | 64 | 5.6793 ms / 5.7377 ms / 5.8250 ms | 171.67 Melem/s / 174.29 Melem/s / 176.08 Melem/s |
| 1024 | 1000000 | 512 | 5.3594 ms / 5.5660 ms / 5.9541 ms | 167.95 Melem/s / 179.66 Melem/s / 186.59 Melem/s |
