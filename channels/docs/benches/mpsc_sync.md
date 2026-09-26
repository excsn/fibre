# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

`MpscUnboundedSync/Prod-{Prod}_Items-{Items}`

| Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| 1 | 100000 | 508.31 µs / 544.11 µs / 587.66 µs | 170.17 Melem/s / 183.79 Melem/s / 196.73 Melem/s |
| 1 | 1000000 | 4.2894 ms / 4.3468 ms / 4.4086 ms | 226.83 Melem/s / 230.05 Melem/s / 233.13 Melem/s |
| 1 | 10000000 | 42.778 ms / 43.375 ms / 44.269 ms | 225.89 Melem/s / 230.55 Melem/s / 233.76 Melem/s |
| 4 | 100000 | 5.0459 ms / 5.4986 ms / 5.9513 ms | 16.803 Melem/s / 18.187 Melem/s / 19.818 Melem/s |
| 4 | 1000000 | 49.570 ms / 52.043 ms / 56.072 ms | 17.834 Melem/s / 19.215 Melem/s / 20.173 Melem/s |
| 4 | 10000000 | 478.82 ms / 501.97 ms / 528.37 ms | 18.926 Melem/s / 19.921 Melem/s / 20.884 Melem/s |
| 14 | 100000 | 7.1477 ms / 7.1614 ms / 7.1730 ms | 13.941 Melem/s / 13.964 Melem/s / 13.991 Melem/s |
| 14 | 1000000 | 76.632 ms / 76.831 ms / 76.953 ms | 12.995 Melem/s / 13.016 Melem/s / 13.049 Melem/s |
| 14 | 10000000 | 771.19 ms / 773.42 ms / 775.56 ms | 12.894 Melem/s / 12.930 Melem/s / 12.967 Melem/s |

## Bounded Results (`MpscBoundedSync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 13.946 ms / 14.172 ms / 14.365 ms | 6.9612 Melem/s / 7.0560 Melem/s / 7.1705 Melem/s |
| 1 | 1 | 1000000 | 140.12 ms / 152.45 ms / 167.84 ms | 5.9581 Melem/s / 6.5593 Melem/s / 7.1368 Melem/s |
| 1 | 1 | 10000000 | 1.4355 s / 1.4971 s / 1.5662 s | 6.3848 Melem/s / 6.6795 Melem/s / 6.9663 Melem/s |
| 1 | 4 | 100000 | 36.692 ms / 40.691 ms / 47.295 ms | 2.1144 Melem/s / 2.4575 Melem/s / 2.7254 Melem/s |
| 1 | 4 | 1000000 | 370.86 ms / 389.37 ms / 415.61 ms | 2.4061 Melem/s / 2.5682 Melem/s / 2.6965 Melem/s |
| 1 | 4 | 10000000 | 3.9009 s / 4.0279 s / 4.1747 s | 2.3954 Melem/s / 2.4827 Melem/s / 2.5635 Melem/s |
| 1 | 14 | 100000 | 82.520 ms / 82.844 ms / 83.111 ms | 1.2032 Melem/s / 1.2071 Melem/s / 1.2118 Melem/s |
| 1 | 14 | 1000000 | 822.65 ms / 826.78 ms / 830.71 ms | 1.2038 Melem/s / 1.2095 Melem/s / 1.2156 Melem/s |
| 1 | 14 | 10000000 | 8.2672 s / 8.2838 s / 8.2990 s | 1.2050 Melem/s / 1.2072 Melem/s / 1.2096 Melem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 4 | 1 | 100000 | 6.9281 ms / 7.0169 ms / 7.1189 ms | 14.047 Melem/s / 14.251 Melem/s / 14.434 Melem/s |
| 4 | 1 | 1000000 | 68.764 ms / 70.970 ms / 72.617 ms | 13.771 Melem/s / 14.090 Melem/s / 14.542 Melem/s |
| 4 | 1 | 10000000 | 676.28 ms / 721.02 ms / 772.14 ms | 12.951 Melem/s / 13.869 Melem/s / 14.787 Melem/s |
| 4 | 4 | 100000 | 12.652 ms / 13.080 ms / 13.699 ms | 7.2995 Melem/s / 7.6451 Melem/s / 7.9041 Melem/s |
| 4 | 4 | 1000000 | 136.71 ms / 144.99 ms / 153.87 ms | 6.4988 Melem/s / 6.8972 Melem/s / 7.3145 Melem/s |
| 4 | 4 | 10000000 | 1.3160 s / 1.3495 s / 1.3862 s | 7.2141 Melem/s / 7.4101 Melem/s / 7.5990 Melem/s |
| 4 | 14 | 100000 | 30.554 ms / 31.037 ms / 31.546 ms | 3.1699 Melem/s / 3.2220 Melem/s / 3.2729 Melem/s |
| 4 | 14 | 1000000 | 313.43 ms / 315.24 ms / 317.48 ms | 3.1498 Melem/s / 3.1722 Melem/s / 3.1905 Melem/s |
| 4 | 14 | 10000000 | 3.0650 s / 3.1155 s / 3.1536 s | 3.1710 Melem/s / 3.2098 Melem/s / 3.2627 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedSync/Cap-{Cap}_Prod-{Prod}_Items-{Items}`

| Cap | Prod | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 128 | 1 | 100000 | 1.3388 ms / 1.3854 ms / 1.4165 ms | 70.598 Melem/s / 72.183 Melem/s / 74.694 Melem/s |
| 128 | 1 | 1000000 | 13.487 ms / 14.129 ms / 14.858 ms | 67.304 Melem/s / 70.779 Melem/s / 74.145 Melem/s |
| 128 | 1 | 10000000 | 134.32 ms / 134.94 ms / 135.56 ms | 73.768 Melem/s / 74.108 Melem/s / 74.447 Melem/s |
| 128 | 4 | 100000 | 2.4480 ms / 2.4725 ms / 2.4955 ms | 40.071 Melem/s / 40.445 Melem/s / 40.850 Melem/s |
| 128 | 4 | 1000000 | 25.146 ms / 26.376 ms / 29.170 ms | 34.282 Melem/s / 37.913 Melem/s / 39.768 Melem/s |
| 128 | 4 | 10000000 | 243.12 ms / 248.91 ms / 255.37 ms | 39.158 Melem/s / 40.174 Melem/s / 41.131 Melem/s |
| 128 | 14 | 100000 | 2.7181 ms / 2.7862 ms / 2.8978 ms | 34.509 Melem/s / 35.891 Melem/s / 36.790 Melem/s |
| 128 | 14 | 1000000 | 26.262 ms / 26.720 ms / 27.432 ms | 36.454 Melem/s / 37.425 Melem/s / 38.078 Melem/s |
| 128 | 14 | 10000000 | 260.68 ms / 282.49 ms / 314.19 ms | 31.828 Melem/s / 35.399 Melem/s / 38.361 Melem/s |

## Bounded Batch Results (`MpscBoundedSyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut`, the
consumer drains via `recv_batch_mut`, both reusing a caller-owned buffer (allocation-free
after the drain-in-place `send_batch_mut`). Batch sizes 8, 64 and 512._

### Capacity: 1 (`Cap-1`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 1 | 1 | 100000 | 8 | 27.090 ms / 27.346 ms / 27.667 ms | 3.6144 Melem/s / 3.6569 Melem/s / 3.6915 Melem/s |
| 1 | 1 | 100000 | 64 | 27.379 ms / 27.931 ms / 29.151 ms | 3.4304 Melem/s / 3.5803 Melem/s / 3.6524 Melem/s |
| 1 | 1 | 100000 | 512 | 26.707 ms / 26.844 ms / 27.123 ms | 3.6870 Melem/s / 3.7252 Melem/s / 3.7443 Melem/s |
| 1 | 1 | 1000000 | 8 | 267.31 ms / 273.21 ms / 282.17 ms | 3.5440 Melem/s / 3.6602 Melem/s / 3.7410 Melem/s |
| 1 | 1 | 1000000 | 64 | 267.07 ms / 270.33 ms / 275.97 ms | 3.6236 Melem/s / 3.6991 Melem/s / 3.7444 Melem/s |
| 1 | 1 | 1000000 | 512 | 272.65 ms / 279.23 ms / 286.71 ms | 3.4878 Melem/s / 3.5812 Melem/s / 3.6678 Melem/s |
| 1 | 1 | 10000000 | 8 | 2.6587 s / 2.6988 s / 2.7448 s | 3.6432 Melem/s / 3.7054 Melem/s / 3.7612 Melem/s |
| 1 | 1 | 10000000 | 64 | 2.6474 s / 2.6891 s / 2.7340 s | 3.6577 Melem/s / 3.7188 Melem/s / 3.7773 Melem/s |
| 1 | 1 | 10000000 | 512 | 2.6354 s / 2.6754 s / 2.7238 s | 3.6713 Melem/s / 3.7377 Melem/s / 3.7944 Melem/s |
| 1 | 4 | 100000 | 8 | 152.35 ms / 156.23 ms / 162.01 ms | 617.24 Kelem/s / 640.08 Kelem/s / 656.36 Kelem/s |
| 1 | 4 | 100000 | 64 | 152.08 ms / 155.08 ms / 160.21 ms | 624.18 Kelem/s / 644.82 Kelem/s / 657.57 Kelem/s |
| 1 | 4 | 100000 | 512 | 152.77 ms / 154.80 ms / 156.90 ms | 637.36 Kelem/s / 646.00 Kelem/s / 654.60 Kelem/s |
| 1 | 4 | 1000000 | 8 | 1.5158 s / 1.5449 s / 1.5787 s | 633.42 Kelem/s / 647.29 Kelem/s / 659.71 Kelem/s |
| 1 | 4 | 1000000 | 64 | 1.5275 s / 1.5633 s / 1.6081 s | 621.84 Kelem/s / 639.67 Kelem/s / 654.66 Kelem/s |
| 1 | 4 | 1000000 | 512 | 1.5503 s / 1.5721 s / 1.5994 s | 625.25 Kelem/s / 636.08 Kelem/s / 645.03 Kelem/s |
| 1 | 4 | 10000000 | 8 | 15.451 s / 15.754 s / 16.101 s | 621.08 Kelem/s / 634.78 Kelem/s / 647.20 Kelem/s |
| 1 | 4 | 10000000 | 64 | 15.559 s / 16.238 s / 17.231 s | 580.35 Kelem/s / 615.84 Kelem/s / 642.73 Kelem/s |
| 1 | 4 | 10000000 | 512 | 15.565 s / 15.691 s / 15.845 s | 631.10 Kelem/s / 637.29 Kelem/s / 642.48 Kelem/s |
| 1 | 14 | 100000 | 8 | 170.20 ms / 172.63 ms / 174.97 ms | 571.52 Kelem/s / 579.26 Kelem/s / 587.55 Kelem/s |
| 1 | 14 | 100000 | 64 | 173.26 ms / 176.87 ms / 180.29 ms | 554.68 Kelem/s / 565.40 Kelem/s / 577.15 Kelem/s |
| 1 | 14 | 100000 | 512 | 170.74 ms / 173.80 ms / 178.29 ms | 560.89 Kelem/s / 575.39 Kelem/s / 585.68 Kelem/s |
| 1 | 14 | 1000000 | 8 | 1.7723 s / 1.8167 s / 1.8631 s | 536.75 Kelem/s / 550.44 Kelem/s / 564.23 Kelem/s |
| 1 | 14 | 1000000 | 64 | 1.7093 s / 1.7514 s / 1.7962 s | 556.72 Kelem/s / 570.99 Kelem/s / 585.04 Kelem/s |
| 1 | 14 | 1000000 | 512 | 1.7190 s / 1.8671 s / 2.0966 s | 476.96 Kelem/s / 535.60 Kelem/s / 581.74 Kelem/s |
| 1 | 14 | 10000000 | 8 | 17.709 s / 18.264 s / 18.898 s | 529.17 Kelem/s / 547.53 Kelem/s / 564.67 Kelem/s |
| 1 | 14 | 10000000 | 64 | 17.506 s / 17.749 s / 18.025 s | 554.80 Kelem/s / 563.41 Kelem/s / 571.22 Kelem/s |
| 1 | 14 | 10000000 | 512 | 17.108 s / 17.207 s / 17.314 s | 577.58 Kelem/s / 581.17 Kelem/s / 584.53 Kelem/s |

### Capacity: 4 (`Cap-4`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 100000 | 8 | 17.843 ms / 18.432 ms / 19.060 ms | 5.2466 Melem/s / 5.4254 Melem/s / 5.6046 Melem/s |
| 4 | 1 | 100000 | 64 | 19.312 ms / 19.588 ms / 19.951 ms | 5.0122 Melem/s / 5.1051 Melem/s / 5.1783 Melem/s |
| 4 | 1 | 100000 | 512 | 18.790 ms / 19.288 ms / 19.827 ms | 5.0437 Melem/s / 5.1846 Melem/s / 5.3221 Melem/s |
| 4 | 1 | 1000000 | 8 | 174.77 ms / 179.43 ms / 185.13 ms | 5.4017 Melem/s / 5.5732 Melem/s / 5.7217 Melem/s |
| 4 | 1 | 1000000 | 64 | 195.19 ms / 198.55 ms / 202.36 ms | 4.9418 Melem/s / 5.0364 Melem/s / 5.1233 Melem/s |
| 4 | 1 | 1000000 | 512 | 187.98 ms / 191.78 ms / 196.13 ms | 5.0985 Melem/s / 5.2143 Melem/s / 5.3198 Melem/s |
| 4 | 1 | 10000000 | 8 | 1.7869 s / 1.8287 s / 1.8760 s | 5.3306 Melem/s / 5.4683 Melem/s / 5.5963 Melem/s |
| 4 | 1 | 10000000 | 64 | 1.9227 s / 1.9530 s / 1.9809 s | 5.0481 Melem/s / 5.1203 Melem/s / 5.2010 Melem/s |
| 4 | 1 | 10000000 | 512 | 1.9419 s / 1.9858 s / 2.0328 s | 4.9194 Melem/s / 5.0358 Melem/s / 5.1497 Melem/s |
| 4 | 4 | 100000 | 8 | 55.357 ms / 55.843 ms / 56.417 ms | 1.7725 Melem/s / 1.7907 Melem/s / 1.8064 Melem/s |
| 4 | 4 | 100000 | 64 | 57.459 ms / 57.984 ms / 58.324 ms | 1.7146 Melem/s / 1.7246 Melem/s / 1.7404 Melem/s |
| 4 | 4 | 100000 | 512 | 58.291 ms / 59.141 ms / 60.420 ms | 1.6551 Melem/s / 1.6909 Melem/s / 1.7155 Melem/s |
| 4 | 4 | 1000000 | 8 | 540.78 ms / 558.73 ms / 576.36 ms | 1.7350 Melem/s / 1.7898 Melem/s / 1.8492 Melem/s |
| 4 | 4 | 1000000 | 64 | 584.99 ms / 599.05 ms / 610.12 ms | 1.6390 Melem/s / 1.6693 Melem/s / 1.7094 Melem/s |
| 4 | 4 | 1000000 | 512 | 597.93 ms / 606.30 ms / 615.36 ms | 1.6251 Melem/s / 1.6494 Melem/s / 1.6724 Melem/s |
| 4 | 4 | 10000000 | 8 | 5.7052 s / 5.7852 s / 5.8609 s | 1.7062 Melem/s / 1.7285 Melem/s / 1.7528 Melem/s |
| 4 | 4 | 10000000 | 64 | 6.0784 s / 6.1290 s / 6.1787 s | 1.6185 Melem/s / 1.6316 Melem/s / 1.6452 Melem/s |
| 4 | 4 | 10000000 | 512 | 6.1571 s / 6.2156 s / 6.2835 s | 1.5915 Melem/s / 1.6088 Melem/s / 1.6241 Melem/s |
| 4 | 14 | 100000 | 8 | 56.145 ms / 56.956 ms / 57.975 ms | 1.7249 Melem/s / 1.7558 Melem/s / 1.7811 Melem/s |
| 4 | 14 | 100000 | 64 | 63.172 ms / 64.232 ms / 65.958 ms | 1.5161 Melem/s / 1.5569 Melem/s / 1.5830 Melem/s |
| 4 | 14 | 100000 | 512 | 64.851 ms / 65.466 ms / 65.936 ms | 1.5166 Melem/s / 1.5275 Melem/s / 1.5420 Melem/s |
| 4 | 14 | 1000000 | 8 | 565.76 ms / 580.28 ms / 599.48 ms | 1.6681 Melem/s / 1.7233 Melem/s / 1.7675 Melem/s |
| 4 | 14 | 1000000 | 64 | 646.14 ms / 662.69 ms / 680.54 ms | 1.4694 Melem/s / 1.5090 Melem/s / 1.5476 Melem/s |
| 4 | 14 | 1000000 | 512 | 601.72 ms / 659.53 ms / 709.45 ms | 1.4095 Melem/s / 1.5162 Melem/s / 1.6619 Melem/s |
| 4 | 14 | 10000000 | 8 | 5.8270 s / 5.8816 s / 5.9466 s | 1.6816 Melem/s / 1.7002 Melem/s / 1.7162 Melem/s |
| 4 | 14 | 10000000 | 64 | 6.5224 s / 6.6678 s / 6.8898 s | 1.4514 Melem/s / 1.4997 Melem/s / 1.5332 Melem/s |
| 4 | 14 | 10000000 | 512 | 6.7089 s / 6.7447 s / 6.7863 s | 1.4736 Melem/s / 1.4826 Melem/s / 1.4906 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpscBoundedSyncBatch/Cap-{Cap}_Prod-{Prod}_Items-{Items}_Batch-{Batch}`

| Cap | Prod | Items | Batch | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 100000 | 8 | 1.6130 ms / 1.6308 ms / 1.6634 ms | 60.118 Melem/s / 61.320 Melem/s / 61.997 Melem/s |
| 128 | 1 | 100000 | 64 | 1.6747 ms / 1.6825 ms / 1.7001 ms | 58.821 Melem/s / 59.436 Melem/s / 59.712 Melem/s |
| 128 | 1 | 100000 | 512 | 3.2711 ms / 3.3579 ms / 3.4145 ms | 29.287 Melem/s / 29.781 Melem/s / 30.571 Melem/s |
| 128 | 1 | 1000000 | 8 | 16.527 ms / 17.849 ms / 18.630 ms | 53.678 Melem/s / 56.026 Melem/s / 60.508 Melem/s |
| 128 | 1 | 1000000 | 64 | 17.287 ms / 17.546 ms / 17.757 ms | 56.317 Melem/s / 56.994 Melem/s / 57.846 Melem/s |
| 128 | 1 | 1000000 | 512 | 33.862 ms / 34.676 ms / 35.253 ms | 28.366 Melem/s / 28.838 Melem/s / 29.531 Melem/s |
| 128 | 1 | 10000000 | 8 | 165.96 ms / 170.34 ms / 173.18 ms | 57.744 Melem/s / 58.707 Melem/s / 60.257 Melem/s |
| 128 | 1 | 10000000 | 64 | 179.42 ms / 183.32 ms / 187.45 ms | 53.347 Melem/s / 54.550 Melem/s / 55.736 Melem/s |
| 128 | 1 | 10000000 | 512 | 330.31 ms / 334.47 ms / 338.10 ms | 29.577 Melem/s / 29.898 Melem/s / 30.274 Melem/s |
| 128 | 4 | 100000 | 8 | 1.9414 ms / 2.0792 ms / 2.2963 ms | 43.548 Melem/s / 48.094 Melem/s / 51.509 Melem/s |
| 128 | 4 | 100000 | 64 | 1.7568 ms / 1.8598 ms / 1.9443 ms | 51.432 Melem/s / 53.770 Melem/s / 56.922 Melem/s |
| 128 | 4 | 100000 | 512 | 3.4059 ms / 3.4397 ms / 3.4740 ms | 28.786 Melem/s / 29.072 Melem/s / 29.361 Melem/s |
| 128 | 4 | 1000000 | 8 | 17.304 ms / 17.459 ms / 17.552 ms | 56.975 Melem/s / 57.277 Melem/s / 57.790 Melem/s |
| 128 | 4 | 1000000 | 64 | 18.064 ms / 18.637 ms / 19.192 ms | 52.105 Melem/s / 53.656 Melem/s / 55.358 Melem/s |
| 128 | 4 | 1000000 | 512 | 33.156 ms / 33.494 ms / 33.868 ms | 29.526 Melem/s / 29.856 Melem/s / 30.161 Melem/s |
| 128 | 4 | 10000000 | 8 | 172.03 ms / 183.40 ms / 198.80 ms | 50.302 Melem/s / 54.525 Melem/s / 58.129 Melem/s |
| 128 | 4 | 10000000 | 64 | 174.42 ms / 187.19 ms / 203.06 ms | 49.248 Melem/s / 53.422 Melem/s / 57.332 Melem/s |
| 128 | 4 | 10000000 | 512 | 334.74 ms / 339.31 ms / 343.80 ms | 29.087 Melem/s / 29.472 Melem/s / 29.874 Melem/s |
| 128 | 14 | 100000 | 8 | 1.8837 ms / 1.8947 ms / 1.9149 ms | 52.223 Melem/s / 52.778 Melem/s / 53.086 Melem/s |
| 128 | 14 | 100000 | 64 | 1.9824 ms / 2.0531 ms / 2.1064 ms | 47.475 Melem/s / 48.707 Melem/s / 50.443 Melem/s |
| 128 | 14 | 100000 | 512 | 3.4589 ms / 3.5295 ms / 3.5814 ms | 27.922 Melem/s / 28.332 Melem/s / 28.911 Melem/s |
| 128 | 14 | 1000000 | 8 | 17.336 ms / 17.505 ms / 17.758 ms | 56.312 Melem/s / 57.127 Melem/s / 57.684 Melem/s |
| 128 | 14 | 1000000 | 64 | 17.455 ms / 17.772 ms / 18.245 ms | 54.810 Melem/s / 56.269 Melem/s / 57.289 Melem/s |
| 128 | 14 | 1000000 | 512 | 33.610 ms / 34.069 ms / 34.368 ms | 29.097 Melem/s / 29.352 Melem/s / 29.753 Melem/s |
| 128 | 14 | 10000000 | 8 | 175.32 ms / 177.62 ms / 179.34 ms | 55.759 Melem/s / 56.301 Melem/s / 57.038 Melem/s |
| 128 | 14 | 10000000 | 64 | 175.89 ms / 189.18 ms / 210.08 ms | 47.601 Melem/s / 52.859 Melem/s / 56.853 Melem/s |
| 128 | 14 | 10000000 | 512 | 338.10 ms / 344.07 ms / 349.17 ms | 28.639 Melem/s / 29.064 Melem/s / 29.577 Melem/s |

**Notes.** At `Cap-128`, `Batch-8` and `Batch-64` run at 48-61 Melem/s for 1, 4 and 14 producers. `Batch-512` runs at 28-30 Melem/s. `Cap-4` runs at 5.0-5.6 Melem/s with 1 producer and 1.5-1.8 with 4 or 14. `Cap-1` runs at 3.6-3.7 Melem/s with 1 producer and 0.54-0.65 with 4 or 14.
