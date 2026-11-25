# Fibre Benchmark: `SpscAsync`
**Test Machine:** MacBook M4 Pro

Measured with the real-concurrency harness: a spawned producer task and a spawned
consumer task per iteration on a multi-thread Tokio runtime (`benches/spsc_bounded.rs`).
Numbers are not comparable to older revisions of this file, which measured a
same-thread send/recv loop with no cross-task synchronization.

## Results

`SpscAsync/Cap-{Cap}_Items-{Items}`

| Cap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 1000 | 83.162 µs / 83.847 µs / 85.733 µs | 11.664 Melem/s / 11.926 Melem/s / 12.025 Melem/s |
| 1 | 100000 | 7.5117 ms / 7.5937 ms / 7.8131 ms | 12.799 Melem/s / 13.169 Melem/s / 13.313 Melem/s |
| 1 | 1000000 | 73.544 ms / 73.995 ms / 74.525 ms | 13.418 Melem/s / 13.514 Melem/s / 13.597 Melem/s |
| 128 | 1000 | 13.454 µs / 13.550 µs / 13.670 µs | 73.154 Melem/s / 73.801 Melem/s / 74.326 Melem/s |
| 128 | 100000 | 693.53 µs / 702.01 µs / 705.77 µs | 141.69 Melem/s / 142.45 Melem/s / 144.19 Melem/s |
| 128 | 1000000 | 6.8060 ms / 6.8566 ms / 6.9918 ms | 143.02 Melem/s / 145.85 Melem/s / 146.93 Melem/s |
| 1024 | 1000 | 11.643 µs / 11.849 µs / 12.219 µs | 81.841 Melem/s / 84.395 Melem/s / 85.892 Melem/s |
| 1024 | 100000 | 543.94 µs / 552.10 µs / 573.63 µs | 174.33 Melem/s / 181.13 Melem/s / 183.84 Melem/s |
| 1024 | 1000000 | 5.3825 ms / 5.4222 ms / 5.5293 ms | 180.86 Melem/s / 184.43 Melem/s / 185.79 Melem/s |

## Batch Results (`SpscAsyncBatch`)

`send_batch`/`recv_batch` with the same spawned producer/consumer task harness. Axes:
capacity × total items × batch size.

`SpscAsyncBatch/Cap-{Cap}_Items-{Items}_Batch-{Batch}`

| Cap | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1000 | 8 | 100.60 µs / 101.60 µs / 103.79 µs | 9.6352 Melem/s / 9.8427 Melem/s / 9.9400 Melem/s |
| 1 | 1000 | 64 | 103.47 µs / 104.99 µs / 106.95 µs | 9.3504 Melem/s / 9.5252 Melem/s / 9.6649 Melem/s |
| 1 | 1000 | 512 | 125.56 µs / 127.52 µs / 130.65 µs | 7.6543 Melem/s / 7.8419 Melem/s / 7.9641 Melem/s |
| 1 | 100000 | 8 | 8.7963 ms / 8.8741 ms / 8.9862 ms | 11.128 Melem/s / 11.269 Melem/s / 11.368 Melem/s |
| 1 | 100000 | 64 | 9.1794 ms / 9.2274 ms / 9.2931 ms | 10.761 Melem/s / 10.837 Melem/s / 10.894 Melem/s |
| 1 | 100000 | 512 | 11.692 ms / 11.725 ms / 11.748 ms | 8.5119 Melem/s / 8.5290 Melem/s / 8.5531 Melem/s |
| 1 | 1000000 | 8 | 89.197 ms / 89.550 ms / 89.942 ms | 11.118 Melem/s / 11.167 Melem/s / 11.211 Melem/s |
| 1 | 1000000 | 64 | 94.762 ms / 95.549 ms / 96.255 ms | 10.389 Melem/s / 10.466 Melem/s / 10.553 Melem/s |
| 1 | 1000000 | 512 | 117.41 ms / 117.93 ms / 118.33 ms | 8.4506 Melem/s / 8.4798 Melem/s / 8.5172 Melem/s |
| 128 | 1000 | 8 | 13.457 µs / 13.494 µs / 13.529 µs | 73.913 Melem/s / 74.108 Melem/s / 74.312 Melem/s |
| 128 | 1000 | 64 | 12.684 µs / 12.859 µs / 13.292 µs | 75.236 Melem/s / 77.766 Melem/s / 78.842 Melem/s |
| 128 | 1000 | 512 | 12.748 µs / 12.765 µs / 12.793 µs | 78.166 Melem/s / 78.341 Melem/s / 78.446 Melem/s |
| 128 | 100000 | 8 | 681.99 µs / 684.08 µs / 687.26 µs | 145.51 Melem/s / 146.18 Melem/s / 146.63 Melem/s |
| 128 | 100000 | 64 | 588.71 µs / 590.57 µs / 592.77 µs | 168.70 Melem/s / 169.33 Melem/s / 169.86 Melem/s |
| 128 | 100000 | 512 | 574.98 µs / 575.72 µs / 577.24 µs | 173.24 Melem/s / 173.70 Melem/s / 173.92 Melem/s |
| 128 | 1000000 | 8 | 6.8227 ms / 6.8414 ms / 6.8841 ms | 145.26 Melem/s / 146.17 Melem/s / 146.57 Melem/s |
| 128 | 1000000 | 64 | 5.7677 ms / 5.7787 ms / 5.7943 ms | 172.58 Melem/s / 173.05 Melem/s / 173.38 Melem/s |
| 128 | 1000000 | 512 | 5.6948 ms / 5.6976 ms / 5.7035 ms | 175.33 Melem/s / 175.51 Melem/s / 175.60 Melem/s |
| 1024 | 1000 | 8 | 12.347 µs / 12.420 µs / 12.486 µs | 80.088 Melem/s / 80.515 Melem/s / 80.989 Melem/s |
| 1024 | 1000 | 64 | 11.747 µs / 11.851 µs / 12.107 µs | 82.596 Melem/s / 84.380 Melem/s / 85.126 Melem/s |
| 1024 | 1000 | 512 | 11.972 µs / 12.248 µs / 12.697 µs | 78.761 Melem/s / 81.646 Melem/s / 83.528 Melem/s |
| 1024 | 100000 | 8 | 498.54 µs / 504.32 µs / 512.49 µs | 195.12 Melem/s / 198.28 Melem/s / 200.59 Melem/s |
| 1024 | 100000 | 64 | 477.95 µs / 484.41 µs / 493.71 µs | 202.55 Melem/s / 206.44 Melem/s / 209.23 Melem/s |
| 1024 | 100000 | 512 | 470.74 µs / 472.04 µs / 473.22 µs | 211.32 Melem/s / 211.84 Melem/s / 212.43 Melem/s |
| 1024 | 1000000 | 8 | 4.9278 ms / 4.9764 ms / 5.0421 ms | 198.33 Melem/s / 200.95 Melem/s / 202.93 Melem/s |
| 1024 | 1000000 | 64 | 4.6741 ms / 4.7095 ms / 4.7419 ms | 210.89 Melem/s / 212.34 Melem/s / 213.94 Melem/s |
| 1024 | 1000000 | 512 | 4.7429 ms / 4.7703 ms / 4.7977 ms | 208.43 Melem/s / 209.63 Melem/s / 210.84 Melem/s |
