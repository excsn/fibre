# Fibre Benchmark: `MpmcV2 Cap 0`
**Test Machine:** MacBook M4 Pro

## Results

**SYNC**

`MpmcRendezvousSync/Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 175.08 ms / 178.63 ms / 180.40 ms | 554.33 Kelem/s / 559.83 Kelem/s / 571.17 Kelem/s |
| 1 | 1 | 1000000 | 1.7680 s / 1.7881 s / 1.8089 s | 552.81 Kelem/s / 559.25 Kelem/s / 565.62 Kelem/s |
| 1 | 4 | 100000 | 154.59 ms / 159.19 ms / 164.44 ms | 608.14 Kelem/s / 628.20 Kelem/s / 646.85 Kelem/s |
| 1 | 4 | 1000000 | 1.5451 s / 1.5665 s / 1.5901 s | 628.87 Kelem/s / 638.35 Kelem/s / 647.23 Kelem/s |
| 1 | 14 | 100000 | 150.80 ms / 156.58 ms / 162.34 ms | 616.00 Kelem/s / 638.64 Kelem/s / 663.15 Kelem/s |
| 1 | 14 | 1000000 | 1.5693 s / 1.5938 s / 1.6174 s | 618.26 Kelem/s / 627.44 Kelem/s / 637.21 Kelem/s |
| 4 | 1 | 100000 | 156.81 ms / 157.67 ms / 159.20 ms | 628.15 Kelem/s / 634.22 Kelem/s / 637.72 Kelem/s |
| 4 | 1 | 1000000 | 1.5179 s / 1.5371 s / 1.5582 s | 641.79 Kelem/s / 650.59 Kelem/s / 658.80 Kelem/s |
| 4 | 4 | 100000 | 160.34 ms / 162.19 ms / 164.39 ms | 608.32 Kelem/s / 616.56 Kelem/s / 623.67 Kelem/s |
| 4 | 4 | 1000000 | 1.6040 s / 1.6159 s / 1.6294 s | 613.72 Kelem/s / 618.85 Kelem/s / 623.45 Kelem/s |
| 4 | 14 | 100000 | 157.38 ms / 158.38 ms / 160.55 ms | 622.84 Kelem/s / 631.41 Kelem/s / 635.42 Kelem/s |
| 4 | 14 | 1000000 | 1.5744 s / 1.5809 s / 1.5912 s | 628.47 Kelem/s / 632.55 Kelem/s / 635.18 Kelem/s |
| 14 | 1 | 100000 | 144.80 ms / 146.73 ms / 149.47 ms | 669.04 Kelem/s / 681.54 Kelem/s / 690.61 Kelem/s |
| 14 | 1 | 1000000 | 1.4357 s / 1.4692 s / 1.5068 s | 663.68 Kelem/s / 680.66 Kelem/s / 696.52 Kelem/s |
| 14 | 4 | 100000 | 155.85 ms / 156.79 ms / 157.89 ms | 633.33 Kelem/s / 637.81 Kelem/s / 641.62 Kelem/s |
| 14 | 4 | 1000000 | 1.5564 s / 1.5678 s / 1.5822 s | 632.04 Kelem/s / 637.82 Kelem/s / 642.50 Kelem/s |
| 14 | 14 | 100000 | 149.34 ms / 151.73 ms / 154.69 ms | 646.45 Kelem/s / 659.05 Kelem/s / 669.62 Kelem/s |
| 14 | 14 | 1000000 | 1.5056 s / 1.5181 s / 1.5293 s | 653.89 Kelem/s / 658.72 Kelem/s / 664.21 Kelem/s |

---

**ASYNC**

`MpmcRendezvousAsync/Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 3.4484 ms / 3.4646 ms / 3.4948 ms | 28.614 Melem/s / 28.863 Melem/s / 28.999 Melem/s |
| 1 | 1 | 1000000 | 34.332 ms / 34.431 ms / 34.583 ms | 28.916 Melem/s / 29.044 Melem/s / 29.127 Melem/s |
| 1 | 4 | 100000 | 8.7570 ms / 8.9299 ms / 9.0678 ms | 11.028 Melem/s / 11.198 Melem/s / 11.419 Melem/s |
| 1 | 4 | 1000000 | 84.719 ms / 86.925 ms / 88.911 ms | 11.247 Melem/s / 11.504 Melem/s / 11.804 Melem/s |
| 1 | 14 | 100000 | 9.9681 ms / 10.029 ms / 10.104 ms | 9.8967 Melem/s / 9.9709 Melem/s / 10.032 Melem/s |
| 1 | 14 | 1000000 | 98.995 ms / 100.04 ms / 101.32 ms | 9.8697 Melem/s / 9.9957 Melem/s / 10.102 Melem/s |
| 4 | 1 | 100000 | 8.7197 ms / 8.7537 ms / 8.8163 ms | 11.343 Melem/s / 11.424 Melem/s / 11.468 Melem/s |
| 4 | 1 | 1000000 | 87.165 ms / 87.816 ms / 88.340 ms | 11.320 Melem/s / 11.387 Melem/s / 11.473 Melem/s |
| 4 | 4 | 100000 | 11.859 ms / 11.923 ms / 11.989 ms | 8.3408 Melem/s / 8.3869 Melem/s / 8.4325 Melem/s |
| 4 | 4 | 1000000 | 115.00 ms / 119.28 ms / 121.45 ms | 8.2335 Melem/s / 8.3834 Melem/s / 8.6953 Melem/s |
| 4 | 14 | 100000 | 18.798 ms / 19.541 ms / 19.893 ms | 5.0270 Melem/s / 5.1175 Melem/s / 5.3198 Melem/s |
| 4 | 14 | 1000000 | 195.94 ms / 197.91 ms / 199.39 ms | 5.0152 Melem/s / 5.0528 Melem/s / 5.1036 Melem/s |
| 14 | 1 | 100000 | 11.475 ms / 11.537 ms / 11.609 ms | 8.6138 Melem/s / 8.6676 Melem/s / 8.7143 Melem/s |
| 14 | 1 | 1000000 | 115.28 ms / 115.86 ms / 116.40 ms | 8.5909 Melem/s / 8.6311 Melem/s / 8.6745 Melem/s |
| 14 | 4 | 100000 | 20.286 ms / 20.537 ms / 20.771 ms | 4.8144 Melem/s / 4.8693 Melem/s / 4.9296 Melem/s |
| 14 | 4 | 1000000 | 195.42 ms / 202.93 ms / 208.85 ms | 4.7882 Melem/s / 4.9278 Melem/s / 5.1171 Melem/s |
| 14 | 14 | 100000 | 12.045 ms / 12.132 ms / 12.242 ms | 8.1687 Melem/s / 8.2428 Melem/s / 8.3023 Melem/s |
| 14 | 14 | 1000000 | 121.05 ms / 121.71 ms / 122.16 ms | 8.1861 Melem/s / 8.2163 Melem/s / 8.2614 Melem/s |
