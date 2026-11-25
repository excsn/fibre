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
| 1 | 1000 | 3.7058 ms / 3.7961 ms / 3.9825 ms | 251.10 Kelem/s / 263.43 Kelem/s / 269.84 Kelem/s |
| 1 | 100000 | 249.58 ms / 289.15 ms / 325.35 ms | 307.36 Kelem/s / 345.84 Kelem/s / 400.68 Kelem/s |
| 1 | 1000000 | 3.1533 s / 3.3356 s / 3.5111 s | 284.81 Kelem/s / 299.80 Kelem/s / 317.13 Kelem/s |
| 128 | 1000 | 64.389 µs / 64.719 µs / 64.973 µs | 15.391 Melem/s / 15.452 Melem/s / 15.531 Melem/s |
| 128 | 100000 | 3.3612 ms / 3.4356 ms / 3.5365 ms | 28.277 Melem/s / 29.107 Melem/s / 29.751 Melem/s |
| 128 | 1000000 | 35.153 ms / 35.566 ms / 36.431 ms | 27.449 Melem/s / 28.116 Melem/s / 28.447 Melem/s |
| 1024 | 1000 | 31.531 µs / 32.252 µs / 33.085 µs | 30.225 Melem/s / 31.006 Melem/s / 31.715 Melem/s |
| 1024 | 100000 | 2.3871 ms / 2.5287 ms / 2.6733 ms | 37.408 Melem/s / 39.546 Melem/s / 41.893 Melem/s |
| 1024 | 1000000 | 23.419 ms / 24.363 ms / 26.033 ms | 38.413 Melem/s / 41.045 Melem/s / 42.701 Melem/s |

## Batch Results (`SpscSyncBatch`)

`send_batch`/`recv_batch` with the same spawned producer/consumer harness. Axes:
capacity × total items × batch size.

`SpscSyncBatch/Cap-{Cap}_Items-{Items}_Batch-{Batch}`

| Cap | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1000 | 8 | 3.5741 ms / 3.7130 ms / 3.8613 ms | 258.98 Kelem/s / 269.32 Kelem/s / 279.79 Kelem/s |
| 1 | 1000 | 64 | 3.6048 ms / 3.7323 ms / 3.8993 ms | 256.46 Kelem/s / 267.93 Kelem/s / 277.41 Kelem/s |
| 1 | 1000 | 512 | 3.6633 ms / 3.7756 ms / 3.9241 ms | 254.84 Kelem/s / 264.86 Kelem/s / 272.98 Kelem/s |
| 1 | 100000 | 8 | 354.25 ms / 363.30 ms / 372.89 ms | 268.18 Kelem/s / 275.26 Kelem/s / 282.28 Kelem/s |
| 1 | 100000 | 64 | 353.75 ms / 364.55 ms / 378.36 ms | 264.30 Kelem/s / 274.31 Kelem/s / 282.69 Kelem/s |
| 1 | 100000 | 512 | 348.29 ms / 353.11 ms / 358.40 ms | 279.02 Kelem/s / 283.20 Kelem/s / 287.12 Kelem/s |
| 1 | 1000000 | 8 | 3.5906 s / 3.6372 s / 3.6840 s | 271.44 Kelem/s / 274.93 Kelem/s / 278.50 Kelem/s |
| 1 | 1000000 | 64 | 3.4799 s / 3.5658 s / 3.6289 s | 275.56 Kelem/s / 280.44 Kelem/s / 287.36 Kelem/s |
| 1 | 1000000 | 512 | 3.5973 s / 3.6417 s / 3.6896 s | 271.03 Kelem/s / 274.60 Kelem/s / 277.99 Kelem/s |
| 128 | 1000 | 8 | 64.042 µs / 64.407 µs / 65.025 µs | 15.379 Melem/s / 15.526 Melem/s / 15.615 Melem/s |
| 128 | 1000 | 64 | 62.731 µs / 63.488 µs / 64.129 µs | 15.593 Melem/s / 15.751 Melem/s / 15.941 Melem/s |
| 128 | 1000 | 512 | 63.189 µs / 64.055 µs / 65.096 µs | 15.362 Melem/s / 15.612 Melem/s / 15.825 Melem/s |
| 128 | 100000 | 8 | 3.3606 ms / 3.4051 ms / 3.4474 ms | 29.008 Melem/s / 29.368 Melem/s / 29.757 Melem/s |
| 128 | 100000 | 64 | 3.3730 ms / 3.4617 ms / 3.6613 ms | 27.313 Melem/s / 28.888 Melem/s / 29.647 Melem/s |
| 128 | 100000 | 512 | 4.0488 ms / 4.0952 ms / 4.1989 ms | 23.816 Melem/s / 24.419 Melem/s / 24.698 Melem/s |
| 128 | 1000000 | 8 | 34.941 ms / 35.520 ms / 36.838 ms | 27.146 Melem/s / 28.153 Melem/s / 28.619 Melem/s |
| 128 | 1000000 | 64 | 37.226 ms / 38.747 ms / 40.519 ms | 24.679 Melem/s / 25.809 Melem/s / 26.863 Melem/s |
| 128 | 1000000 | 512 | 40.001 ms / 40.120 ms / 40.270 ms | 24.833 Melem/s / 24.925 Melem/s / 24.999 Melem/s |
| 1024 | 1000 | 8 | 30.145 µs / 31.620 µs / 32.417 µs | 30.848 Melem/s / 31.626 Melem/s / 33.173 Melem/s |
| 1024 | 1000 | 64 | 27.923 µs / 28.856 µs / 29.667 µs | 33.707 Melem/s / 34.655 Melem/s / 35.813 Melem/s |
| 1024 | 1000 | 512 | 30.494 µs / 30.714 µs / 30.959 µs | 32.301 Melem/s / 32.558 Melem/s / 32.794 Melem/s |
| 1024 | 100000 | 8 | 2.0316 ms / 2.0543 ms / 2.0948 ms | 47.737 Melem/s / 48.678 Melem/s / 49.223 Melem/s |
| 1024 | 100000 | 64 | 281.69 µs / 287.13 µs / 294.10 µs | 340.02 Melem/s / 348.28 Melem/s / 355.00 Melem/s |
| 1024 | 100000 | 512 | 264.21 µs / 268.70 µs / 278.17 µs | 359.49 Melem/s / 372.16 Melem/s / 378.49 Melem/s |
| 1024 | 1000000 | 8 | 19.842 ms / 20.249 ms / 21.015 ms | 47.585 Melem/s / 49.384 Melem/s / 50.397 Melem/s |
| 1024 | 1000000 | 64 | 2.5191 ms / 2.5256 ms / 2.5342 ms | 394.59 Melem/s / 395.94 Melem/s / 396.97 Melem/s |
| 1024 | 1000000 | 512 | 2.2749 ms / 2.2877 ms / 2.3075 ms | 433.36 Melem/s / 437.12 Melem/s / 439.58 Melem/s |
