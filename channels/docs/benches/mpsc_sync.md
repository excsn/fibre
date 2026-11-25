# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

`MpscUnboundedSync/Prod-{Prod}_Items-{Items}`

| Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 100000 | 497.16 µs / 505.67 µs / 528.21 µs | 189.32 Melem/s / 197.76 Melem/s / 201.14 Melem/s |
| 1 | 1000000 | 4.6927 ms / 4.7764 ms / 4.9184 ms | 203.32 Melem/s / 209.36 Melem/s / 213.10 Melem/s |
| 1 | 10000000 | 46.062 ms / 47.046 ms / 48.831 ms | 204.79 Melem/s / 212.56 Melem/s / 217.10 Melem/s |
| 4 | 100000 | 3.7624 ms / 3.8787 ms / 3.9738 ms | 25.165 Melem/s / 25.782 Melem/s / 26.579 Melem/s |
| 4 | 1000000 | 40.328 ms / 40.930 ms / 42.113 ms | 23.746 Melem/s / 24.432 Melem/s / 24.797 Melem/s |
| 4 | 10000000 | 402.32 ms / 409.43 ms / 416.58 ms | 24.005 Melem/s / 24.424 Melem/s / 24.856 Melem/s |
| 14 | 100000 | 7.1017 ms / 7.1119 ms / 7.1200 ms | 14.045 Melem/s / 14.061 Melem/s / 14.081 Melem/s |
| 14 | 1000000 | 76.111 ms / 76.454 ms / 76.944 ms | 12.996 Melem/s / 13.080 Melem/s / 13.139 Melem/s |
| 14 | 10000000 | 737.68 ms / 755.37 ms / 767.00 ms | 13.038 Melem/s / 13.239 Melem/s / 13.556 Melem/s |

## Bounded Results (`MpscBoundedSync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 14.918 ms / 15.732 ms / 16.124 ms | 6.2021 Melem/s / 6.3565 Melem/s / 6.7032 Melem/s |
| 1 | 1 | 1000000 | 145.99 ms / 149.34 ms / 153.22 ms | 6.5264 Melem/s / 6.6959 Melem/s / 6.8496 Melem/s |
| 1 | 1 | 10000000 | 1.4514 s / 1.4750 s / 1.5044 s | 6.6474 Melem/s / 6.7795 Melem/s / 6.8901 Melem/s |
| 1 | 4 | 100000 | 35.837 ms / 36.815 ms / 38.647 ms | 2.5875 Melem/s / 2.7163 Melem/s / 2.7904 Melem/s |
| 1 | 4 | 1000000 | 371.04 ms / 387.74 ms / 406.84 ms | 2.4580 Melem/s / 2.5791 Melem/s / 2.6951 Melem/s |
| 1 | 4 | 10000000 | 3.8069 s / 3.8553 s / 3.9083 s | 2.5586 Melem/s / 2.5938 Melem/s / 2.6268 Melem/s |
| 1 | 14 | 100000 | 82.309 ms / 82.749 ms / 83.457 ms | 1.1982 Melem/s / 1.2085 Melem/s / 1.2149 Melem/s |
| 1 | 14 | 1000000 | 825.61 ms / 830.95 ms / 839.59 ms | 1.1911 Melem/s / 1.2034 Melem/s / 1.2112 Melem/s |
| 1 | 14 | 10000000 | 8.3017 s / 8.3226 s / 8.3489 s | 1.1978 Melem/s / 1.2015 Melem/s / 1.2046 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 4 | 1 | 100000 | 6.7679 ms / 6.9538 ms / 7.1365 ms | 14.013 Melem/s / 14.381 Melem/s / 14.776 Melem/s |
| 4 | 1 | 1000000 | 67.000 ms / 68.963 ms / 71.109 ms | 14.063 Melem/s / 14.501 Melem/s / 14.925 Melem/s |
| 4 | 1 | 10000000 | 669.98 ms / 682.25 ms / 701.40 ms | 14.257 Melem/s / 14.657 Melem/s / 14.926 Melem/s |
| 4 | 4 | 100000 | 13.183 ms / 13.660 ms / 14.492 ms | 6.9002 Melem/s / 7.3205 Melem/s / 7.5853 Melem/s |
| 4 | 4 | 1000000 | 134.98 ms / 138.17 ms / 141.96 ms | 7.0441 Melem/s / 7.2374 Melem/s / 7.4085 Melem/s |
| 4 | 4 | 10000000 | 1.3712 s / 1.3959 s / 1.4225 s | 7.0297 Melem/s / 7.1639 Melem/s / 7.2929 Melem/s |
| 4 | 14 | 100000 | 31.157 ms / 31.714 ms / 32.628 ms | 3.0649 Melem/s / 3.1532 Melem/s / 3.2095 Melem/s |
| 4 | 14 | 1000000 | 315.19 ms / 317.56 ms / 321.40 ms | 3.1114 Melem/s / 3.1490 Melem/s / 3.1727 Melem/s |
| 4 | 14 | 10000000 | 3.1573 s / 3.1720 s / 3.1891 s | 3.1357 Melem/s / 3.1526 Melem/s / 3.1673 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 128 | 1 | 100000 | 1.3890 ms / 1.4232 ms / 1.4813 ms | 67.510 Melem/s / 70.264 Melem/s / 71.996 Melem/s |
| 128 | 1 | 1000000 | 13.827 ms / 13.959 ms / 14.147 ms | 70.687 Melem/s / 71.639 Melem/s / 72.323 Melem/s |
| 128 | 1 | 10000000 | 138.72 ms / 142.39 ms / 147.14 ms | 67.960 Melem/s / 70.228 Melem/s / 72.085 Melem/s |
| 128 | 4 | 100000 | 2.6198 ms / 2.7136 ms / 2.8191 ms | 35.472 Melem/s / 36.851 Melem/s / 38.171 Melem/s |
| 128 | 4 | 1000000 | 26.225 ms / 27.154 ms / 28.097 ms | 35.591 Melem/s / 36.827 Melem/s / 38.132 Melem/s |
| 128 | 4 | 10000000 | 277.96 ms / 284.42 ms / 292.91 ms | 34.140 Melem/s / 35.160 Melem/s / 35.977 Melem/s |
| 128 | 14 | 100000 | 2.7696 ms / 2.9231 ms / 3.1585 ms | 31.660 Melem/s / 34.210 Melem/s / 36.106 Melem/s |
| 128 | 14 | 1000000 | 26.083 ms / 27.248 ms / 28.602 ms | 34.963 Melem/s / 36.700 Melem/s / 38.339 Melem/s |
| 128 | 14 | 10000000 | 259.41 ms / 267.82 ms / 282.21 ms | 35.435 Melem/s / 37.339 Melem/s / 38.549 Melem/s |

## Bounded Batch Results (`MpscBoundedSyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut`, the
consumer drains via `recv_batch_mut`, both reusing a caller-owned buffer (allocation-free
after the drain-in-place `send_batch_mut`). Batch sizes 8, 64 and 512._

### Capacity: 1 (`Cap-1`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 1 | 1 | 100000 | 8 | 23.838 ms / 23.959 ms / 24.080 ms | 4.1529 Melem/s / 4.1737 Melem/s / 4.1950 Melem/s |
| 1 | 1 | 100000 | 64 | 23.576 ms / 23.727 ms / 23.850 ms | 4.1928 Melem/s / 4.2146 Melem/s / 4.2417 Melem/s |
| 1 | 1 | 100000 | 512 | 23.577 ms / 23.691 ms / 23.888 ms | 4.1862 Melem/s / 4.2210 Melem/s / 4.2415 Melem/s |
| 1 | 1 | 1000000 | 8 | 230.44 ms / 235.07 ms / 240.99 ms | 4.1495 Melem/s / 4.2540 Melem/s / 4.3395 Melem/s |
| 1 | 1 | 1000000 | 64 | 231.75 ms / 235.39 ms / 239.46 ms | 4.1760 Melem/s / 4.2483 Melem/s / 4.3149 Melem/s |
| 1 | 1 | 1000000 | 512 | 230.31 ms / 231.70 ms / 233.13 ms | 4.2895 Melem/s / 4.3159 Melem/s / 4.3420 Melem/s |
| 1 | 1 | 10000000 | 8 | 2.2857 s / 2.3191 s / 2.3620 s | 4.2337 Melem/s / 4.3119 Melem/s / 4.3750 Melem/s |
| 1 | 1 | 10000000 | 64 | 2.2949 s / 2.3247 s / 2.3569 s | 4.2429 Melem/s / 4.3016 Melem/s / 4.3575 Melem/s |
| 1 | 1 | 10000000 | 512 | 2.2511 s / 2.2806 s / 2.3135 s | 4.3225 Melem/s / 4.3849 Melem/s / 4.4423 Melem/s |
| 1 | 4 | 100000 | 8 | 140.00 ms / 142.82 ms / 146.25 ms | 683.77 Kelem/s / 700.20 Kelem/s / 714.27 Kelem/s |
| 1 | 4 | 100000 | 64 | 138.62 ms / 140.92 ms / 143.90 ms | 694.91 Kelem/s / 709.61 Kelem/s / 721.38 Kelem/s |
| 1 | 4 | 100000 | 512 | 139.04 ms / 141.31 ms / 145.19 ms | 688.76 Kelem/s / 707.67 Kelem/s / 719.21 Kelem/s |
| 1 | 4 | 1000000 | 8 | 1.3841 s / 1.4025 s / 1.4254 s | 701.55 Kelem/s / 713.02 Kelem/s / 722.50 Kelem/s |
| 1 | 4 | 1000000 | 64 | 1.3959 s / 1.4177 s / 1.4442 s | 692.43 Kelem/s / 705.38 Kelem/s / 716.39 Kelem/s |
| 1 | 4 | 1000000 | 512 | 1.4068 s / 1.4277 s / 1.4508 s | 689.29 Kelem/s / 700.42 Kelem/s / 710.84 Kelem/s |
| 1 | 4 | 10000000 | 8 | 14.098 s / 14.197 s / 14.329 s | 697.89 Kelem/s / 704.37 Kelem/s / 709.33 Kelem/s |
| 1 | 4 | 10000000 | 64 | 14.109 s / 14.156 s / 14.199 s | 704.29 Kelem/s / 706.42 Kelem/s / 708.75 Kelem/s |
| 1 | 4 | 10000000 | 512 | 14.106 s / 14.181 s / 14.260 s | 701.25 Kelem/s / 705.17 Kelem/s / 708.94 Kelem/s |
| 1 | 14 | 100000 | 8 | 157.24 ms / 160.96 ms / 166.23 ms | 601.57 Kelem/s / 621.28 Kelem/s / 635.99 Kelem/s |
| 1 | 14 | 100000 | 64 | 157.37 ms / 159.73 ms / 162.34 ms | 615.97 Kelem/s / 626.07 Kelem/s / 635.46 Kelem/s |
| 1 | 14 | 100000 | 512 | 159.74 ms / 162.28 ms / 164.82 ms | 606.72 Kelem/s / 616.23 Kelem/s / 626.03 Kelem/s |
| 1 | 14 | 1000000 | 8 | 1.5770 s / 1.5975 s / 1.6204 s | 617.13 Kelem/s / 625.98 Kelem/s / 634.13 Kelem/s |
| 1 | 14 | 1000000 | 64 | 1.6084 s / 1.6219 s / 1.6356 s | 611.41 Kelem/s / 616.57 Kelem/s / 621.73 Kelem/s |
| 1 | 14 | 1000000 | 512 | 1.5898 s / 1.6126 s / 1.6351 s | 611.60 Kelem/s / 620.12 Kelem/s / 629.03 Kelem/s |
| 1 | 14 | 10000000 | 8 | 16.048 s / 16.088 s / 16.128 s | 620.03 Kelem/s / 621.59 Kelem/s / 623.12 Kelem/s |
| 1 | 14 | 10000000 | 64 | 16.141 s / 16.203 s / 16.257 s | 615.11 Kelem/s / 617.16 Kelem/s / 619.52 Kelem/s |
| 1 | 14 | 10000000 | 512 | 16.153 s / 16.216 s / 16.283 s | 614.16 Kelem/s / 616.67 Kelem/s / 619.08 Kelem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 100000 | 8 | 16.180 ms / 16.248 ms / 16.356 ms | 6.1139 Melem/s / 6.1546 Melem/s / 6.1804 Melem/s |
| 4 | 1 | 100000 | 64 | 17.664 ms / 17.855 ms / 17.992 ms | 5.5580 Melem/s / 5.6006 Melem/s / 5.6613 Melem/s |
| 4 | 1 | 100000 | 512 | 18.160 ms / 18.390 ms / 18.568 ms | 5.3856 Melem/s / 5.4377 Melem/s / 5.5066 Melem/s |
| 4 | 1 | 1000000 | 8 | 161.13 ms / 163.93 ms / 165.79 ms | 6.0317 Melem/s / 6.1001 Melem/s / 6.2061 Melem/s |
| 4 | 1 | 1000000 | 64 | 177.15 ms / 179.83 ms / 185.06 ms | 5.4037 Melem/s / 5.5608 Melem/s / 5.6450 Melem/s |
| 4 | 1 | 1000000 | 512 | 177.12 ms / 178.70 ms / 180.25 ms | 5.5478 Melem/s / 5.5959 Melem/s / 5.6458 Melem/s |
| 4 | 1 | 10000000 | 8 | 1.6229 s / 1.6426 s / 1.6652 s | 6.0053 Melem/s / 6.0878 Melem/s / 6.1618 Melem/s |
| 4 | 1 | 10000000 | 64 | 1.7802 s / 1.8171 s / 1.8507 s | 5.4035 Melem/s / 5.5032 Melem/s / 5.6174 Melem/s |
| 4 | 1 | 10000000 | 512 | 1.8410 s / 1.8659 s / 1.8975 s | 5.2700 Melem/s / 5.3593 Melem/s / 5.4317 Melem/s |
| 4 | 4 | 100000 | 8 | 57.428 ms / 58.721 ms / 60.306 ms | 1.6582 Melem/s / 1.7030 Melem/s / 1.7413 Melem/s |
| 4 | 4 | 100000 | 64 | 57.171 ms / 58.717 ms / 59.602 ms | 1.6778 Melem/s / 1.7031 Melem/s / 1.7491 Melem/s |
| 4 | 4 | 100000 | 512 | 55.205 ms / 56.075 ms / 57.083 ms | 1.7518 Melem/s / 1.7833 Melem/s / 1.8114 Melem/s |
| 4 | 4 | 1000000 | 8 | 588.95 ms / 597.63 ms / 605.68 ms | 1.6510 Melem/s / 1.6733 Melem/s / 1.6979 Melem/s |
| 4 | 4 | 1000000 | 64 | 524.47 ms / 556.12 ms / 582.37 ms | 1.7171 Melem/s / 1.7982 Melem/s / 1.9067 Melem/s |
| 4 | 4 | 1000000 | 512 | 561.45 ms / 575.24 ms / 587.45 ms | 1.7023 Melem/s / 1.7384 Melem/s / 1.7811 Melem/s |
| 4 | 4 | 10000000 | 8 | 6.5298 s / 6.6192 s / 6.7062 s | 1.4912 Melem/s / 1.5108 Melem/s / 1.5314 Melem/s |
| 4 | 4 | 10000000 | 64 | 6.2025 s / 6.2786 s / 6.3572 s | 1.5730 Melem/s / 1.5927 Melem/s / 1.6123 Melem/s |
| 4 | 4 | 10000000 | 512 | 6.2067 s / 6.2665 s / 6.3193 s | 1.5825 Melem/s / 1.5958 Melem/s / 1.6112 Melem/s |
| 4 | 14 | 100000 | 8 | 65.701 ms / 66.449 ms / 66.938 ms | 1.4939 Melem/s / 1.5049 Melem/s / 1.5221 Melem/s |
| 4 | 14 | 100000 | 64 | 68.216 ms / 68.948 ms / 69.666 ms | 1.4354 Melem/s / 1.4504 Melem/s / 1.4659 Melem/s |
| 4 | 14 | 100000 | 512 | 68.756 ms / 70.080 ms / 71.393 ms | 1.4007 Melem/s / 1.4270 Melem/s / 1.4544 Melem/s |
| 4 | 14 | 1000000 | 8 | 646.18 ms / 654.28 ms / 660.29 ms | 1.5145 Melem/s / 1.5284 Melem/s / 1.5476 Melem/s |
| 4 | 14 | 1000000 | 64 | 666.66 ms / 673.47 ms / 679.47 ms | 1.4717 Melem/s / 1.4849 Melem/s / 1.5000 Melem/s |
| 4 | 14 | 1000000 | 512 | 693.17 ms / 700.39 ms / 709.69 ms | 1.4091 Melem/s / 1.4278 Melem/s / 1.4426 Melem/s |
| 4 | 14 | 10000000 | 8 | 6.6026 s / 6.6494 s / 6.6893 s | 1.4949 Melem/s / 1.5039 Melem/s / 1.5146 Melem/s |
| 4 | 14 | 10000000 | 64 | 6.8194 s / 6.8581 s / 6.8946 s | 1.4504 Melem/s / 1.4581 Melem/s / 1.4664 Melem/s |
| 4 | 14 | 10000000 | 512 | 6.9985 s / 7.0434 s / 7.0865 s | 1.4111 Melem/s / 1.4198 Melem/s / 1.4289 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 100000 | 8 | 1.4840 ms / 1.5018 ms / 1.5409 ms | 64.895 Melem/s / 66.585 Melem/s / 67.383 Melem/s |
| 128 | 1 | 100000 | 64 | 2.0595 ms / 2.0882 ms / 2.1457 ms | 46.605 Melem/s / 47.888 Melem/s / 48.555 Melem/s |
| 128 | 1 | 100000 | 512 | 3.5740 ms / 3.6351 ms / 3.7548 ms | 26.632 Melem/s / 27.510 Melem/s / 27.980 Melem/s |
| 128 | 1 | 1000000 | 8 | 14.413 ms / 14.855 ms / 15.483 ms | 64.587 Melem/s / 67.318 Melem/s / 69.381 Melem/s |
| 128 | 1 | 1000000 | 64 | 19.752 ms / 20.377 ms / 21.129 ms | 47.329 Melem/s / 49.074 Melem/s / 50.627 Melem/s |
| 128 | 1 | 1000000 | 512 | 35.718 ms / 36.205 ms / 37.104 ms | 26.951 Melem/s / 27.620 Melem/s / 27.997 Melem/s |
| 128 | 1 | 10000000 | 8 | 145.19 ms / 146.77 ms / 149.93 ms | 66.699 Melem/s / 68.132 Melem/s / 68.874 Melem/s |
| 128 | 1 | 10000000 | 64 | 207.06 ms / 213.31 ms / 223.16 ms | 44.812 Melem/s / 46.879 Melem/s / 48.295 Melem/s |
| 128 | 1 | 10000000 | 512 | 357.60 ms / 360.53 ms / 363.56 ms | 27.506 Melem/s / 27.737 Melem/s / 27.964 Melem/s |
| 128 | 4 | 100000 | 8 | 1.8438 ms / 1.8560 ms / 1.8633 ms | 53.669 Melem/s / 53.880 Melem/s / 54.236 Melem/s |
| 128 | 4 | 100000 | 64 | 2.4220 ms / 2.4679 ms / 2.5484 ms | 39.240 Melem/s / 40.521 Melem/s / 41.288 Melem/s |
| 128 | 4 | 100000 | 512 | 3.6102 ms / 3.6735 ms / 3.7207 ms | 26.876 Melem/s / 27.222 Melem/s / 27.699 Melem/s |
| 128 | 4 | 1000000 | 8 | 18.068 ms / 18.154 ms / 18.286 ms | 54.686 Melem/s / 55.085 Melem/s / 55.348 Melem/s |
| 128 | 4 | 1000000 | 64 | 24.098 ms / 24.303 ms / 24.875 ms | 40.200 Melem/s / 41.147 Melem/s / 41.498 Melem/s |
| 128 | 4 | 1000000 | 512 | 36.965 ms / 37.345 ms / 37.965 ms | 26.340 Melem/s / 26.777 Melem/s / 27.052 Melem/s |
| 128 | 4 | 10000000 | 8 | 179.00 ms / 179.77 ms / 180.64 ms | 55.358 Melem/s / 55.625 Melem/s / 55.866 Melem/s |
| 128 | 4 | 10000000 | 64 | 241.76 ms / 245.34 ms / 251.12 ms | 39.822 Melem/s / 40.759 Melem/s / 41.364 Melem/s |
| 128 | 4 | 10000000 | 512 | 365.20 ms / 370.54 ms / 377.98 ms | 26.456 Melem/s / 26.988 Melem/s / 27.383 Melem/s |
| 128 | 14 | 100000 | 8 | 1.9474 ms / 2.0091 ms / 2.0664 ms | 48.393 Melem/s / 49.774 Melem/s / 51.349 Melem/s |
| 128 | 14 | 100000 | 64 | 2.5801 ms / 2.6362 ms / 2.6993 ms | 37.047 Melem/s / 37.934 Melem/s / 38.759 Melem/s |
| 128 | 14 | 100000 | 512 | 3.8410 ms / 3.9556 ms / 4.1040 ms | 24.366 Melem/s / 25.280 Melem/s / 26.035 Melem/s |
| 128 | 14 | 1000000 | 8 | 17.927 ms / 18.362 ms / 18.943 ms | 52.789 Melem/s / 54.461 Melem/s / 55.783 Melem/s |
| 128 | 14 | 1000000 | 64 | 24.485 ms / 24.827 ms / 25.269 ms | 39.574 Melem/s / 40.278 Melem/s / 40.841 Melem/s |
| 128 | 14 | 1000000 | 512 | 36.431 ms / 37.414 ms / 38.367 ms | 26.064 Melem/s / 26.728 Melem/s / 27.449 Melem/s |
| 128 | 14 | 10000000 | 8 | 178.03 ms / 179.08 ms / 180.93 ms | 55.270 Melem/s / 55.841 Melem/s / 56.169 Melem/s |
| 128 | 14 | 10000000 | 64 | 244.24 ms / 247.62 ms / 253.34 ms | 39.473 Melem/s / 40.384 Melem/s / 40.944 Melem/s |
| 128 | 14 | 10000000 | 512 | 372.76 ms / 377.06 ms / 383.28 ms | 26.090 Melem/s / 26.521 Melem/s / 26.827 Melem/s |

**Notes.** At `Cap-128`, `Batch-8` and `Batch-64` run at 48-61 Melem/s for 1, 4 and 14 producers. `Batch-512` runs at 28-30 Melem/s. `Cap-4` runs at 5.0-5.6 Melem/s with 1 producer and 1.5-1.8 with 4 or 14. `Cap-1` runs at 3.6-3.7 Melem/s with 1 producer and 0.54-0.65 with 4 or 14.
