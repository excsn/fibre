# Fibre Benchmark: `SpmcAsync`
**Test Machine:** MacBook M4 Pro

## Results

`SpmcAsync/Cons-{Cons}_Cap-{Cap}_Items-{Items}`

| Cons | Cap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 751.15 ms / 753.79 ms / 756.52 ms | 132.18 Kelem/s / 132.66 Kelem/s / 133.13 Kelem/s |
| 1 | 128 | 100000 | 6.3257 ms / 6.3438 ms / 6.3630 ms | 15.716 Melem/s / 15.763 Melem/s / 15.809 Melem/s |
| 4 | 1 | 100000 | 812.75 ms / 824.30 ms / 839.68 ms | 476.37 Kelem/s / 485.26 Kelem/s / 492.16 Kelem/s |
| 4 | 128 | 100000 | 20.530 ms / 20.770 ms / 21.184 ms | 18.882 Melem/s / 19.259 Melem/s / 19.484 Melem/s |
| 14 | 1 | 100000 | 776.80 ms / 787.93 ms / 805.04 ms | 1.7390 Melem/s / 1.7768 Melem/s / 1.8023 Melem/s |
| 14 | 128 | 100000 | 125.41 ms / 125.86 ms / 126.40 ms | 11.076 Melem/s / 11.124 Melem/s / 11.164 Melem/s |
