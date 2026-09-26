# Fibre Benchmark: `MpmcV2 Async`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpmcUnboundedAsync`)

`MpmcUnboundedAsync/Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 1.4288 ms / 1.4591 ms / 1.5086 ms | 66.287 Melem/s / 68.536 Melem/s / 69.990 Melem/s |
| 1 | 1 | 1000000 | 12.274 ms / 12.314 ms / 12.344 ms | 81.012 Melem/s / 81.211 Melem/s / 81.475 Melem/s |
| 1 | 4 | 100000 | 2.5041 ms / 2.5112 ms / 2.5218 ms | 39.655 Melem/s / 39.822 Melem/s / 39.934 Melem/s |
| 1 | 4 | 1000000 | 23.970 ms / 24.005 ms / 24.041 ms | 41.595 Melem/s / 41.658 Melem/s / 41.718 Melem/s |
| 1 | 14 | 100000 | 3.8596 ms / 3.9481 ms / 4.0048 ms | 24.970 Melem/s / 25.328 Melem/s / 25.910 Melem/s |
| 1 | 14 | 1000000 | 40.394 ms / 40.489 ms / 40.592 ms | 24.635 Melem/s / 24.698 Melem/s / 24.756 Melem/s |
| 4 | 1 | 100000 | 3.6758 ms / 3.7579 ms / 3.9300 ms | 25.445 Melem/s / 26.610 Melem/s / 27.205 Melem/s |
| 4 | 1 | 1000000 | 47.353 ms / 51.667 ms / 56.631 ms | 17.658 Melem/s / 19.355 Melem/s / 21.118 Melem/s |
| 4 | 4 | 100000 | 6.4854 ms / 6.8433 ms / 7.4364 ms | 13.447 Melem/s / 14.613 Melem/s / 15.419 Melem/s |
| 4 | 4 | 1000000 | 90.811 ms / 92.175 ms / 92.898 ms | 10.765 Melem/s / 10.849 Melem/s / 11.012 Melem/s |
| 4 | 14 | 100000 | 6.4115 ms / 6.6077 ms / 7.0243 ms | 14.236 Melem/s / 15.134 Melem/s / 15.597 Melem/s |
| 4 | 14 | 1000000 | 83.762 ms / 84.502 ms / 85.676 ms | 11.672 Melem/s / 11.834 Melem/s / 11.939 Melem/s |
| 14 | 1 | 100000 | 7.7411 ms / 7.7540 ms / 7.7703 ms | 12.870 Melem/s / 12.897 Melem/s / 12.918 Melem/s |
| 14 | 1 | 1000000 | 78.872 ms / 79.374 ms / 79.727 ms | 12.543 Melem/s / 12.599 Melem/s / 12.679 Melem/s |
| 14 | 4 | 100000 | 8.6192 ms / 8.6891 ms / 8.7242 ms | 11.462 Melem/s / 11.509 Melem/s / 11.602 Melem/s |
| 14 | 4 | 1000000 | 89.354 ms / 89.871 ms / 90.255 ms | 11.080 Melem/s / 11.127 Melem/s / 11.191 Melem/s |
| 14 | 14 | 100000 | 8.6089 ms / 8.7201 ms / 8.7921 ms | 11.374 Melem/s / 11.468 Melem/s / 11.616 Melem/s |
| 14 | 14 | 1000000 | 88.544 ms / 90.033 ms / 91.498 ms | 10.929 Melem/s / 11.107 Melem/s / 11.294 Melem/s |

---

## Bounded Results (`MpmcAsync`)

### Capacity: 4 (`Cap-4`)

`MpmcAsync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 1 | 100000 | 3.9767 ms / 4.0034 ms / 4.0340 ms | 24.789 Melem/s / 24.979 Melem/s / 25.146 Melem/s |
| 4 | 1 | 1 | 1000000 | 36.897 ms / 45.092 ms / 50.953 ms | 19.626 Melem/s / 22.177 Melem/s / 27.102 Melem/s |
| 4 | 1 | 4 | 100000 | 5.8240 ms / 6.1934 ms / 6.8627 ms | 14.571 Melem/s / 16.146 Melem/s / 17.170 Melem/s |
| 4 | 1 | 4 | 1000000 | 55.343 ms / 56.216 ms / 57.476 ms | 17.399 Melem/s / 17.789 Melem/s / 18.069 Melem/s |
| 4 | 1 | 14 | 100000 | 8.4006 ms / 8.4453 ms / 8.4942 ms | 11.773 Melem/s / 11.841 Melem/s / 11.904 Melem/s |
| 4 | 1 | 14 | 1000000 | 83.880 ms / 84.049 ms / 84.183 ms | 11.879 Melem/s / 11.898 Melem/s / 11.922 Melem/s |
| 4 | 4 | 1 | 100000 | 5.4417 ms / 5.4729 ms / 5.5107 ms | 18.147 Melem/s / 18.272 Melem/s / 18.377 Melem/s |
| 4 | 4 | 1 | 1000000 | 54.672 ms / 55.322 ms / 56.060 ms | 17.838 Melem/s / 18.076 Melem/s / 18.291 Melem/s |
| 4 | 4 | 4 | 100000 | 9.7658 ms / 9.8851 ms / 10.015 ms | 9.9848 Melem/s / 10.116 Melem/s / 10.240 Melem/s |
| 4 | 4 | 4 | 1000000 | 97.264 ms / 98.134 ms / 99.616 ms | 10.039 Melem/s / 10.190 Melem/s / 10.281 Melem/s |
| 4 | 4 | 14 | 100000 | 9.1009 ms / 9.1536 ms / 9.1947 ms | 10.876 Melem/s / 10.925 Melem/s / 10.988 Melem/s |
| 4 | 4 | 14 | 1000000 | 91.146 ms / 92.003 ms / 92.725 ms | 10.785 Melem/s / 10.869 Melem/s / 10.971 Melem/s |
| 4 | 14 | 1 | 100000 | 8.5241 ms / 8.5687 ms / 8.6157 ms | 11.607 Melem/s / 11.670 Melem/s / 11.731 Melem/s |
| 4 | 14 | 1 | 1000000 | 85.465 ms / 85.700 ms / 86.011 ms | 11.626 Melem/s / 11.669 Melem/s / 11.701 Melem/s |
| 4 | 14 | 4 | 100000 | 10.354 ms / 10.406 ms / 10.455 ms | 9.5651 Melem/s / 9.6095 Melem/s / 9.6578 Melem/s |
| 4 | 14 | 4 | 1000000 | 101.93 ms / 104.09 ms / 105.67 ms | 9.4638 Melem/s / 9.6074 Melem/s / 9.8106 Melem/s |
| 4 | 14 | 14 | 100000 | 8.6596 ms / 8.7407 ms / 8.8115 ms | 11.349 Melem/s / 11.441 Melem/s / 11.548 Melem/s |
| 4 | 14 | 14 | 1000000 | 86.168 ms / 86.582 ms / 86.968 ms | 11.499 Melem/s / 11.550 Melem/s / 11.605 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpmcAsync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 1 | 100000 | 1.3968 ms / 1.3982 ms / 1.4015 ms | 71.354 Melem/s / 71.520 Melem/s / 71.592 Melem/s |
| 128 | 1 | 1 | 1000000 | 13.898 ms / 13.950 ms / 14.030 ms | 71.276 Melem/s / 71.684 Melem/s / 71.950 Melem/s |
| 128 | 1 | 4 | 100000 | 2.5864 ms / 2.5971 ms / 2.6059 ms | 38.374 Melem/s / 38.505 Melem/s / 38.664 Melem/s |
| 128 | 1 | 4 | 1000000 | 25.944 ms / 26.343 ms / 26.803 ms | 37.309 Melem/s / 37.961 Melem/s / 38.545 Melem/s |
| 128 | 1 | 14 | 100000 | 3.6608 ms / 3.6669 ms / 3.6722 ms | 27.232 Melem/s / 27.271 Melem/s / 27.317 Melem/s |
| 128 | 1 | 14 | 1000000 | 36.363 ms / 36.454 ms / 36.622 ms | 27.306 Melem/s / 27.432 Melem/s / 27.501 Melem/s |
| 128 | 4 | 1 | 100000 | 2.6072 ms / 2.6274 ms / 2.6454 ms | 37.802 Melem/s / 38.060 Melem/s / 38.356 Melem/s |
| 128 | 4 | 1 | 1000000 | 26.493 ms / 26.627 ms / 26.755 ms | 37.376 Melem/s / 37.556 Melem/s / 37.745 Melem/s |
| 128 | 4 | 4 | 100000 | 4.5860 ms / 4.6875 ms / 4.7670 ms | 20.978 Melem/s / 21.333 Melem/s / 21.805 Melem/s |
| 128 | 4 | 4 | 1000000 | 44.980 ms / 46.224 ms / 48.005 ms | 20.831 Melem/s / 21.634 Melem/s / 22.232 Melem/s |
| 128 | 4 | 14 | 100000 | 3.9757 ms / 4.0476 ms / 4.1169 ms | 24.290 Melem/s / 24.706 Melem/s / 25.153 Melem/s |
| 128 | 4 | 14 | 1000000 | 40.844 ms / 41.134 ms / 41.339 ms | 24.190 Melem/s / 24.311 Melem/s / 24.483 Melem/s |
| 128 | 14 | 1 | 100000 | 3.7807 ms / 3.8385 ms / 3.9223 ms | 25.495 Melem/s / 26.052 Melem/s / 26.450 Melem/s |
| 128 | 14 | 1 | 1000000 | 37.697 ms / 37.935 ms / 38.137 ms | 26.221 Melem/s / 26.361 Melem/s / 26.527 Melem/s |
| 128 | 14 | 4 | 100000 | 4.6387 ms / 4.6499 ms / 4.6625 ms | 21.448 Melem/s / 21.506 Melem/s / 21.558 Melem/s |
| 128 | 14 | 4 | 1000000 | 45.363 ms / 45.575 ms / 45.781 ms | 21.843 Melem/s / 21.942 Melem/s / 22.044 Melem/s |
| 128 | 14 | 14 | 100000 | 4.7017 ms / 5.4624 ms / 5.9269 ms | 16.872 Melem/s / 18.307 Melem/s / 21.269 Melem/s |
| 128 | 14 | 14 | 1000000 | 50.151 ms / 52.509 ms / 56.889 ms | 17.578 Melem/s / 19.044 Melem/s / 19.940 Melem/s |
