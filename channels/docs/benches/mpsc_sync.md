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
- **Time:** 26.495 ms – 27.476 ms – 29.366 ms  
- **Throughput:** 3.4053 Melem/s – 3.6395 Melem/s – 3.7743 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 225.28 ms – 241.55 ms – 257.59 ms  
- **Throughput:** 3.8821 Melem/s – 4.1400 Melem/s – 4.4390 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 2.2906 s – 2.5528 s – 2.8392 s  
- **Throughput:** 3.5221 Melem/s – 3.9173 Melem/s – 4.3657 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 38.848 ms – 41.053 ms – 43.825 ms  
- **Throughput:** 2.2818 Melem/s – 2.4359 Melem/s – 2.5741 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 346.28 ms – 362.36 ms – 380.48 ms  
- **Throughput:** 2.6283 Melem/s – 2.7597 Melem/s – 2.8878 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.8832 s – 4.0873 s – 4.3145 s  
- **Throughput:** 2.3178 Melem/s – 2.4466 Melem/s – 2.5752 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 81.940 ms – 82.898 ms – 84.380 ms  
- **Throughput:** 1.1851 Melem/s – 1.2063 Melem/s – 1.2204 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 846.72 ms – 859.79 ms – 871.62 ms  
- **Throughput:** 1.1473 Melem/s – 1.1631 Melem/s – 1.1810 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 8.6497 s – 8.7511 s – 8.9035 s  
- **Throughput:** 1.1231 Melem/s – 1.1427 Melem/s – 1.1561 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 8.9092 ms – 9.0154 ms – 9.1359 ms  
- **Throughput:** 10.946 Melem/s – 11.092 Melem/s – 11.224 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 89.329 ms – 92.777 ms – 101.19 ms  
- **Throughput:** 9.8822 Melem/s – 10.778 Melem/s – 11.195 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 891.88 ms – 907.84 ms – 927.76 ms  
- **Throughput:** 10.779 Melem/s – 11.015 Melem/s – 11.212 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 15.234 ms – 16.035 ms – 17.478 ms  
- **Throughput:** 5.7215 Melem/s – 6.2362 Melem/s – 6.5643 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 151.03 ms – 158.10 ms – 166.96 ms  
- **Throughput:** 5.9894 Melem/s – 6.3250 Melem/s – 6.6213 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.5613 s – 1.6016 s – 1.6411 s  
- **Throughput:** 6.0936 Melem/s – 6.2439 Melem/s – 6.4051 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 28.443 ms – 28.723 ms – 28.991 ms  
- **Throughput:** 3.4493 Melem/s – 3.4815 Melem/s – 3.5158 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 278.73 ms – 281.98 ms – 285.42 ms  
- **Throughput:** 3.5036 Melem/s – 3.5463 Melem/s – 3.5878 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 2.8923 s – 3.0454 s – 3.3040 s  
- **Throughput:** 3.0266 Melem/s – 3.2837 Melem/s – 3.4575 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 8.5625 ms – 8.6385 ms – 8.7743 ms  
- **Throughput:** 11.397 Melem/s – 11.576 Melem/s – 11.679 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 93.990 ms – 97.399 ms – 104.31 ms  
- **Throughput:** 9.5867 Melem/s – 10.267 Melem/s – 10.639 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 877.62 ms – 895.69 ms – 922.94 ms  
- **Throughput:** 10.835 Melem/s – 11.165 Melem/s – 11.394 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 6.3775 ms – 6.5913 ms – 6.8758 ms  
- **Throughput:** 14.544 Melem/s – 15.171 Melem/s – 15.680 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 62.493 ms – 63.935 ms – 65.721 ms  
- **Throughput:** 15.216 Melem/s – 15.641 Melem/s – 16.002 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 617.18 ms – 626.63 ms – 636.32 ms  
- **Throughput:** 15.715 Melem/s – 15.959 Melem/s – 16.203 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 6.2342 ms – 6.3478 ms – 6.4332 ms  
- **Throughput:** 15.544 Melem/s – 15.754 Melem/s – 16.041 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 61.112 ms – 61.704 ms – 62.466 ms  
- **Throughput:** 16.009 Melem/s – 16.206 Melem/s – 16.363 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 620.87 ms – 633.72 ms – 649.31 ms  
- **Throughput:** 15.401 Melem/s – 15.780 Melem/s – 16.106 Melem/s