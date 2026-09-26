# Fibre Benchmark: `SpmcAsync`
**Test Machine:** MacBook M4 Pro

## Results

`SpmcAsync/Cons-{Cons}_Cap-{Cap}_Items-{Items}`

| Cons | Cap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 778.37 ms / 780.61 ms / 782.75 ms | 127.75 Kelem/s / 128.10 Kelem/s / 128.47 Kelem/s |
| 1 | 128 | 100000 | 6.6843 ms / 6.7435 ms / 6.7953 ms | 14.716 Melem/s / 14.829 Melem/s / 14.960 Melem/s |
| 4 | 1 | 100000 | 785.62 ms / 787.89 ms / 790.17 ms | 506.22 Kelem/s / 507.68 Kelem/s / 509.15 Kelem/s |
| 4 | 128 | 100000 | 20.325 ms / 20.461 ms / 20.623 ms | 19.396 Melem/s / 19.549 Melem/s / 19.680 Melem/s |
| 14 | 1 | 100000 | 805.24 ms / 807.45 ms / 809.61 ms | 1.7292 Melem/s / 1.7338 Melem/s / 1.7386 Melem/s |
| 14 | 128 | 100000 | 119.32 ms / 119.85 ms / 120.38 ms | 11.630 Melem/s / 11.682 Melem/s / 11.733 Melem/s |
