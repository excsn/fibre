# Fibre Benchmark: `MpscAsync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedAsync`)

`MpscUnboundedAsync/Prod-{Prod}_Items-{Items}`

| Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 100000 | 817.17 µs / 823.30 µs / 839.04 µs | 119.18 Melem/s / 121.46 Melem/s / 122.37 Melem/s |
| 1 | 1000000 | 8.4621 ms / 8.5342 ms / 8.6437 ms | 115.69 Melem/s / 117.18 Melem/s / 118.17 Melem/s |
| 1 | 10000000 | 85.547 ms / 85.698 ms / 86.098 ms | 116.15 Melem/s / 116.69 Melem/s / 116.90 Melem/s |
| 4 | 100000 | 3.9872 ms / 4.1822 ms / 4.3760 ms | 22.852 Melem/s / 23.911 Melem/s / 25.080 Melem/s |
| 4 | 1000000 | 37.815 ms / 38.449 ms / 39.203 ms | 25.508 Melem/s / 26.009 Melem/s / 26.445 Melem/s |
| 4 | 10000000 | 405.90 ms / 419.79 ms / 434.30 ms | 23.025 Melem/s / 23.822 Melem/s / 24.637 Melem/s |
| 14 | 100000 | 7.4105 ms / 7.4215 ms / 7.4420 ms | 13.437 Melem/s / 13.474 Melem/s / 13.494 Melem/s |
| 14 | 1000000 | 75.777 ms / 76.901 ms / 77.551 ms | 12.895 Melem/s / 13.004 Melem/s / 13.197 Melem/s |
| 14 | 10000000 | 706.79 ms / 756.45 ms / 782.24 ms | 12.784 Melem/s / 13.220 Melem/s / 14.148 Melem/s |

---

## Bounded Results (`MpscBoundedAsync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 9.7959 ms / 9.8373 ms / 9.9085 ms | 10.092 Melem/s / 10.165 Melem/s / 10.208 Melem/s |
| 1 | 1 | 1000000 | 97.877 ms / 98.909 ms / 99.665 ms | 10.034 Melem/s / 10.110 Melem/s / 10.217 Melem/s |
| 1 | 1 | 10000000 | 991.34 ms / 999.02 ms / 1.0067 s | 9.9338 Melem/s / 10.010 Melem/s / 10.087 Melem/s |
| 1 | 4 | 100000 | 9.9092 ms / 10.027 ms / 10.156 ms | 9.8459 Melem/s / 9.9734 Melem/s / 10.092 Melem/s |
| 1 | 4 | 1000000 | 98.330 ms / 99.615 ms / 100.45 ms | 9.9549 Melem/s / 10.039 Melem/s / 10.170 Melem/s |
| 1 | 4 | 10000000 | 988.87 ms / 995.29 ms / 1.0016 s | 9.9838 Melem/s / 10.047 Melem/s / 10.113 Melem/s |
| 1 | 14 | 100000 | 10.059 ms / 10.108 ms / 10.166 ms | 9.8370 Melem/s / 9.8934 Melem/s / 9.9411 Melem/s |
| 1 | 14 | 1000000 | 100.77 ms / 101.52 ms / 102.38 ms | 9.7676 Melem/s / 9.8502 Melem/s / 9.9239 Melem/s |
| 1 | 14 | 10000000 | 1.0010 s / 1.0109 s / 1.0218 s | 9.7864 Melem/s / 9.8917 Melem/s / 9.9896 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 4 | 1 | 100000 | 3.5344 ms / 3.5533 ms / 3.5700 ms | 28.011 Melem/s / 28.143 Melem/s / 28.294 Melem/s |
| 4 | 1 | 1000000 | 35.546 ms / 35.682 ms / 35.758 ms | 27.966 Melem/s / 28.026 Melem/s / 28.133 Melem/s |
| 4 | 1 | 10000000 | 354.44 ms / 358.95 ms / 364.27 ms | 27.452 Melem/s / 27.859 Melem/s / 28.213 Melem/s |
| 4 | 4 | 100000 | 3.6519 ms / 3.6697 ms / 3.6929 ms | 27.079 Melem/s / 27.250 Melem/s / 27.383 Melem/s |
| 4 | 4 | 1000000 | 36.116 ms / 36.633 ms / 37.286 ms | 26.820 Melem/s / 27.298 Melem/s / 27.689 Melem/s |
| 4 | 4 | 10000000 | 359.07 ms / 361.31 ms / 363.64 ms | 27.500 Melem/s / 27.677 Melem/s / 27.849 Melem/s |
| 4 | 14 | 100000 | 3.7529 ms / 3.8114 ms / 3.9166 ms | 25.532 Melem/s / 26.237 Melem/s / 26.646 Melem/s |
| 4 | 14 | 1000000 | 36.835 ms / 37.229 ms / 37.768 ms | 26.477 Melem/s / 26.861 Melem/s / 27.148 Melem/s |
| 4 | 14 | 10000000 | 361.37 ms / 363.60 ms / 365.67 ms | 27.347 Melem/s / 27.502 Melem/s / 27.672 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedAsync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 128 | 1 | 100000 | 1.4462 ms / 1.4595 ms / 1.4880 ms | 67.203 Melem/s / 68.515 Melem/s / 69.147 Melem/s |
| 128 | 1 | 1000000 | 14.971 ms / 15.161 ms / 15.504 ms | 64.501 Melem/s / 65.961 Melem/s / 66.794 Melem/s |
| 128 | 1 | 10000000 | 155.91 ms / 158.47 ms / 161.08 ms | 62.082 Melem/s / 63.104 Melem/s / 64.139 Melem/s |
| 128 | 4 | 100000 | 2.3051 ms / 2.3485 ms / 2.3964 ms | 41.729 Melem/s / 42.581 Melem/s / 43.382 Melem/s |
| 128 | 4 | 1000000 | 15.680 ms / 16.183 ms / 16.645 ms | 60.076 Melem/s / 61.792 Melem/s / 63.777 Melem/s |
| 128 | 4 | 10000000 | 144.25 ms / 145.83 ms / 147.74 ms | 67.688 Melem/s / 68.573 Melem/s / 69.324 Melem/s |
| 128 | 14 | 100000 | 2.1773 ms / 2.2144 ms / 2.2530 ms | 44.385 Melem/s / 45.158 Melem/s / 45.929 Melem/s |
| 128 | 14 | 1000000 | 15.092 ms / 15.538 ms / 15.968 ms | 62.626 Melem/s / 64.358 Melem/s / 66.258 Melem/s |
| 128 | 14 | 10000000 | 143.39 ms / 144.63 ms / 146.09 ms | 68.449 Melem/s / 69.141 Melem/s / 69.739 Melem/s |

## Bounded Batch Results (`MpscBoundedAsyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut().await`,
the consumer drains via `recv_batch_mut().await`, both reusing a caller-owned buffer.
Batch sizes 8, 64 and 512._

### Capacity: 1 (`Cap-1`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 1 | 1 | 100000 | 8 | 10.868 ms / 10.919 ms / 10.996 ms | 9.0941 Melem/s / 9.1580 Melem/s / 9.2015 Melem/s |
| 1 | 1 | 100000 | 64 | 10.893 ms / 11.011 ms / 11.185 ms | 8.9405 Melem/s / 9.0822 Melem/s / 9.1801 Melem/s |
| 1 | 1 | 100000 | 512 | 12.709 ms / 12.757 ms / 12.827 ms | 7.7959 Melem/s / 7.8389 Melem/s / 7.8682 Melem/s |
| 1 | 1 | 1000000 | 8 | 108.33 ms / 108.67 ms / 109.04 ms | 9.1714 Melem/s / 9.2022 Melem/s / 9.2313 Melem/s |
| 1 | 1 | 1000000 | 64 | 108.40 ms / 108.93 ms / 109.43 ms | 9.1379 Melem/s / 9.1805 Melem/s / 9.2251 Melem/s |
| 1 | 1 | 1000000 | 512 | 129.12 ms / 129.85 ms / 130.80 ms | 7.6454 Melem/s / 7.7013 Melem/s / 7.7449 Melem/s |
| 1 | 1 | 10000000 | 8 | 1.0847 s / 1.0867 s / 1.0890 s | 9.1828 Melem/s / 9.2026 Melem/s / 9.2188 Melem/s |
| 1 | 1 | 10000000 | 64 | 1.0869 s / 1.0913 s / 1.0958 s | 9.1254 Melem/s / 9.1632 Melem/s / 9.2003 Melem/s |
| 1 | 1 | 10000000 | 512 | 1.2993 s / 1.3070 s / 1.3146 s | 7.6066 Melem/s / 7.6511 Melem/s / 7.6965 Melem/s |
| 1 | 4 | 100000 | 8 | 10.929 ms / 10.939 ms / 10.958 ms | 9.1257 Melem/s / 9.1412 Melem/s / 9.1501 Melem/s |
| 1 | 4 | 100000 | 64 | 10.957 ms / 11.052 ms / 11.155 ms | 8.9645 Melem/s / 9.0483 Melem/s / 9.1267 Melem/s |
| 1 | 4 | 100000 | 512 | 12.964 ms / 13.003 ms / 13.059 ms | 7.6573 Melem/s / 7.6908 Melem/s / 7.7136 Melem/s |
| 1 | 4 | 1000000 | 8 | 109.04 ms / 109.14 ms / 109.30 ms | 9.1495 Melem/s / 9.1621 Melem/s / 9.1707 Melem/s |
| 1 | 4 | 1000000 | 64 | 109.55 ms / 110.25 ms / 110.96 ms | 9.0122 Melem/s / 9.0700 Melem/s / 9.1285 Melem/s |
| 1 | 4 | 1000000 | 512 | 129.79 ms / 131.01 ms / 132.10 ms | 7.5698 Melem/s / 7.6327 Melem/s / 7.7049 Melem/s |
| 1 | 4 | 10000000 | 8 | 1.0864 s / 1.0893 s / 1.0936 s | 9.1438 Melem/s / 9.1800 Melem/s / 9.2051 Melem/s |
| 1 | 4 | 10000000 | 64 | 1.0903 s / 1.0945 s / 1.0987 s | 9.1017 Melem/s / 9.1368 Melem/s / 9.1716 Melem/s |
| 1 | 4 | 10000000 | 512 | 1.3093 s / 1.3229 s / 1.3376 s | 7.4759 Melem/s / 7.5589 Melem/s / 7.6376 Melem/s |
| 1 | 14 | 100000 | 8 | 11.114 ms / 11.134 ms / 11.154 ms | 8.9655 Melem/s / 8.9815 Melem/s / 8.9973 Melem/s |
| 1 | 14 | 100000 | 64 | 11.193 ms / 11.211 ms / 11.222 ms | 8.9110 Melem/s / 8.9197 Melem/s / 8.9338 Melem/s |
| 1 | 14 | 100000 | 512 | 13.477 ms / 13.534 ms / 13.632 ms | 7.3356 Melem/s / 7.3886 Melem/s / 7.4201 Melem/s |
| 1 | 14 | 1000000 | 8 | 111.39 ms / 111.84 ms / 112.21 ms | 8.9115 Melem/s / 8.9417 Melem/s / 8.9776 Melem/s |
| 1 | 14 | 1000000 | 64 | 111.53 ms / 111.95 ms / 112.67 ms | 8.8753 Melem/s / 8.9324 Melem/s / 8.9665 Melem/s |
| 1 | 14 | 1000000 | 512 | 135.15 ms / 137.68 ms / 138.88 ms | 7.2003 Melem/s / 7.2632 Melem/s / 7.3992 Melem/s |
| 1 | 14 | 10000000 | 8 | 1.0955 s / 1.0990 s / 1.1025 s | 9.0702 Melem/s / 9.0995 Melem/s / 9.1283 Melem/s |
| 1 | 14 | 10000000 | 64 | 1.1094 s / 1.1177 s / 1.1268 s | 8.8747 Melem/s / 8.9469 Melem/s / 9.0141 Melem/s |
| 1 | 14 | 10000000 | 512 | 1.3499 s / 1.3753 s / 1.4013 s | 7.1360 Melem/s / 7.2712 Melem/s / 7.4082 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 100000 | 8 | 2.8643 ms / 2.8684 ms / 2.8719 ms | 34.820 Melem/s / 34.863 Melem/s / 34.913 Melem/s |
| 4 | 1 | 100000 | 64 | 2.8979 ms / 2.9033 ms / 2.9085 ms | 34.382 Melem/s / 34.443 Melem/s / 34.508 Melem/s |
| 4 | 1 | 100000 | 512 | 3.4456 ms / 3.4536 ms / 3.4665 ms | 28.848 Melem/s / 28.955 Melem/s / 29.022 Melem/s |
| 4 | 1 | 1000000 | 8 | 28.520 ms / 28.556 ms / 28.589 ms | 34.978 Melem/s / 35.019 Melem/s / 35.063 Melem/s |
| 4 | 1 | 1000000 | 64 | 28.713 ms / 28.929 ms / 29.366 ms | 34.052 Melem/s / 34.567 Melem/s / 34.828 Melem/s |
| 4 | 1 | 1000000 | 512 | 34.220 ms / 34.292 ms / 34.361 ms | 29.102 Melem/s / 29.162 Melem/s / 29.223 Melem/s |
| 4 | 1 | 10000000 | 8 | 285.27 ms / 286.30 ms / 287.46 ms | 34.787 Melem/s / 34.928 Melem/s / 35.055 Melem/s |
| 4 | 1 | 10000000 | 64 | 289.57 ms / 290.99 ms / 292.40 ms | 34.200 Melem/s / 34.365 Melem/s / 34.534 Melem/s |
| 4 | 1 | 10000000 | 512 | 344.46 ms / 346.20 ms / 348.05 ms | 28.731 Melem/s / 28.885 Melem/s / 29.031 Melem/s |
| 4 | 4 | 100000 | 8 | 2.8780 ms / 2.8836 ms / 2.8918 ms | 34.581 Melem/s / 34.679 Melem/s / 34.746 Melem/s |
| 4 | 4 | 100000 | 64 | 3.0103 ms / 3.0308 ms / 3.0576 ms | 32.706 Melem/s / 32.995 Melem/s / 33.220 Melem/s |
| 4 | 4 | 100000 | 512 | 3.5409 ms / 3.5752 ms / 3.6428 ms | 27.451 Melem/s / 27.970 Melem/s / 28.241 Melem/s |
| 4 | 4 | 1000000 | 8 | 28.639 ms / 28.765 ms / 28.954 ms | 34.538 Melem/s / 34.764 Melem/s / 34.917 Melem/s |
| 4 | 4 | 1000000 | 64 | 29.946 ms / 30.154 ms / 30.495 ms | 32.792 Melem/s / 33.164 Melem/s / 33.393 Melem/s |
| 4 | 4 | 1000000 | 512 | 35.268 ms / 35.451 ms / 35.797 ms | 27.936 Melem/s / 28.208 Melem/s / 28.354 Melem/s |
| 4 | 4 | 10000000 | 8 | 286.28 ms / 287.73 ms / 289.75 ms | 34.513 Melem/s / 34.755 Melem/s / 34.931 Melem/s |
| 4 | 4 | 10000000 | 64 | 299.27 ms / 301.32 ms / 303.28 ms | 32.973 Melem/s / 33.187 Melem/s / 33.414 Melem/s |
| 4 | 4 | 10000000 | 512 | 350.24 ms / 354.75 ms / 359.15 ms | 27.843 Melem/s / 28.189 Melem/s / 28.552 Melem/s |
| 4 | 14 | 100000 | 8 | 2.9363 ms / 2.9482 ms / 2.9557 ms | 33.833 Melem/s / 33.919 Melem/s / 34.057 Melem/s |
| 4 | 14 | 100000 | 64 | 3.0696 ms / 3.0983 ms / 3.1534 ms | 31.712 Melem/s / 32.276 Melem/s / 32.577 Melem/s |
| 4 | 14 | 100000 | 512 | 3.8126 ms / 3.8427 ms / 3.9103 ms | 25.573 Melem/s / 26.023 Melem/s / 26.229 Melem/s |
| 4 | 14 | 1000000 | 8 | 29.211 ms / 29.445 ms / 29.895 ms | 33.451 Melem/s / 33.962 Melem/s / 34.233 Melem/s |
| 4 | 14 | 1000000 | 64 | 30.500 ms / 30.745 ms / 30.961 ms | 32.298 Melem/s / 32.525 Melem/s / 32.787 Melem/s |
| 4 | 14 | 1000000 | 512 | 37.652 ms / 37.984 ms / 38.561 ms | 25.933 Melem/s / 26.327 Melem/s / 26.559 Melem/s |
| 4 | 14 | 10000000 | 8 | 290.04 ms / 291.10 ms / 292.48 ms | 34.191 Melem/s / 34.352 Melem/s / 34.477 Melem/s |
| 4 | 14 | 10000000 | 64 | 301.96 ms / 303.72 ms / 305.73 ms | 32.709 Melem/s / 32.925 Melem/s / 33.117 Melem/s |
| 4 | 14 | 10000000 | 512 | 370.84 ms / 378.45 ms / 385.10 ms | 25.967 Melem/s / 26.423 Melem/s / 26.966 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedAsyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 100000 | 8 | 777.15 µs / 785.05 µs / 788.71 µs | 126.79 Melem/s / 127.38 Melem/s / 128.68 Melem/s |
| 128 | 1 | 100000 | 64 | 490.28 µs / 492.42 µs / 496.03 µs | 201.60 Melem/s / 203.08 Melem/s / 203.97 Melem/s |
| 128 | 1 | 100000 | 512 | 442.61 µs / 444.74 µs / 451.80 µs | 221.34 Melem/s / 224.85 Melem/s / 225.93 Melem/s |
| 128 | 1 | 1000000 | 8 | 7.7331 ms / 7.8123 ms / 7.9702 ms | 125.47 Melem/s / 128.00 Melem/s / 129.31 Melem/s |
| 128 | 1 | 1000000 | 64 | 4.7787 ms / 4.8105 ms / 4.8800 ms | 204.92 Melem/s / 207.88 Melem/s / 209.26 Melem/s |
| 128 | 1 | 1000000 | 512 | 4.3525 ms / 4.3785 ms / 4.4452 ms | 224.96 Melem/s / 228.39 Melem/s / 229.75 Melem/s |
| 128 | 1 | 10000000 | 8 | 79.250 ms / 79.773 ms / 80.363 ms | 124.43 Melem/s / 125.36 Melem/s / 126.18 Melem/s |
| 128 | 1 | 10000000 | 64 | 47.567 ms / 47.849 ms / 48.313 ms | 206.99 Melem/s / 208.99 Melem/s / 210.23 Melem/s |
| 128 | 1 | 10000000 | 512 | 43.067 ms / 43.161 ms / 43.259 ms | 231.16 Melem/s / 231.69 Melem/s / 232.20 Melem/s |
| 128 | 4 | 100000 | 8 | 892.06 µs / 898.18 µs / 902.76 µs | 110.77 Melem/s / 111.34 Melem/s / 112.10 Melem/s |
| 128 | 4 | 100000 | 64 | 497.87 µs / 498.73 µs / 499.84 µs | 200.07 Melem/s / 200.51 Melem/s / 200.86 Melem/s |
| 128 | 4 | 100000 | 512 | 449.93 µs / 450.90 µs / 452.38 µs | 221.05 Melem/s / 221.78 Melem/s / 222.26 Melem/s |
| 128 | 4 | 1000000 | 8 | 8.3759 ms / 8.4413 ms / 8.5043 ms | 117.59 Melem/s / 118.47 Melem/s / 119.39 Melem/s |
| 128 | 4 | 1000000 | 64 | 4.7965 ms / 4.8116 ms / 4.8233 ms | 207.33 Melem/s / 207.83 Melem/s / 208.49 Melem/s |
| 128 | 4 | 1000000 | 512 | 4.3766 ms / 4.3943 ms / 4.4468 ms | 224.88 Melem/s / 227.57 Melem/s / 228.49 Melem/s |
| 128 | 4 | 10000000 | 8 | 84.207 ms / 85.393 ms / 86.277 ms | 115.91 Melem/s / 117.11 Melem/s / 118.76 Melem/s |
| 128 | 4 | 10000000 | 64 | 47.824 ms / 48.001 ms / 48.120 ms | 207.81 Melem/s / 208.33 Melem/s / 209.10 Melem/s |
| 128 | 4 | 10000000 | 512 | 43.431 ms / 43.503 ms / 43.594 ms | 229.39 Melem/s / 229.87 Melem/s / 230.25 Melem/s |
| 128 | 14 | 100000 | 8 | 940.51 µs / 944.63 µs / 948.56 µs | 105.42 Melem/s / 105.86 Melem/s / 106.32 Melem/s |
| 128 | 14 | 100000 | 64 | 510.23 µs / 513.32 µs / 519.24 µs | 192.59 Melem/s / 194.81 Melem/s / 195.99 Melem/s |
| 128 | 14 | 100000 | 512 | 467.17 µs / 468.40 µs / 469.40 µs | 213.04 Melem/s / 213.49 Melem/s / 214.06 Melem/s |
| 128 | 14 | 1000000 | 8 | 8.5243 ms / 8.5753 ms / 8.6345 ms | 115.81 Melem/s / 116.61 Melem/s / 117.31 Melem/s |
| 128 | 14 | 1000000 | 64 | 4.8089 ms / 4.8315 ms / 4.8459 ms | 206.36 Melem/s / 206.98 Melem/s / 207.95 Melem/s |
| 128 | 14 | 1000000 | 512 | 4.4882 ms / 4.4949 ms / 4.5044 ms | 222.01 Melem/s / 222.48 Melem/s / 222.81 Melem/s |
| 128 | 14 | 10000000 | 8 | 84.435 ms / 85.608 ms / 86.698 ms | 115.34 Melem/s / 116.81 Melem/s / 118.43 Melem/s |
| 128 | 14 | 10000000 | 64 | 47.767 ms / 48.067 ms / 48.471 ms | 206.31 Melem/s / 208.04 Melem/s / 209.35 Melem/s |
| 128 | 14 | 10000000 | 512 | 44.477 ms / 44.681 ms / 45.056 ms | 221.95 Melem/s / 223.81 Melem/s / 224.83 Melem/s |

**Notes.** At `Cap-128` throughput rises with batch size: 108-127 Melem/s at `Batch-8`, 194-212 at `Batch-64` and 208-225 at `Batch-512`, close to flat across 1, 4 and 14 producers. At `Cap-1` and `Cap-4`, `Batch-512` is 18-30% below `Batch-8`.
