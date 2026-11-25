# Fibre Benchmark: `MpmcV2 Sync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpmcUnboundedSync`)

`MpmcUnboundedSync/Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 666.39 µs / 673.92 µs / 683.84 µs | 146.23 Melem/s / 148.39 Melem/s / 150.06 Melem/s |
| 1 | 1 | 1000000 | 6.2071 ms / 6.2153 ms / 6.2224 ms | 160.71 Melem/s / 160.89 Melem/s / 161.11 Melem/s |
| 1 | 4 | 100000 | 2.4431 ms / 2.5226 ms / 2.6696 ms | 37.459 Melem/s / 39.642 Melem/s / 40.932 Melem/s |
| 1 | 4 | 1000000 | 25.598 ms / 26.048 ms / 26.688 ms | 37.469 Melem/s / 38.390 Melem/s / 39.065 Melem/s |
| 1 | 14 | 100000 | 8.4606 ms / 9.6196 ms / 11.561 ms | 8.6498 Melem/s / 10.395 Melem/s / 11.819 Melem/s |
| 1 | 14 | 1000000 | 319.81 ms / 496.85 ms / 687.78 ms | 1.4540 Melem/s / 2.0127 Melem/s / 3.1268 Melem/s |
| 4 | 1 | 100000 | 8.3375 ms / 8.5696 ms / 8.9637 ms | 11.156 Melem/s / 11.669 Melem/s / 11.994 Melem/s |
| 4 | 1 | 1000000 | 52.906 ms / 55.474 ms / 57.782 ms | 17.306 Melem/s / 18.027 Melem/s / 18.901 Melem/s |
| 4 | 4 | 100000 | 11.744 ms / 11.773 ms / 11.803 ms | 8.4727 Melem/s / 8.4942 Melem/s / 8.5148 Melem/s |
| 4 | 4 | 1000000 | 114.27 ms / 116.45 ms / 117.87 ms | 8.4838 Melem/s / 8.5876 Melem/s / 8.7513 Melem/s |
| 4 | 14 | 100000 | 11.428 ms / 11.570 ms / 11.691 ms | 8.5533 Melem/s / 8.6431 Melem/s / 8.7508 Melem/s |
| 4 | 14 | 1000000 | 112.56 ms / 116.11 ms / 118.51 ms | 8.4378 Melem/s / 8.6124 Melem/s / 8.8845 Melem/s |
| 14 | 1 | 100000 | 7.9371 ms / 7.9922 ms / 8.0266 ms | 12.459 Melem/s / 12.512 Melem/s / 12.599 Melem/s |
| 14 | 1 | 1000000 | 83.885 ms / 84.115 ms / 84.274 ms | 11.866 Melem/s / 11.888 Melem/s / 11.921 Melem/s |
| 14 | 4 | 100000 | 10.356 ms / 10.398 ms / 10.437 ms | 9.5810 Melem/s / 9.6171 Melem/s / 9.6558 Melem/s |
| 14 | 4 | 1000000 | 106.36 ms / 106.68 ms / 107.15 ms | 9.3330 Melem/s / 9.3735 Melem/s / 9.4023 Melem/s |
| 14 | 14 | 100000 | 11.709 ms / 11.905 ms / 12.065 ms | 8.2883 Melem/s / 8.3995 Melem/s / 8.5405 Melem/s |
| 14 | 14 | 1000000 | 125.04 ms / 128.59 ms / 131.64 ms | 7.5964 Melem/s / 7.7766 Melem/s / 7.9972 Melem/s |

---

## Bounded Results (`MpmcSync`)

### Capacity: 4 (`Cap-4`)

`MpmcSync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 1 | 100000 | 12.476 ms / 12.663 ms / 12.959 ms | 7.7169 Melem/s / 7.8968 Melem/s / 8.0152 Melem/s |
| 4 | 1 | 1 | 1000000 | 121.01 ms / 123.52 ms / 128.89 ms | 7.7583 Melem/s / 8.0961 Melem/s / 8.2639 Melem/s |
| 4 | 1 | 4 | 100000 | 35.154 ms / 35.914 ms / 36.882 ms | 2.7113 Melem/s / 2.7844 Melem/s / 2.8446 Melem/s |
| 4 | 1 | 4 | 1000000 | 323.12 ms / 329.86 ms / 339.04 ms | 2.9495 Melem/s / 3.0316 Melem/s / 3.0948 Melem/s |
| 4 | 1 | 14 | 100000 | 160.05 ms / 163.29 ms / 167.12 ms | 598.38 Kelem/s / 612.39 Kelem/s / 624.79 Kelem/s |
| 4 | 1 | 14 | 1000000 | 1.6168 s / 1.6297 s / 1.6450 s | 607.91 Kelem/s / 613.62 Kelem/s / 618.51 Kelem/s |
| 4 | 4 | 1 | 100000 | 30.419 ms / 31.625 ms / 33.556 ms | 2.9801 Melem/s / 3.1620 Melem/s / 3.2875 Melem/s |
| 4 | 4 | 1 | 1000000 | 307.49 ms / 318.43 ms / 335.04 ms | 2.9847 Melem/s / 3.1405 Melem/s / 3.2522 Melem/s |
| 4 | 4 | 4 | 100000 | 69.063 ms / 69.605 ms / 70.361 ms | 1.4212 Melem/s / 1.4367 Melem/s / 1.4480 Melem/s |
| 4 | 4 | 4 | 1000000 | 688.37 ms / 691.90 ms / 694.79 ms | 1.4393 Melem/s / 1.4453 Melem/s / 1.4527 Melem/s |
| 4 | 4 | 14 | 100000 | 109.94 ms / 111.71 ms / 113.83 ms | 878.48 Kelem/s / 895.18 Kelem/s / 909.61 Kelem/s |
| 4 | 4 | 14 | 1000000 | 1.0763 s / 1.0933 s / 1.1103 s | 900.65 Kelem/s / 914.64 Kelem/s / 929.07 Kelem/s |
| 4 | 14 | 1 | 100000 | 159.08 ms / 161.24 ms / 163.76 ms | 610.65 Kelem/s / 620.21 Kelem/s / 628.63 Kelem/s |
| 4 | 14 | 1 | 1000000 | 1.5869 s / 1.6037 s / 1.6196 s | 617.43 Kelem/s / 623.55 Kelem/s / 630.17 Kelem/s |
| 4 | 14 | 4 | 100000 | 100.14 ms / 101.81 ms / 103.34 ms | 967.68 Kelem/s / 982.20 Kelem/s / 998.64 Kelem/s |
| 4 | 14 | 4 | 1000000 | 1.0397 s / 1.0613 s / 1.0797 s | 926.15 Kelem/s / 942.28 Kelem/s / 961.80 Kelem/s |
| 4 | 14 | 14 | 100000 | 87.896 ms / 88.611 ms / 89.292 ms | 1.1199 Melem/s / 1.1285 Melem/s / 1.1377 Melem/s |
| 4 | 14 | 14 | 1000000 | 778.80 ms / 831.15 ms / 862.83 ms | 1.1590 Melem/s / 1.2032 Melem/s / 1.2840 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpmcSync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 1 | 100000 | 2.3800 ms / 2.3868 ms / 2.3922 ms | 41.803 Melem/s / 41.897 Melem/s / 42.016 Melem/s |
| 128 | 1 | 1 | 1000000 | 23.699 ms / 23.858 ms / 24.011 ms | 41.648 Melem/s / 41.914 Melem/s / 42.195 Melem/s |
| 128 | 1 | 4 | 100000 | 7.3781 ms / 7.5580 ms / 7.7109 ms | 12.969 Melem/s / 13.231 Melem/s / 13.554 Melem/s |
| 128 | 1 | 4 | 1000000 | 73.875 ms / 74.626 ms / 75.544 ms | 13.237 Melem/s / 13.400 Melem/s / 13.536 Melem/s |
| 128 | 1 | 14 | 100000 | 9.5143 ms / 9.8087 ms / 10.040 ms | 9.9599 Melem/s / 10.195 Melem/s / 10.510 Melem/s |
| 128 | 1 | 14 | 1000000 | 92.430 ms / 95.818 ms / 99.170 ms | 10.084 Melem/s / 10.436 Melem/s / 10.819 Melem/s |
| 128 | 4 | 1 | 100000 | 7.5032 ms / 7.5689 ms / 7.6414 ms | 13.087 Melem/s / 13.212 Melem/s / 13.328 Melem/s |
| 128 | 4 | 1 | 1000000 | 78.424 ms / 80.706 ms / 82.828 ms | 12.073 Melem/s / 12.391 Melem/s / 12.751 Melem/s |
| 128 | 4 | 4 | 100000 | 18.016 ms / 18.475 ms / 18.846 ms | 5.3061 Melem/s / 5.4126 Melem/s / 5.5507 Melem/s |
| 128 | 4 | 4 | 1000000 | 176.73 ms / 183.44 ms / 188.34 ms | 5.3096 Melem/s / 5.4515 Melem/s / 5.6583 Melem/s |
| 128 | 4 | 14 | 100000 | 6.4838 ms / 6.6122 ms / 6.8170 ms | 14.669 Melem/s / 15.124 Melem/s / 15.423 Melem/s |
| 128 | 4 | 14 | 1000000 | 58.177 ms / 60.067 ms / 62.815 ms | 15.920 Melem/s / 16.648 Melem/s / 17.189 Melem/s |
| 128 | 14 | 1 | 100000 | 10.791 ms / 10.964 ms / 11.181 ms | 8.9434 Melem/s / 9.1210 Melem/s / 9.2671 Melem/s |
| 128 | 14 | 1 | 1000000 | 99.085 ms / 101.65 ms / 104.36 ms | 9.5824 Melem/s / 9.8375 Melem/s / 10.092 Melem/s |
| 128 | 14 | 4 | 100000 | 7.3864 ms / 7.5203 ms / 7.6672 ms | 13.043 Melem/s / 13.297 Melem/s / 13.538 Melem/s |
| 128 | 14 | 4 | 1000000 | 65.394 ms / 66.230 ms / 67.704 ms | 14.770 Melem/s / 15.099 Melem/s / 15.292 Melem/s |
| 128 | 14 | 14 | 100000 | 5.6907 ms / 5.7436 ms / 5.8411 ms | 17.120 Melem/s / 17.411 Melem/s / 17.573 Melem/s |
| 128 | 14 | 14 | 1000000 | 48.362 ms / 49.431 ms / 51.384 ms | 19.461 Melem/s / 20.230 Melem/s / 20.677 Melem/s |
