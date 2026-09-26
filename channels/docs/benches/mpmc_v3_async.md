# Fibre Benchmark: `MpmcV3 Async`
**Test Machine:** MacBook M4 Pro

## Results

`MpmcAsync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 1 | 100000 | 6.3424 ms / 6.3990 ms / 6.4301 ms | 15.552 Melem/s / 15.627 Melem/s / 15.767 Melem/s |
| 4 | 1 | 1 | 1000000 | 61.880 ms / 63.215 ms / 63.859 ms | 15.659 Melem/s / 15.819 Melem/s / 16.160 Melem/s |
| 4 | 1 | 4 | 100000 | 10.201 ms / 10.269 ms / 10.362 ms | 9.6504 Melem/s / 9.7380 Melem/s / 9.8026 Melem/s |
| 4 | 1 | 4 | 1000000 | 96.806 ms / 98.137 ms / 99.097 ms | 10.091 Melem/s / 10.190 Melem/s / 10.330 Melem/s |
| 4 | 1 | 14 | 100000 | 16.959 ms / 16.997 ms / 17.033 ms | 5.8709 Melem/s / 5.8834 Melem/s / 5.8967 Melem/s |
| 4 | 1 | 14 | 1000000 | 168.88 ms / 169.28 ms / 169.99 ms | 5.8826 Melem/s / 5.9073 Melem/s / 5.9214 Melem/s |
| 4 | 4 | 1 | 100000 | 9.9956 ms / 10.042 ms / 10.081 ms | 9.9201 Melem/s / 9.9582 Melem/s / 10.004 Melem/s |
| 4 | 4 | 1 | 1000000 | 106.71 ms / 107.29 ms / 108.30 ms | 9.2332 Melem/s / 9.3206 Melem/s / 9.3710 Melem/s |
| 4 | 4 | 4 | 100000 | 14.464 ms / 14.970 ms / 15.683 ms | 6.3765 Melem/s / 6.6801 Melem/s / 6.9138 Melem/s |
| 4 | 4 | 4 | 1000000 | 137.13 ms / 141.80 ms / 147.14 ms | 6.7961 Melem/s / 7.0522 Melem/s / 7.2923 Melem/s |
| 4 | 4 | 14 | 100000 | 44.385 ms / 44.610 ms / 44.870 ms | 2.2287 Melem/s / 2.2416 Melem/s / 2.2530 Melem/s |
| 4 | 4 | 14 | 1000000 | 445.10 ms / 447.80 ms / 450.41 ms | 2.2202 Melem/s / 2.2331 Melem/s / 2.2467 Melem/s |
| 4 | 14 | 1 | 100000 | 18.881 ms / 18.948 ms / 19.030 ms | 5.2548 Melem/s / 5.2776 Melem/s / 5.2964 Melem/s |
| 4 | 14 | 1 | 1000000 | 190.32 ms / 190.83 ms / 191.37 ms | 5.2254 Melem/s / 5.2403 Melem/s / 5.2542 Melem/s |
| 4 | 14 | 4 | 100000 | 40.372 ms / 40.740 ms / 41.076 ms | 2.4345 Melem/s / 2.4546 Melem/s / 2.4770 Melem/s |
| 4 | 14 | 4 | 1000000 | 402.17 ms / 407.43 ms / 412.56 ms | 2.4239 Melem/s / 2.4544 Melem/s / 2.4865 Melem/s |
| 4 | 14 | 14 | 100000 | 43.388 ms / 43.865 ms / 44.200 ms | 2.2625 Melem/s / 2.2797 Melem/s / 2.3048 Melem/s |
| 4 | 14 | 14 | 1000000 | 441.91 ms / 444.80 ms / 447.71 ms | 2.2336 Melem/s / 2.2482 Melem/s / 2.2629 Melem/s |
| 128 | 1 | 1 | 100000 | 1.3980 ms / 1.4354 ms / 1.4597 ms | 68.505 Melem/s / 69.669 Melem/s / 71.532 Melem/s |
| 128 | 1 | 1 | 1000000 | 15.486 ms / 15.987 ms / 16.438 ms | 60.833 Melem/s / 62.552 Melem/s / 64.572 Melem/s |
| 128 | 1 | 4 | 100000 | 9.9059 ms / 11.186 ms / 12.652 ms | 7.9041 Melem/s / 8.9401 Melem/s / 10.095 Melem/s |
| 128 | 1 | 4 | 1000000 | 91.525 ms / 93.847 ms / 96.048 ms | 10.411 Melem/s / 10.656 Melem/s / 10.926 Melem/s |
| 128 | 1 | 14 | 100000 | 13.671 ms / 13.768 ms / 13.859 ms | 7.2155 Melem/s / 7.2634 Melem/s / 7.3148 Melem/s |
| 128 | 1 | 14 | 1000000 | 94.377 ms / 112.03 ms / 136.19 ms | 7.3424 Melem/s / 8.9258 Melem/s / 10.596 Melem/s |
| 128 | 4 | 1 | 100000 | 9.8984 ms / 10.232 ms / 10.620 ms | 9.4160 Melem/s / 9.7733 Melem/s / 10.103 Melem/s |
| 128 | 4 | 1 | 1000000 | 95.431 ms / 99.584 ms / 104.26 ms | 9.5917 Melem/s / 10.042 Melem/s / 10.479 Melem/s |
| 128 | 4 | 4 | 100000 | 16.307 ms / 16.651 ms / 17.138 ms | 5.8350 Melem/s / 6.0058 Melem/s / 6.1325 Melem/s |
| 128 | 4 | 4 | 1000000 | 64.541 ms / 72.650 ms / 81.847 ms | 12.218 Melem/s / 13.765 Melem/s / 15.494 Melem/s |
| 128 | 4 | 14 | 100000 | 29.333 ms / 29.543 ms / 29.830 ms | 3.3523 Melem/s / 3.3849 Melem/s / 3.4092 Melem/s |
| 128 | 4 | 14 | 1000000 | 300.20 ms / 302.29 ms / 303.90 ms | 3.2906 Melem/s / 3.3081 Melem/s / 3.3312 Melem/s |
| 128 | 14 | 1 | 100000 | 20.984 ms / 22.071 ms / 23.575 ms | 4.2417 Melem/s / 4.5309 Melem/s / 4.7656 Melem/s |
| 128 | 14 | 1 | 1000000 | 220.90 ms / 230.39 ms / 240.15 ms | 4.1641 Melem/s / 4.3406 Melem/s / 4.5270 Melem/s |
| 128 | 14 | 4 | 100000 | 47.215 ms / 48.038 ms / 48.814 ms | 2.0486 Melem/s / 2.0817 Melem/s / 2.1180 Melem/s |
| 128 | 14 | 4 | 1000000 | 511.93 ms / 515.05 ms / 518.01 ms | 1.9305 Melem/s / 1.9415 Melem/s / 1.9534 Melem/s |
| 128 | 14 | 14 | 100000 | 48.705 ms / 49.052 ms / 49.428 ms | 2.0232 Melem/s / 2.0387 Melem/s / 2.0532 Melem/s |
| 128 | 14 | 14 | 1000000 | 478.76 ms / 488.69 ms / 498.14 ms | 2.0075 Melem/s / 2.0463 Melem/s / 2.0887 Melem/s |
