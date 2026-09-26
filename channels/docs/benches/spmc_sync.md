# Fibre Benchmark: `SpmcSync`
**Test Machine:** MacBook M4 Pro

## Results

`SpmcSync/Cons-{Cons}_Cap-{Cap}_Items-{Items}`

| Cons | Cap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 420.68 ms / 428.05 ms / 434.91 ms | 229.93 Kelem/s / 233.62 Kelem/s / 237.71 Kelem/s |
| 1 | 128 | 100000 | 6.6143 ms / 6.6674 ms / 6.7239 ms | 14.872 Melem/s / 14.998 Melem/s / 15.119 Melem/s |
| 4 | 1 | 100000 | 857.51 ms / 863.74 ms / 869.70 ms | 459.93 Kelem/s / 463.10 Kelem/s / 466.46 Kelem/s |
| 4 | 128 | 100000 | 25.095 ms / 25.512 ms / 25.976 ms | 15.399 Melem/s / 15.679 Melem/s / 15.939 Melem/s |
| 14 | 1 | 100000 | 2.8141 s / 2.8234 s / 2.8337 s | 494.05 Kelem/s / 495.86 Kelem/s / 497.50 Kelem/s |
| 14 | 128 | 100000 | 4.4640 s / 4.6387 s / 4.8091 s | 291.12 Kelem/s / 301.81 Kelem/s / 313.62 Kelem/s |
