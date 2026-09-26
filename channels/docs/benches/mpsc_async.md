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
Batch sizes 8, 64 and 512._

### Capacity: 1 (`Cap-1`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 1 | 1 | 100000 | 8 | 11.397 ms / 11.456 ms / 11.551 ms | 8.6573 Melem/s / 8.7287 Melem/s / 8.7741 Melem/s |
| 1 | 1 | 100000 | 64 | 11.900 ms / 11.948 ms / 11.987 ms | 8.3421 Melem/s / 8.3699 Melem/s / 8.4033 Melem/s |
| 1 | 1 | 100000 | 512 | 13.895 ms / 13.975 ms / 14.085 ms | 7.0995 Melem/s / 7.1558 Melem/s / 7.1966 Melem/s |
| 1 | 1 | 1000000 | 8 | 112.53 ms / 112.72 ms / 113.07 ms | 8.8444 Melem/s / 8.8715 Melem/s / 8.8868 Melem/s |
| 1 | 1 | 1000000 | 64 | 119.65 ms / 125.59 ms / 134.69 ms | 7.4245 Melem/s / 7.9623 Melem/s / 8.3575 Melem/s |
| 1 | 1 | 1000000 | 512 | 139.35 ms / 140.51 ms / 141.27 ms | 7.0785 Melem/s / 7.1169 Melem/s / 7.1764 Melem/s |
| 1 | 1 | 10000000 | 8 | 1.1040 s / 1.1219 s / 1.1411 s | 8.7637 Melem/s / 8.9134 Melem/s / 9.0582 Melem/s |
| 1 | 1 | 10000000 | 64 | 1.1634 s / 1.1724 s / 1.1802 s | 8.4731 Melem/s / 8.5296 Melem/s / 8.5952 Melem/s |
| 1 | 1 | 10000000 | 512 | 1.4289 s / 1.4494 s / 1.4676 s | 6.8138 Melem/s / 6.8996 Melem/s / 6.9986 Melem/s |
| 1 | 4 | 100000 | 8 | 11.473 ms / 11.532 ms / 11.580 ms | 8.6354 Melem/s / 8.6713 Melem/s / 8.7164 Melem/s |
| 1 | 4 | 100000 | 64 | 12.132 ms / 12.213 ms / 12.288 ms | 8.1377 Melem/s / 8.1877 Melem/s / 8.2426 Melem/s |
| 1 | 4 | 100000 | 512 | 14.587 ms / 14.747 ms / 15.055 ms | 6.6425 Melem/s / 6.7810 Melem/s / 6.8555 Melem/s |
| 1 | 4 | 1000000 | 8 | 112.29 ms / 113.35 ms / 114.55 ms | 8.7296 Melem/s / 8.8226 Melem/s / 8.9052 Melem/s |
| 1 | 4 | 1000000 | 64 | 118.08 ms / 119.00 ms / 119.75 ms | 8.3504 Melem/s / 8.4037 Melem/s / 8.4690 Melem/s |
| 1 | 4 | 1000000 | 512 | 145.60 ms / 147.57 ms / 151.14 ms | 6.6165 Melem/s / 6.7766 Melem/s / 6.8680 Melem/s |
| 1 | 4 | 10000000 | 8 | 1.1194 s / 1.1319 s / 1.1443 s | 8.7391 Melem/s / 8.8343 Melem/s / 8.9330 Melem/s |
| 1 | 4 | 10000000 | 64 | 1.1740 s / 1.1855 s / 1.1978 s | 8.3484 Melem/s / 8.4354 Melem/s / 8.5177 Melem/s |
| 1 | 4 | 10000000 | 512 | 1.4823 s / 1.4960 s / 1.5089 s | 6.6275 Melem/s / 6.6843 Melem/s / 6.7461 Melem/s |
| 1 | 14 | 100000 | 8 | 11.688 ms / 11.771 ms / 11.930 ms | 8.3820 Melem/s / 8.4957 Melem/s / 8.5561 Melem/s |
| 1 | 14 | 100000 | 64 | 12.118 ms / 12.159 ms / 12.211 ms | 8.1897 Melem/s / 8.2245 Melem/s / 8.2522 Melem/s |
| 1 | 14 | 100000 | 512 | 15.970 ms / 16.323 ms / 16.668 ms | 5.9996 Melem/s / 6.1263 Melem/s / 6.2619 Melem/s |
| 1 | 14 | 1000000 | 8 | 114.38 ms / 115.94 ms / 117.25 ms | 8.5288 Melem/s / 8.6248 Melem/s / 8.7426 Melem/s |
| 1 | 14 | 1000000 | 64 | 119.42 ms / 120.92 ms / 122.32 ms | 8.1752 Melem/s / 8.2702 Melem/s / 8.3739 Melem/s |
| 1 | 14 | 1000000 | 512 | 161.46 ms / 164.57 ms / 167.32 ms | 5.9765 Melem/s / 6.0763 Melem/s / 6.1936 Melem/s |
| 1 | 14 | 10000000 | 8 | 1.1355 s / 1.1462 s / 1.1563 s | 8.6486 Melem/s / 8.7242 Melem/s / 8.8063 Melem/s |
| 1 | 14 | 10000000 | 64 | 1.1993 s / 1.2110 s / 1.2235 s | 8.1732 Melem/s / 8.2578 Melem/s / 8.3383 Melem/s |
| 1 | 14 | 10000000 | 512 | 1.6248 s / 1.6536 s / 1.6790 s | 5.9558 Melem/s / 6.0476 Melem/s / 6.1548 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 100000 | 8 | 3.0765 ms / 3.1835 ms / 3.2562 ms | 30.710 Melem/s / 31.412 Melem/s / 32.504 Melem/s |
| 4 | 1 | 100000 | 64 | 3.2303 ms / 3.2419 ms / 3.2712 ms | 30.570 Melem/s / 30.846 Melem/s / 30.957 Melem/s |
| 4 | 1 | 100000 | 512 | 3.7742 ms / 3.7973 ms / 3.8174 ms | 26.196 Melem/s / 26.334 Melem/s / 26.496 Melem/s |
| 4 | 1 | 1000000 | 8 | 30.842 ms / 31.066 ms / 31.343 ms | 31.905 Melem/s / 32.190 Melem/s / 32.423 Melem/s |
| 4 | 1 | 1000000 | 64 | 32.319 ms / 32.450 ms / 32.531 ms | 30.740 Melem/s / 30.817 Melem/s / 30.941 Melem/s |
| 4 | 1 | 1000000 | 512 | 37.673 ms / 37.777 ms / 37.874 ms | 26.403 Melem/s / 26.471 Melem/s / 26.545 Melem/s |
| 4 | 1 | 10000000 | 8 | 302.27 ms / 305.88 ms / 310.35 ms | 32.222 Melem/s / 32.692 Melem/s / 33.084 Melem/s |
| 4 | 1 | 10000000 | 64 | 317.16 ms / 319.03 ms / 321.10 ms | 31.143 Melem/s / 31.345 Melem/s / 31.530 Melem/s |
| 4 | 1 | 10000000 | 512 | 380.77 ms / 386.88 ms / 394.11 ms | 25.373 Melem/s / 25.848 Melem/s / 26.263 Melem/s |
| 4 | 4 | 100000 | 8 | 3.2071 ms / 3.2165 ms / 3.2243 ms | 31.015 Melem/s / 31.090 Melem/s / 31.180 Melem/s |
| 4 | 4 | 100000 | 64 | 3.2983 ms / 3.3088 ms / 3.3188 ms | 30.132 Melem/s / 30.222 Melem/s / 30.319 Melem/s |
| 4 | 4 | 100000 | 512 | 3.9770 ms / 3.9834 ms / 3.9928 ms | 25.045 Melem/s / 25.104 Melem/s / 25.145 Melem/s |
| 4 | 4 | 1000000 | 8 | 31.817 ms / 32.406 ms / 33.018 ms | 30.286 Melem/s / 30.858 Melem/s / 31.429 Melem/s |
| 4 | 4 | 1000000 | 64 | 32.822 ms / 32.990 ms / 33.177 ms | 30.141 Melem/s / 30.312 Melem/s / 30.468 Melem/s |
| 4 | 4 | 1000000 | 512 | 40.294 ms / 40.910 ms / 41.469 ms | 24.114 Melem/s / 24.444 Melem/s / 24.817 Melem/s |
| 4 | 4 | 10000000 | 8 | 316.24 ms / 320.10 ms / 324.64 ms | 30.803 Melem/s / 31.240 Melem/s / 31.622 Melem/s |
| 4 | 4 | 10000000 | 64 | 329.16 ms / 333.31 ms / 337.27 ms | 29.650 Melem/s / 30.002 Melem/s / 30.380 Melem/s |
| 4 | 4 | 10000000 | 512 | 407.13 ms / 426.30 ms / 457.63 ms | 21.852 Melem/s / 23.458 Melem/s / 24.562 Melem/s |
| 4 | 14 | 100000 | 8 | 3.2929 ms / 3.3069 ms / 3.3241 ms | 30.083 Melem/s / 30.240 Melem/s / 30.369 Melem/s |
| 4 | 14 | 100000 | 64 | 3.4263 ms / 3.4389 ms / 3.4586 ms | 28.913 Melem/s / 29.079 Melem/s / 29.186 Melem/s |
| 4 | 14 | 100000 | 512 | 4.4585 ms / 4.5479 ms / 4.6606 ms | 21.456 Melem/s / 21.988 Melem/s / 22.429 Melem/s |
| 4 | 14 | 1000000 | 8 | 32.659 ms / 32.825 ms / 32.955 ms | 30.345 Melem/s / 30.465 Melem/s / 30.619 Melem/s |
| 4 | 14 | 1000000 | 64 | 34.228 ms / 34.307 ms / 34.413 ms | 29.059 Melem/s / 29.149 Melem/s / 29.216 Melem/s |
| 4 | 14 | 1000000 | 512 | 44.231 ms / 44.782 ms / 45.559 ms | 21.949 Melem/s / 22.330 Melem/s / 22.609 Melem/s |
| 4 | 14 | 10000000 | 8 | 323.71 ms / 326.48 ms / 329.22 ms | 30.375 Melem/s / 30.630 Melem/s / 30.892 Melem/s |
| 4 | 14 | 10000000 | 64 | 336.02 ms / 340.64 ms / 345.27 ms | 28.963 Melem/s / 29.357 Melem/s / 29.760 Melem/s |
| 4 | 14 | 10000000 | 512 | 437.28 ms / 442.07 ms / 446.98 ms | 22.372 Melem/s / 22.621 Melem/s / 22.869 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 100000 | 8 | 793.27 µs / 802.26 µs / 810.01 µs | 123.46 Melem/s / 124.65 Melem/s / 126.06 Melem/s |
| 128 | 1 | 100000 | 64 | 482.54 µs / 487.48 µs / 493.42 µs | 202.67 Melem/s / 205.14 Melem/s / 207.24 Melem/s |
| 128 | 1 | 100000 | 512 | 453.75 µs / 455.69 µs / 459.52 µs | 217.62 Melem/s / 219.45 Melem/s / 220.39 Melem/s |
| 128 | 1 | 1000000 | 8 | 7.8459 ms / 7.9030 ms / 7.9503 ms | 125.78 Melem/s / 126.54 Melem/s / 127.46 Melem/s |
| 128 | 1 | 1000000 | 64 | 4.7096 ms / 4.7375 ms / 4.7675 ms | 209.75 Melem/s / 211.08 Melem/s / 212.33 Melem/s |
| 128 | 1 | 1000000 | 512 | 4.4412 ms / 4.4490 ms / 4.4555 ms | 224.44 Melem/s / 224.77 Melem/s / 225.17 Melem/s |
| 128 | 1 | 10000000 | 8 | 81.469 ms / 82.138 ms / 82.717 ms | 120.89 Melem/s / 121.75 Melem/s / 122.75 Melem/s |
| 128 | 1 | 10000000 | 64 | 47.121 ms / 47.272 ms / 47.530 ms | 210.39 Melem/s / 211.54 Melem/s / 212.22 Melem/s |
| 128 | 1 | 10000000 | 512 | 44.370 ms / 44.763 ms / 45.165 ms | 221.41 Melem/s / 223.40 Melem/s / 225.38 Melem/s |
| 128 | 4 | 100000 | 8 | 873.17 µs / 875.72 µs / 878.37 µs | 113.85 Melem/s / 114.19 Melem/s / 114.52 Melem/s |
| 128 | 4 | 100000 | 64 | 492.86 µs / 497.91 µs / 507.78 µs | 196.94 Melem/s / 200.84 Melem/s / 202.90 Melem/s |
| 128 | 4 | 100000 | 512 | 461.33 µs / 461.80 µs / 462.35 µs | 216.29 Melem/s / 216.54 Melem/s / 216.76 Melem/s |
| 128 | 4 | 1000000 | 8 | 8.4708 ms / 8.5130 ms / 8.5518 ms | 116.93 Melem/s / 117.47 Melem/s / 118.05 Melem/s |
| 128 | 4 | 1000000 | 64 | 4.7408 ms / 4.7656 ms / 4.7979 ms | 208.43 Melem/s / 209.84 Melem/s / 210.94 Melem/s |
| 128 | 4 | 1000000 | 512 | 4.4934 ms / 4.5289 ms / 4.5810 ms | 218.29 Melem/s / 220.81 Melem/s / 222.55 Melem/s |
| 128 | 4 | 10000000 | 8 | 83.932 ms / 85.209 ms / 86.024 ms | 116.25 Melem/s / 117.36 Melem/s / 119.14 Melem/s |
| 128 | 4 | 10000000 | 64 | 47.235 ms / 47.522 ms / 47.899 ms | 208.77 Melem/s / 210.43 Melem/s / 211.71 Melem/s |
| 128 | 4 | 10000000 | 512 | 44.893 ms / 45.011 ms / 45.129 ms | 221.59 Melem/s / 222.17 Melem/s / 222.75 Melem/s |
| 128 | 14 | 100000 | 8 | 920.32 µs / 923.33 µs / 926.78 µs | 107.90 Melem/s / 108.30 Melem/s / 108.66 Melem/s |
| 128 | 14 | 100000 | 64 | 504.71 µs / 515.07 µs / 528.82 µs | 189.10 Melem/s / 194.15 Melem/s / 198.14 Melem/s |
| 128 | 14 | 100000 | 512 | 478.93 µs / 481.62 µs / 487.39 µs | 205.17 Melem/s / 207.63 Melem/s / 208.80 Melem/s |
| 128 | 14 | 1000000 | 8 | 8.5368 ms / 8.6109 ms / 8.6969 ms | 114.98 Melem/s / 116.13 Melem/s / 117.14 Melem/s |
| 128 | 14 | 1000000 | 64 | 4.7366 ms / 4.7781 ms / 4.8343 ms | 206.86 Melem/s / 209.29 Melem/s / 211.12 Melem/s |
| 128 | 14 | 1000000 | 512 | 4.5378 ms / 4.5454 ms / 4.5568 ms | 219.45 Melem/s / 220.00 Melem/s / 220.37 Melem/s |
| 128 | 14 | 10000000 | 8 | 84.152 ms / 84.892 ms / 85.992 ms | 116.29 Melem/s / 117.80 Melem/s / 118.83 Melem/s |
| 128 | 14 | 10000000 | 64 | 48.008 ms / 49.021 ms / 49.891 ms | 200.44 Melem/s / 203.99 Melem/s / 208.30 Melem/s |
| 128 | 14 | 10000000 | 512 | 45.738 ms / 46.026 ms / 46.522 ms | 214.95 Melem/s / 217.27 Melem/s / 218.64 Melem/s |

**Notes.** At `Cap-128` throughput rises with batch size: 108-127 Melem/s at `Batch-8`, 194-212 at `Batch-64` and 208-225 at `Batch-512`, close to flat across 1, 4 and 14 producers. At `Cap-1` and `Cap-4`, `Batch-512` is 18-30% below `Batch-8`.
