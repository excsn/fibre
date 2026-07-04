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

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedSync/Cap-1_Prod-1_Items-100000`
- **Time:** 17.126 ms – 17.438 ms – 17.816 ms  
- **Throughput:** 5.6129 Melem/s – 5.7347 Melem/s – 5.8392 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 171.43 ms – 175.53 ms – 179.95 ms  
- **Throughput:** 5.5571 Melem/s – 5.6969 Melem/s – 5.8332 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 1.8304 s – 1.8796 s – 1.9287 s  
- **Throughput:** 5.1847 Melem/s – 5.3202 Melem/s – 5.4631 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 35.775 ms – 38.216 ms – 39.860 ms  
- **Throughput:** 2.5088 Melem/s – 2.6167 Melem/s – 2.7952 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 355.09 ms – 372.20 ms – 389.39 ms  
- **Throughput:** 2.5681 Melem/s – 2.6867 Melem/s – 2.8162 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.1181 s – 3.3359 s – 3.5812 s  
- **Throughput:** 2.7924 Melem/s – 2.9977 Melem/s – 3.2071 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 83.312 ms – 86.310 ms – 87.754 ms  
- **Throughput:** 1.1396 Melem/s – 1.1586 Melem/s – 1.2003 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 875.84 ms – 888.08 ms – 898.98 ms  
- **Throughput:** 1.1124 Melem/s – 1.1260 Melem/s – 1.1418 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 8.9013 s – 8.9654 s – 9.0168 s  
- **Throughput:** 1.1090 Melem/s – 1.1154 Melem/s – 1.1234 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 14.369 ms – 14.852 ms – 15.094 ms  
- **Throughput:** 6.6253 Melem/s – 6.7333 Melem/s – 6.9596 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 138.53 ms – 139.11 ms – 140.15 ms  
- **Throughput:** 7.1350 Melem/s – 7.1883 Melem/s – 7.2189 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 1.3475 s – 1.3612 s – 1.3785 s  
- **Throughput:** 7.2541 Melem/s – 7.3462 Melem/s – 7.4212 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 18.436 ms – 18.630 ms – 19.037 ms  
- **Throughput:** 5.2530 Melem/s – 5.3678 Melem/s – 5.4242 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 187.29 ms – 193.58 ms – 201.76 ms  
- **Throughput:** 4.9563 Melem/s – 5.1658 Melem/s – 5.3392 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.8387 s – 1.8894 s – 1.9513 s  
- **Throughput:** 5.1249 Melem/s – 5.2927 Melem/s – 5.4386 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 44.599 ms – 44.711 ms – 44.781 ms  
- **Throughput:** 2.2331 Melem/s – 2.2366 Melem/s – 2.2422 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 440.85 ms – 442.41 ms – 444.16 ms  
- **Throughput:** 2.2514 Melem/s – 2.2604 Melem/s – 2.2683 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 4.3997 s – 4.4067 s – 4.4135 s  
- **Throughput:** 2.2658 Melem/s – 2.2693 Melem/s – 2.2729 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 11.096 ms – 11.216 ms – 11.311 ms  
- **Throughput:** 8.8410 Melem/s – 8.9155 Melem/s – 9.0121 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 112.84 ms – 113.36 ms – 113.96 ms  
- **Throughput:** 8.7748 Melem/s – 8.8216 Melem/s – 8.8618 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 1.0857 s – 1.0954 s – 1.1070 s  
- **Throughput:** 9.0338 Melem/s – 9.1287 Melem/s – 9.2108 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 9.8435 ms – 10.419 ms – 11.321 ms  
- **Throughput:** 8.8329 Melem/s – 9.5980 Melem/s – 10.159 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 120.42 ms – 131.18 ms – 140.56 ms  
- **Throughput:** 7.1143 Melem/s – 7.6229 Melem/s – 8.3043 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 962.64 ms – 1.0070 s – 1.0503 s  
- **Throughput:** 9.5210 Melem/s – 9.9305 Melem/s – 10.388 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 34.250 ms – 34.870 ms – 35.167 ms  
- **Throughput:** 2.8436 Melem/s – 2.8678 Melem/s – 2.9197 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 348.75 ms – 353.52 ms – 357.81 ms  
- **Throughput:** 2.7948 Melem/s – 2.8287 Melem/s – 2.8674 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 3.5713 s – 3.5928 s – 3.6130 s  
- **Throughput:** 2.7678 Melem/s – 2.7834 Melem/s – 2.8001 Melem/s