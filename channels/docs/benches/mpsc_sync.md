# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

### `MpscUnboundedSync/Prod-1_Items-100000`
- **Time:** 6.3924 ms – 6.4302 ms – 6.4682 ms  
- **Throughput:** 15.460 Melem/s – 15.552 Melem/s – 15.644 Melem/s

### `MpscUnboundedSync/Prod-1_Items-1000000`
- **Time:** 65.634 ms – 65.806 ms – 66.135 ms  
- **Throughput:** 15.121 Melem/s – 15.196 Melem/s – 15.236 Melem/s

---

### `MpscUnboundedSync/Prod-4_Items-100000`
- **Time:** 11.061 ms – 11.731 ms – 12.551 ms  
- **Throughput:** 7.9673 Melem/s – 8.5243 Melem/s – 9.0404 Melem/s

### `MpscUnboundedSync/Prod-4_Items-1000000`
- **Time:** 108.22 ms – 112.32 ms – 116.93 ms  
- **Throughput:** 8.5521 Melem/s – 8.9030 Melem/s – 9.2406 Melem/s

---

### `MpscUnboundedSync/Prod-14_Items-100000`
- **Time:** 21.699 ms – 21.955 ms – 22.089 ms  
- **Throughput:** 4.5272 Melem/s – 4.5547 Melem/s – 4.6084 Melem/s

### `MpscUnboundedSync/Prod-14_Items-1000000`
- **Time:** 216.89 ms – 219.35 ms – 221.49 ms  
- **Throughput:** 4.5148 Melem/s – 4.5590 Melem/s – 4.6107 Melem/s


## Bounded Results (`MpscBoundedSync`)

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedSync/Cap-1_Prod-1_Items-100000`
- **Time:** 26.450 ms – 27.436 ms – 28.789 ms  
- **Throughput:** 3.4736 Melem/s – 3.6449 Melem/s – 3.7807 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 220.85 ms – 244.07 ms – 272.83 ms  
- **Throughput:** 3.6652 Melem/s – 4.0971 Melem/s – 4.5279 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 2.1445 s – 2.2529 s – 2.4468 s  
- **Throughput:** 4.0870 Melem/s – 4.4388 Melem/s – 4.6631 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 33.204 ms – 35.319 ms – 37.720 ms  
- **Throughput:** 2.6511 Melem/s – 2.8313 Melem/s – 3.0117 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 351.30 ms – 383.50 ms – 417.04 ms  
- **Throughput:** 2.3979 Melem/s – 2.6076 Melem/s – 2.8465 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.5412 s – 3.7308 s – 3.9386 s  
- **Throughput:** 2.5389 Melem/s – 2.6804 Melem/s – 2.8239 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 81.378 ms – 82.939 ms – 84.401 ms  
- **Throughput:** 1.1848 Melem/s – 1.2057 Melem/s – 1.2288 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 813.00 ms – 842.28 ms – 867.59 ms  
- **Throughput:** 1.1526 Melem/s – 1.1873 Melem/s – 1.2300 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 8.3832 s – 8.5391 s – 8.6710 s  
- **Throughput:** 1.1533 Melem/s – 1.1711 Melem/s – 1.1929 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 8.9286 ms – 9.1184 ms – 9.2584 ms  
- **Throughput:** 10.801 Melem/s – 10.967 Melem/s – 11.200 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 88.062 ms – 89.160 ms – 90.772 ms  
- **Throughput:** 11.017 Melem/s – 11.216 Melem/s – 11.356 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 879.69 ms – 893.65 ms – 908.53 ms  
- **Throughput:** 11.007 Melem/s – 11.190 Melem/s – 11.368 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 14.044 ms – 15.965 ms – 18.284 ms  
- **Throughput:** 5.4693 Melem/s – 6.2637 Melem/s – 7.1204 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 137.37 ms – 144.38 ms – 152.72 ms  
- **Throughput:** 6.5478 Melem/s – 6.9263 Melem/s – 7.2795 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.4027 s – 1.4454 s – 1.4873 s  
- **Throughput:** 6.7236 Melem/s – 6.9183 Melem/s – 7.1289 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 34.889 ms – 38.380 ms – 40.608 ms  
- **Throughput:** 2.4626 Melem/s – 2.6055 Melem/s – 2.8663 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 305.77 ms – 314.44 ms – 321.53 ms  
- **Throughput:** 3.1101 Melem/s – 3.1802 Melem/s – 3.2705 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 3.0588 s – 3.1349 s – 3.2261 s  
- **Throughput:** 3.0997 Melem/s – 3.1899 Melem/s – 3.2692 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 8.6048 ms – 8.6729 ms – 8.7342 ms  
- **Throughput:** 11.449 Melem/s – 11.530 Melem/s – 11.621 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 86.674 ms – 87.706 ms – 88.759 ms  
- **Throughput:** 11.266 Melem/s – 11.402 Melem/s – 11.538 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 855.67 ms – 863.38 ms – 873.65 ms  
- **Throughput:** 11.446 Melem/s – 11.582 Melem/s – 11.687 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 6.5796 ms – 6.7344 ms – 6.9102 ms  
- **Throughput:** 14.471 Melem/s – 14.849 Melem/s – 15.199 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 65.037 ms – 65.809 ms – 66.740 ms  
- **Throughput:** 14.984 Melem/s – 15.196 Melem/s – 15.376 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 641.29 ms – 652.04 ms – 664.73 ms  
- **Throughput:** 15.044 Melem/s – 15.337 Melem/s – 15.594 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 6.6014 ms – 6.7106 ms – 6.8838 ms  
- **Throughput:** 14.527 Melem/s – 14.902 Melem/s – 15.148 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 66.422 ms – 67.968 ms – 69.459 ms  
- **Throughput:** 14.397 Melem/s – 14.713 Melem/s – 15.055 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 654.13 ms – 674.78 ms – 698.97 ms  
- **Throughput:** 14.307 Melem/s – 14.820 Melem/s – 15.287 Melem/s