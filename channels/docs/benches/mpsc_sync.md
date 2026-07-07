# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

### `MpscUnboundedSync/Prod-1_Items-100000`
- **Time:** 508.31 µs – 544.11 µs – 587.66 µs  
- **Throughput:** 170.17 Melem/s – 183.79 Melem/s – 196.73 Melem/s

### `MpscUnboundedSync/Prod-1_Items-1000000`
- **Time:** 4.2894 ms – 4.3468 ms – 4.4086 ms  
- **Throughput:** 226.83 Melem/s – 230.05 Melem/s – 233.13 Melem/s

### `MpscUnboundedSync/Prod-1_Items-10000000`
- **Time:** 42.778 ms – 43.375 ms – 44.269 ms  
- **Throughput:** 225.89 Melem/s – 230.55 Melem/s – 233.76 Melem/s

---

### `MpscUnboundedSync/Prod-4_Items-100000`
- **Time:** 5.0459 ms – 5.4986 ms – 5.9513 ms  
- **Throughput:** 16.803 Melem/s – 18.187 Melem/s – 19.818 Melem/s

### `MpscUnboundedSync/Prod-4_Items-1000000`
- **Time:** 49.570 ms – 52.043 ms – 56.072 ms  
- **Throughput:** 17.834 Melem/s – 19.215 Melem/s – 20.173 Melem/s

### `MpscUnboundedSync/Prod-4_Items-10000000`
- **Time:** 478.82 ms – 501.97 ms – 528.37 ms  
- **Throughput:** 18.926 Melem/s – 19.921 Melem/s – 20.884 Melem/s

---

### `MpscUnboundedSync/Prod-14_Items-100000`
- **Time:** 7.1477 ms – 7.1614 ms – 7.1730 ms  
- **Throughput:** 13.941 Melem/s – 13.964 Melem/s – 13.991 Melem/s

### `MpscUnboundedSync/Prod-14_Items-1000000`
- **Time:** 76.632 ms – 76.831 ms – 76.953 ms  
- **Throughput:** 12.995 Melem/s – 13.016 Melem/s – 13.049 Melem/s

### `MpscUnboundedSync/Prod-14_Items-10000000`
- **Time:** 771.19 ms – 773.42 ms – 775.56 ms  
- **Throughput:** 12.894 Melem/s – 12.930 Melem/s – 12.967 Melem/s


## Bounded Results (`MpscBoundedSync`)

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedSync/Cap-1_Prod-1_Items-100000`
- **Time:** 13.895 ms – 14.111 ms – 14.399 ms  
- **Throughput:** 6.9450 Melem/s – 7.0867 Melem/s – 7.1969 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 138.19 ms – 141.42 ms – 144.91 ms  
- **Throughput:** 6.9010 Melem/s – 7.0711 Melem/s – 7.2363 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 1.3430 s – 1.3812 s – 1.4360 s  
- **Throughput:** 6.9637 Melem/s – 7.2399 Melem/s – 7.4458 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 29.981 ms – 30.897 ms – 31.946 ms  
- **Throughput:** 3.1303 Melem/s – 3.2366 Melem/s – 3.3354 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 315.86 ms – 340.60 ms – 373.34 ms  
- **Throughput:** 2.6785 Melem/s – 2.9360 Melem/s – 3.1659 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.1529 s – 3.2329 s – 3.3193 s  
- **Throughput:** 3.0127 Melem/s – 3.0932 Melem/s – 3.1717 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 80.583 ms – 80.846 ms – 81.132 ms  
- **Throughput:** 1.2326 Melem/s – 1.2369 Melem/s – 1.2410 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 784.73 ms – 793.98 ms – 803.26 ms  
- **Throughput:** 1.2449 Melem/s – 1.2595 Melem/s – 1.2743 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 7.8849 s – 7.9135 s – 7.9405 s  
- **Throughput:** 1.2594 Melem/s – 1.2637 Melem/s – 1.2682 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 6.3119 ms – 6.3436 ms – 6.3997 ms  
- **Throughput:** 15.626 Melem/s – 15.764 Melem/s – 15.843 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 61.685 ms – 62.017 ms – 62.482 ms  
- **Throughput:** 16.005 Melem/s – 16.125 Melem/s – 16.211 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 604.23 ms – 607.18 ms – 611.09 ms  
- **Throughput:** 16.364 Melem/s – 16.470 Melem/s – 16.550 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 10.451 ms – 10.572 ms – 10.792 ms  
- **Throughput:** 9.2665 Melem/s – 9.4593 Melem/s – 9.5685 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 105.38 ms – 111.65 ms – 119.94 ms  
- **Throughput:** 8.3375 Melem/s – 8.9568 Melem/s – 9.4892 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.0601 s – 1.1086 s – 1.1693 s  
- **Throughput:** 8.5525 Melem/s – 9.0204 Melem/s – 9.4329 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 23.798 ms – 23.950 ms – 24.095 ms  
- **Throughput:** 4.1503 Melem/s – 4.1754 Melem/s – 4.2020 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 228.91 ms – 236.44 ms – 240.62 ms  
- **Throughput:** 4.1559 Melem/s – 4.2293 Melem/s – 4.3685 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 2.4008 s – 2.4077 s – 2.4145 s  
- **Throughput:** 4.1417 Melem/s – 4.1533 Melem/s – 4.1652 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.2410 ms – 1.2466 ms – 1.2548 ms  
- **Throughput:** 79.695 Melem/s – 80.218 Melem/s – 80.579 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 12.090 ms – 12.168 ms – 12.305 ms  
- **Throughput:** 81.271 Melem/s – 82.180 Melem/s – 82.711 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 121.33 ms – 122.67 ms – 124.37 ms  
- **Throughput:** 80.406 Melem/s – 81.521 Melem/s – 82.420 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 4.5885 ms – 4.6312 ms – 4.6743 ms  
- **Throughput:** 21.393 Melem/s – 21.593 Melem/s – 21.793 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 46.719 ms – 47.702 ms – 48.727 ms  
- **Throughput:** 20.522 Melem/s – 20.964 Melem/s – 21.405 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 459.21 ms – 472.69 ms – 494.37 ms  
- **Throughput:** 20.228 Melem/s – 21.156 Melem/s – 21.777 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 26.070 ms – 26.336 ms – 26.633 ms  
- **Throughput:** 3.7548 Melem/s – 3.7971 Melem/s – 3.8358 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 263.48 ms – 270.27 ms – 277.83 ms  
- **Throughput:** 3.5993 Melem/s – 3.7001 Melem/s – 3.7953 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 2.6993 s – 2.7165 s – 2.7449 s  
- **Throughput:** 3.6432 Melem/s – 3.6812 Melem/s – 3.7046 Melem/s
