# Fibre Benchmark: `MpmcV3 Sync`
**Test Machine:** MacBook M4 Pro

## Results

`MpmcSync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 1 | 100000 | 4.7583 ms / 4.8301 ms / 4.9922 ms | 20.031 Melem/s / 20.704 Melem/s / 21.016 Melem/s |
| 4 | 1 | 1 | 1000000 | 46.782 ms / 47.058 ms / 47.250 ms | 21.164 Melem/s / 21.250 Melem/s / 21.376 Melem/s |
| 4 | 1 | 4 | 100000 | 14.366 ms / 14.637 ms / 14.922 ms | 6.7015 Melem/s / 6.8319 Melem/s / 6.9609 Melem/s |
| 4 | 1 | 4 | 1000000 | 149.36 ms / 155.74 ms / 166.71 ms | 5.9984 Melem/s / 6.4209 Melem/s / 6.6954 Melem/s |
| 4 | 1 | 14 | 100000 | 57.837 ms / 58.199 ms / 58.653 ms | 1.7049 Melem/s / 1.7182 Melem/s / 1.7290 Melem/s |
| 4 | 1 | 14 | 1000000 | 583.00 ms / 585.60 ms / 587.98 ms | 1.7007 Melem/s / 1.7076 Melem/s / 1.7153 Melem/s |
| 4 | 4 | 1 | 100000 | 14.366 ms / 14.645 ms / 14.937 ms | 6.6949 Melem/s / 6.8284 Melem/s / 6.9607 Melem/s |
| 4 | 4 | 1 | 1000000 | 151.12 ms / 152.99 ms / 155.71 ms | 6.4222 Melem/s / 6.5365 Melem/s / 6.6174 Melem/s |
| 4 | 4 | 4 | 100000 | 29.117 ms / 29.257 ms / 29.463 ms | 3.3941 Melem/s / 3.4180 Melem/s / 3.4345 Melem/s |
| 4 | 4 | 4 | 1000000 | 287.74 ms / 292.74 ms / 297.24 ms | 3.3642 Melem/s / 3.4160 Melem/s / 3.4753 Melem/s |
| 4 | 4 | 14 | 100000 | 63.163 ms / 63.932 ms / 64.594 ms | 1.5481 Melem/s / 1.5642 Melem/s / 1.5832 Melem/s |
| 4 | 4 | 14 | 1000000 | 629.32 ms / 638.51 ms / 646.53 ms | 1.5467 Melem/s / 1.5661 Melem/s / 1.5890 Melem/s |
| 4 | 14 | 1 | 100000 | 62.176 ms / 62.413 ms / 62.785 ms | 1.5927 Melem/s / 1.6022 Melem/s / 1.6083 Melem/s |
| 4 | 14 | 1 | 1000000 | 621.93 ms / 629.62 ms / 636.29 ms | 1.5716 Melem/s / 1.5883 Melem/s / 1.6079 Melem/s |
| 4 | 14 | 4 | 100000 | 59.689 ms / 59.926 ms / 60.167 ms | 1.6620 Melem/s / 1.6687 Melem/s / 1.6753 Melem/s |
| 4 | 14 | 4 | 1000000 | 603.42 ms / 607.66 ms / 611.48 ms | 1.6354 Melem/s / 1.6456 Melem/s / 1.6572 Melem/s |
| 4 | 14 | 14 | 100000 | 62.871 ms / 63.726 ms / 64.397 ms | 1.5529 Melem/s / 1.5692 Melem/s / 1.5905 Melem/s |
| 4 | 14 | 14 | 1000000 | 624.62 ms / 634.67 ms / 641.93 ms | 1.5578 Melem/s / 1.5756 Melem/s / 1.6010 Melem/s |
| 128 | 1 | 1 | 100000 | 4.0237 ms / 4.0348 ms / 4.0477 ms | 24.706 Melem/s / 24.784 Melem/s / 24.853 Melem/s |
| 128 | 1 | 1 | 1000000 | 40.022 ms / 40.182 ms / 40.401 ms | 24.752 Melem/s / 24.887 Melem/s / 24.986 Melem/s |
| 128 | 1 | 4 | 100000 | 14.235 ms / 14.457 ms / 14.746 ms | 6.7817 Melem/s / 6.9170 Melem/s / 7.0249 Melem/s |
| 128 | 1 | 4 | 1000000 | 148.50 ms / 152.16 ms / 155.53 ms | 6.4296 Melem/s / 6.5719 Melem/s / 6.7339 Melem/s |
| 128 | 1 | 14 | 100000 | 58.795 ms / 59.249 ms / 59.653 ms | 1.6801 Melem/s / 1.6878 Melem/s / 1.7008 Melem/s |
| 128 | 1 | 14 | 1000000 | 597.83 ms / 599.45 ms / 601.15 ms | 1.6635 Melem/s / 1.6682 Melem/s / 1.6727 Melem/s |
| 128 | 4 | 1 | 100000 | 14.337 ms / 14.492 ms / 14.655 ms | 6.8238 Melem/s / 6.9001 Melem/s / 6.9748 Melem/s |
| 128 | 4 | 1 | 1000000 | 150.71 ms / 153.64 ms / 157.38 ms | 6.3540 Melem/s / 6.5085 Melem/s / 6.6353 Melem/s |
| 128 | 4 | 4 | 100000 | 28.875 ms / 29.016 ms / 29.186 ms | 3.4263 Melem/s / 3.4464 Melem/s / 3.4632 Melem/s |
| 128 | 4 | 4 | 1000000 | 278.43 ms / 284.01 ms / 289.42 ms | 3.4551 Melem/s / 3.5210 Melem/s / 3.5916 Melem/s |
| 128 | 4 | 14 | 100000 | 61.395 ms / 62.537 ms / 63.261 ms | 1.5807 Melem/s / 1.5990 Melem/s / 1.6288 Melem/s |
| 128 | 4 | 14 | 1000000 | 604.32 ms / 612.19 ms / 620.32 ms | 1.6121 Melem/s / 1.6335 Melem/s / 1.6547 Melem/s |
| 128 | 14 | 1 | 100000 | 54.474 ms / 54.777 ms / 55.252 ms | 1.8099 Melem/s / 1.8256 Melem/s / 1.8357 Melem/s |
| 128 | 14 | 1 | 1000000 | 552.86 ms / 562.16 ms / 568.31 ms | 1.7596 Melem/s / 1.7789 Melem/s / 1.8088 Melem/s |
| 128 | 14 | 4 | 100000 | 57.031 ms / 57.177 ms / 57.329 ms | 1.7443 Melem/s / 1.7490 Melem/s / 1.7534 Melem/s |
| 128 | 14 | 4 | 1000000 | 574.34 ms / 580.73 ms / 586.03 ms | 1.7064 Melem/s / 1.7220 Melem/s / 1.7411 Melem/s |
| 128 | 14 | 14 | 100000 | 60.831 ms / 61.648 ms / 62.270 ms | 1.6059 Melem/s / 1.6221 Melem/s / 1.6439 Melem/s |
| 128 | 14 | 14 | 1000000 | 610.11 ms / 617.62 ms / 622.98 ms | 1.6052 Melem/s / 1.6191 Melem/s / 1.6391 Melem/s |
