# Fibre Benchmark: `MpmcV2 Async`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpmcUnboundedAsync`)

`MpmcUnboundedAsync/Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 100000 | 768.84 µs / 792.35 µs / 833.05 µs | 120.04 Melem/s / 126.21 Melem/s / 130.07 Melem/s |
| 1 | 1 | 1000000 | 12.195 ms / 12.273 ms / 12.335 ms | 81.070 Melem/s / 81.479 Melem/s / 82.001 Melem/s |
| 1 | 4 | 100000 | 3.5135 ms / 3.6260 ms / 3.7628 ms | 26.576 Melem/s / 27.579 Melem/s / 28.462 Melem/s |
| 1 | 4 | 1000000 | 28.346 ms / 28.930 ms / 29.548 ms | 33.844 Melem/s / 34.567 Melem/s / 35.279 Melem/s |
| 1 | 14 | 100000 | 4.3782 ms / 4.9841 ms / 5.8728 ms | 17.028 Melem/s / 20.064 Melem/s / 22.840 Melem/s |
| 1 | 14 | 1000000 | 54.641 ms / 57.765 ms / 60.319 ms | 16.579 Melem/s / 17.312 Melem/s / 18.301 Melem/s |
| 4 | 1 | 100000 | 3.9473 ms / 4.1000 ms / 4.1992 ms | 23.814 Melem/s / 24.390 Melem/s / 25.334 Melem/s |
| 4 | 1 | 1000000 | 42.147 ms / 44.970 ms / 49.173 ms | 20.336 Melem/s / 22.237 Melem/s / 23.726 Melem/s |
| 4 | 4 | 100000 | 12.246 ms / 12.297 ms / 12.352 ms | 8.0961 Melem/s / 8.1319 Melem/s / 8.1659 Melem/s |
| 4 | 4 | 1000000 | 112.79 ms / 117.04 ms / 121.17 ms | 8.2529 Melem/s / 8.5438 Melem/s / 8.8658 Melem/s |
| 4 | 14 | 100000 | 11.893 ms / 12.277 ms / 12.733 ms | 7.8537 Melem/s / 8.1450 Melem/s / 8.4080 Melem/s |
| 4 | 14 | 1000000 | 137.42 ms / 143.37 ms / 148.72 ms | 6.7241 Melem/s / 6.9749 Melem/s / 7.2769 Melem/s |
| 14 | 1 | 100000 | 8.1520 ms / 8.1629 ms / 8.1767 ms | 12.230 Melem/s / 12.251 Melem/s / 12.267 Melem/s |
| 14 | 1 | 1000000 | 82.339 ms / 82.664 ms / 83.051 ms | 12.041 Melem/s / 12.097 Melem/s / 12.145 Melem/s |
| 14 | 4 | 100000 | 11.039 ms / 11.262 ms / 11.362 ms | 8.8012 Melem/s / 8.8795 Melem/s / 9.0591 Melem/s |
| 14 | 4 | 1000000 | 109.17 ms / 111.76 ms / 113.24 ms | 8.8312 Melem/s / 8.9476 Melem/s / 9.1601 Melem/s |
| 14 | 14 | 100000 | 12.927 ms / 13.486 ms / 13.932 ms | 7.1775 Melem/s / 7.4150 Melem/s / 7.7359 Melem/s |
| 14 | 14 | 1000000 | 130.40 ms / 136.20 ms / 140.30 ms | 7.1275 Melem/s / 7.3420 Melem/s / 7.6687 Melem/s |

---

## Bounded Results (`MpmcAsync`)

### Capacity: 4 (`Cap-4`)

`MpmcAsync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 4 | 1 | 1 | 100000 | 3.1409 ms / 3.1770 ms / 3.2333 ms | 30.928 Melem/s / 31.476 Melem/s / 31.838 Melem/s |
| 4 | 1 | 1 | 1000000 | 31.239 ms / 31.508 ms / 31.722 ms | 31.524 Melem/s / 31.738 Melem/s / 32.011 Melem/s |
| 4 | 1 | 4 | 100000 | 5.1815 ms / 5.2661 ms / 5.3763 ms | 18.600 Melem/s / 18.989 Melem/s / 19.299 Melem/s |
| 4 | 1 | 4 | 1000000 | 51.696 ms / 52.694 ms / 53.616 ms | 18.651 Melem/s / 18.977 Melem/s / 19.344 Melem/s |
| 4 | 1 | 14 | 100000 | 7.8861 ms / 7.9435 ms / 7.9991 ms | 12.501 Melem/s / 12.589 Melem/s / 12.681 Melem/s |
| 4 | 1 | 14 | 1000000 | 78.596 ms / 78.730 ms / 78.890 ms | 12.676 Melem/s / 12.702 Melem/s / 12.723 Melem/s |
| 4 | 4 | 1 | 100000 | 5.1054 ms / 5.2036 ms / 5.3193 ms | 18.799 Melem/s / 19.217 Melem/s / 19.587 Melem/s |
| 4 | 4 | 1 | 1000000 | 50.906 ms / 51.580 ms / 52.418 ms | 19.077 Melem/s / 19.387 Melem/s / 19.644 Melem/s |
| 4 | 4 | 4 | 100000 | 8.8971 ms / 8.9683 ms / 9.0573 ms | 11.041 Melem/s / 11.150 Melem/s / 11.240 Melem/s |
| 4 | 4 | 4 | 1000000 | 87.183 ms / 88.535 ms / 89.173 ms | 11.214 Melem/s / 11.295 Melem/s / 11.470 Melem/s |
| 4 | 4 | 14 | 100000 | 8.5824 ms / 8.6125 ms / 8.6369 ms | 11.578 Melem/s / 11.611 Melem/s / 11.652 Melem/s |
| 4 | 4 | 14 | 1000000 | 84.130 ms / 84.752 ms / 85.309 ms | 11.722 Melem/s / 11.799 Melem/s / 11.886 Melem/s |
| 4 | 14 | 1 | 100000 | 7.9909 ms / 8.1189 ms / 8.3338 ms | 11.999 Melem/s / 12.317 Melem/s / 12.514 Melem/s |
| 4 | 14 | 1 | 1000000 | 79.619 ms / 80.104 ms / 80.668 ms | 12.397 Melem/s / 12.484 Melem/s / 12.560 Melem/s |
| 4 | 14 | 4 | 100000 | 9.4073 ms / 9.4780 ms / 9.5803 ms | 10.438 Melem/s / 10.551 Melem/s / 10.630 Melem/s |
| 4 | 14 | 4 | 1000000 | 91.808 ms / 92.303 ms / 92.631 ms | 10.795 Melem/s / 10.834 Melem/s / 10.892 Melem/s |
| 4 | 14 | 14 | 100000 | 8.1844 ms / 8.2984 ms / 8.3848 ms | 11.926 Melem/s / 12.051 Melem/s / 12.218 Melem/s |
| 4 | 14 | 14 | 1000000 | 82.366 ms / 83.094 ms / 83.633 ms | 11.957 Melem/s / 12.035 Melem/s / 12.141 Melem/s |

### Capacity: 128 (`Cap-128`)

`MpmcAsync/Cap-{Cap}_Prod-{Prod}_Cons-{Cons}_Items-{Items}`

| Cap | Prod | Cons | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---|---:|---:|
| 128 | 1 | 1 | 100000 | 1.2387 ms / 1.2556 ms / 1.2907 ms | 77.477 Melem/s / 79.644 Melem/s / 80.733 Melem/s |
| 128 | 1 | 1 | 1000000 | 12.300 ms / 12.548 ms / 12.853 ms | 77.803 Melem/s / 79.691 Melem/s / 81.298 Melem/s |
| 128 | 1 | 4 | 100000 | 2.4328 ms / 2.4848 ms / 2.5530 ms | 39.170 Melem/s / 40.245 Melem/s / 41.105 Melem/s |
| 128 | 1 | 4 | 1000000 | 23.999 ms / 24.150 ms / 24.337 ms | 41.090 Melem/s / 41.408 Melem/s / 41.669 Melem/s |
| 128 | 1 | 14 | 100000 | 3.3903 ms / 3.3960 ms / 3.4039 ms | 29.378 Melem/s / 29.447 Melem/s / 29.496 Melem/s |
| 128 | 1 | 14 | 1000000 | 33.823 ms / 33.896 ms / 33.983 ms | 29.426 Melem/s / 29.502 Melem/s / 29.566 Melem/s |
| 128 | 4 | 1 | 100000 | 2.3759 ms / 2.3915 ms / 2.4051 ms | 41.578 Melem/s / 41.815 Melem/s / 42.089 Melem/s |
| 128 | 4 | 1 | 1000000 | 24.011 ms / 24.122 ms / 24.283 ms | 41.181 Melem/s / 41.456 Melem/s / 41.647 Melem/s |
| 128 | 4 | 4 | 100000 | 4.2631 ms / 4.3006 ms / 4.3357 ms | 23.064 Melem/s / 23.252 Melem/s / 23.457 Melem/s |
| 128 | 4 | 4 | 1000000 | 43.482 ms / 43.659 ms / 43.800 ms | 22.831 Melem/s / 22.905 Melem/s / 22.998 Melem/s |
| 128 | 4 | 14 | 100000 | 3.7270 ms / 3.7375 ms / 3.7463 ms | 26.693 Melem/s / 26.756 Melem/s / 26.831 Melem/s |
| 128 | 4 | 14 | 1000000 | 35.931 ms / 37.079 ms / 38.225 ms | 26.161 Melem/s / 26.969 Melem/s / 27.831 Melem/s |
| 128 | 14 | 1 | 100000 | 3.3611 ms / 3.3685 ms / 3.3749 ms | 29.630 Melem/s / 29.687 Melem/s / 29.752 Melem/s |
| 128 | 14 | 1 | 1000000 | 33.799 ms / 34.373 ms / 35.219 ms | 28.394 Melem/s / 29.092 Melem/s / 29.587 Melem/s |
| 128 | 14 | 4 | 100000 | 3.8628 ms / 3.8878 ms / 3.9246 ms | 25.480 Melem/s / 25.721 Melem/s / 25.888 Melem/s |
| 128 | 14 | 4 | 1000000 | 37.077 ms / 37.538 ms / 38.078 ms | 26.262 Melem/s / 26.640 Melem/s / 26.971 Melem/s |
| 128 | 14 | 14 | 100000 | 5.4228 ms / 5.7666 ms / 5.9507 ms | 16.805 Melem/s / 17.341 Melem/s / 18.441 Melem/s |
| 128 | 14 | 14 | 1000000 | 47.570 ms / 55.393 ms / 60.848 ms | 16.434 Melem/s / 18.053 Melem/s / 21.022 Melem/s |
