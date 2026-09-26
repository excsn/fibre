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
| 1 | 1000 | 92.492 µs / 93.569 µs / 94.528 µs | 10.579 Melem/s / 10.687 Melem/s / 10.812 Melem/s |
| 1 | 100000 | 8.3657 ms / 8.4329 ms / 8.4780 ms | 11.795 Melem/s / 11.858 Melem/s / 11.954 Melem/s |
| 1 | 1000000 | 83.637 ms / 84.073 ms / 84.492 ms | 11.835 Melem/s / 11.894 Melem/s / 11.956 Melem/s |
| 128 | 1000 | 14.196 µs / 14.386 µs / 14.701 µs | 68.024 Melem/s / 69.510 Melem/s / 70.441 Melem/s |
| 128 | 100000 | 719.88 µs / 724.16 µs / 729.40 µs | 137.10 Melem/s / 138.09 Melem/s / 138.91 Melem/s |
| 128 | 1000000 | 7.1723 ms / 7.2071 ms / 7.2242 ms | 138.42 Melem/s / 138.75 Melem/s / 139.42 Melem/s |
| 1024 | 1000 | 12.880 µs / 13.071 µs / 13.191 µs | 75.812 Melem/s / 76.507 Melem/s / 77.642 Melem/s |
| 1024 | 100000 | 551.22 µs / 554.16 µs / 558.75 µs | 178.97 Melem/s / 180.45 Melem/s / 181.42 Melem/s |
| 1024 | 1000000 | 5.3825 ms / 5.4567 ms / 5.5467 ms | 180.29 Melem/s / 183.26 Melem/s / 185.79 Melem/s |

## Batch Results (`SpscAsyncBatch`)

`send_batch`/`recv_batch` with the same spawned producer/consumer task harness. Axes:
capacity × total items × batch size.

`SpscAsyncBatch/Cap-{Cap}_Items-{Items}_Batch-{Batch}`

| Cap | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1000 | 8 | 147.08 µs / 147.78 µs / 148.34 µs | 6.7415 Melem/s / 6.7669 Melem/s / 6.7990 Melem/s |
| 1 | 1000 | 64 | 153.19 µs / 155.52 µs / 158.27 µs | 6.3184 Melem/s / 6.4300 Melem/s / 6.5278 Melem/s |
| 1 | 1000 | 512 | 143.76 µs / 144.89 µs / 147.11 µs | 6.7977 Melem/s / 6.9018 Melem/s / 6.9562 Melem/s |
| 1 | 100000 | 8 | 14.014 ms / 14.078 ms / 14.120 ms | 7.0821 Melem/s / 7.1031 Melem/s / 7.1358 Melem/s |
| 1 | 100000 | 64 | 14.242 ms / 14.489 ms / 14.759 ms | 6.7754 Melem/s / 6.9016 Melem/s / 7.0215 Melem/s |
| 1 | 100000 | 512 | 13.619 ms / 13.789 ms / 13.944 ms | 7.1717 Melem/s / 7.2522 Melem/s / 7.3426 Melem/s |
| 1 | 1000000 | 8 | 140.13 ms / 141.09 ms / 142.31 ms | 7.0270 Melem/s / 7.0877 Melem/s / 7.1361 Melem/s |
| 1 | 1000000 | 64 | 144.04 ms / 145.55 ms / 147.41 ms | 6.7839 Melem/s / 6.8707 Melem/s / 6.9427 Melem/s |
| 1 | 1000000 | 512 | 137.09 ms / 137.88 ms / 138.79 ms | 7.2053 Melem/s / 7.2525 Melem/s / 7.2942 Melem/s |
| 128 | 1000 | 8 | 17.585 µs / 17.754 µs / 17.931 µs | 55.768 Melem/s / 56.326 Melem/s / 56.866 Melem/s |
| 128 | 1000 | 64 | 16.655 µs / 16.718 µs / 16.846 µs | 59.363 Melem/s / 59.817 Melem/s / 60.041 Melem/s |
| 128 | 1000 | 512 | 15.373 µs / 15.475 µs / 15.550 µs | 64.310 Melem/s / 64.619 Melem/s / 65.051 Melem/s |
| 128 | 100000 | 8 | 1.0838 ms / 1.1019 ms / 1.1188 ms | 89.383 Melem/s / 90.750 Melem/s / 92.267 Melem/s |
| 128 | 100000 | 64 | 961.72 µs / 974.44 µs / 990.08 µs | 101.00 Melem/s / 102.62 Melem/s / 103.98 Melem/s |
| 128 | 100000 | 512 | 822.06 µs / 838.55 µs / 854.80 µs | 116.99 Melem/s / 119.25 Melem/s / 121.65 Melem/s |
| 128 | 1000000 | 8 | 10.793 ms / 10.839 ms / 10.883 ms | 91.889 Melem/s / 92.259 Melem/s / 92.651 Melem/s |
| 128 | 1000000 | 64 | 9.5699 ms / 9.6855 ms / 9.7890 ms | 102.16 Melem/s / 103.25 Melem/s / 104.49 Melem/s |
| 128 | 1000000 | 512 | 8.1785 ms / 8.2036 ms / 8.2527 ms | 121.17 Melem/s / 121.90 Melem/s / 122.27 Melem/s |
| 1024 | 1000 | 8 | 15.776 µs / 15.868 µs / 16.013 µs | 62.448 Melem/s / 63.021 Melem/s / 63.387 Melem/s |
| 1024 | 1000 | 64 | 14.765 µs / 14.983 µs / 15.162 µs | 65.956 Melem/s / 66.743 Melem/s / 67.726 Melem/s |
| 1024 | 1000 | 512 | 14.868 µs / 14.995 µs / 15.256 µs | 65.549 Melem/s / 66.690 Melem/s / 67.258 Melem/s |
| 1024 | 100000 | 8 | 824.52 µs / 848.42 µs / 869.90 µs | 114.96 Melem/s / 117.87 Melem/s / 121.28 Melem/s |
| 1024 | 100000 | 64 | 707.09 µs / 707.75 µs / 708.42 µs | 141.16 Melem/s / 141.29 Melem/s / 141.43 Melem/s |
| 1024 | 100000 | 512 | 682.91 µs / 685.60 µs / 688.70 µs | 145.20 Melem/s / 145.86 Melem/s / 146.43 Melem/s |
| 1024 | 1000000 | 8 | 8.6637 ms / 8.7154 ms / 8.7576 ms | 114.19 Melem/s / 114.74 Melem/s / 115.42 Melem/s |
| 1024 | 1000000 | 64 | 6.6755 ms / 6.7225 ms / 6.8316 ms | 146.38 Melem/s / 148.76 Melem/s / 149.80 Melem/s |
| 1024 | 1000000 | 512 | 6.5907 ms / 6.8512 ms / 6.9695 ms | 143.48 Melem/s / 145.96 Melem/s / 151.73 Melem/s |
