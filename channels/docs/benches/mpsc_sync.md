# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

### `MpscUnboundedSync/Prod-1_Items-100000`
- **Time:** 480.52 µs – 496.84 µs – 509.60 µs  
- **Throughput:** 196.23 Melem/s – 201.27 Melem/s – 208.11 Melem/s

### `MpscUnboundedSync/Prod-1_Items-1000000`
- **Time:** 4.6009 ms – 4.7551 ms – 5.0657 ms  
- **Throughput:** 197.41 Melem/s – 210.30 Melem/s – 217.35 Melem/s

### `MpscUnboundedSync/Prod-1_Items-10000000`
- **Time:** 46.309 ms – 47.393 ms – 48.601 ms  
- **Throughput:** 205.76 Melem/s – 211.00 Melem/s – 215.94 Melem/s

---

### `MpscUnboundedSync/Prod-4_Items-100000`
- **Time:** 4.3888 ms – 4.6043 ms – 4.7374 ms  
- **Throughput:** 21.109 Melem/s – 21.719 Melem/s – 22.785 Melem/s

### `MpscUnboundedSync/Prod-4_Items-1000000`
- **Time:** 46.203 ms – 47.668 ms – 50.004 ms  
- **Throughput:** 19.999 Melem/s – 20.978 Melem/s – 21.644 Melem/s

### `MpscUnboundedSync/Prod-4_Items-10000000`
- **Time:** 485.68 ms – 501.40 ms – 520.14 ms  
- **Throughput:** 19.226 Melem/s – 19.944 Melem/s – 20.590 Melem/s

---

### `MpscUnboundedSync/Prod-14_Items-100000`
- **Time:** 7.1674 ms – 7.1761 ms – 7.1907 ms  
- **Throughput:** 13.907 Melem/s – 13.935 Melem/s – 13.952 Melem/s

### `MpscUnboundedSync/Prod-14_Items-1000000`
- **Time:** 75.669 ms – 75.862 ms – 76.079 ms  
- **Throughput:** 13.144 Melem/s – 13.182 Melem/s – 13.215 Melem/s

### `MpscUnboundedSync/Prod-14_Items-10000000`
- **Time:** 755.37 ms – 764.96 ms – 771.95 ms  
- **Throughput:** 12.954 Melem/s – 13.073 Melem/s – 13.239 Melem/s


## Bounded Results (`MpscBoundedSync`)

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedSync/Cap-1_Prod-1_Items-100000`
- **Time:** 16.716 ms – 16.971 ms – 17.459 ms  
- **Throughput:** 5.7276 Melem/s – 5.8925 Melem/s – 5.9823 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 168.15 ms – 169.69 ms – 172.18 ms  
- **Throughput:** 5.8078 Melem/s – 5.8930 Melem/s – 5.9471 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 1.7178 s – 1.7695 s – 1.8231 s  
- **Throughput:** 5.4851 Melem/s – 5.6513 Melem/s – 5.8213 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 31.652 ms – 32.082 ms – 32.676 ms  
- **Throughput:** 3.0604 Melem/s – 3.1170 Melem/s – 3.1593 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 309.19 ms – 321.29 ms – 335.14 ms  
- **Throughput:** 2.9838 Melem/s – 3.1124 Melem/s – 3.2343 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.3062 s – 3.4151 s – 3.5133 s  
- **Throughput:** 2.8463 Melem/s – 2.9281 Melem/s – 3.0246 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 90.300 ms – 90.622 ms – 90.889 ms  
- **Throughput:** 1.1002 Melem/s – 1.1035 Melem/s – 1.1074 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 932.25 ms – 964.36 ms – 996.77 ms  
- **Throughput:** 1.0032 Melem/s – 1.0370 Melem/s – 1.0727 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 8.5342 s – 9.0370 s – 9.4738 s  
- **Throughput:** 1.0555 Melem/s – 1.1066 Melem/s – 1.1718 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 14.974 ms – 15.115 ms – 15.255 ms  
- **Throughput:** 6.5554 Melem/s – 6.6161 Melem/s – 6.6781 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 141.81 ms – 144.06 ms – 147.01 ms  
- **Throughput:** 6.8023 Melem/s – 6.9418 Melem/s – 7.0519 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 1.4206 s – 1.4326 s – 1.4450 s  
- **Throughput:** 6.9205 Melem/s – 6.9804 Melem/s – 7.0394 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 19.474 ms – 20.170 ms – 21.061 ms  
- **Throughput:** 4.7481 Melem/s – 4.9579 Melem/s – 5.1350 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 195.72 ms – 203.82 ms – 212.01 ms  
- **Throughput:** 4.7167 Melem/s – 4.9063 Melem/s – 5.1093 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.9403 s – 1.9707 s – 2.0231 s  
- **Throughput:** 4.9428 Melem/s – 5.0743 Melem/s – 5.1538 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 48.603 ms – 48.863 ms – 49.071 ms  
- **Throughput:** 2.0379 Melem/s – 2.0465 Melem/s – 2.0575 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 486.88 ms – 494.15 ms – 503.75 ms  
- **Throughput:** 1.9851 Melem/s – 2.0237 Melem/s – 2.0539 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 4.8252 s – 4.8349 s – 4.8451 s  
- **Throughput:** 2.0639 Melem/s – 2.0683 Melem/s – 2.0724 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 12.342 ms – 12.427 ms – 12.518 ms  
- **Throughput:** 7.9883 Melem/s – 8.0473 Melem/s – 8.1025 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 124.71 ms – 125.91 ms – 126.51 ms  
- **Throughput:** 7.9047 Melem/s – 7.9421 Melem/s – 8.0183 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 1.2869 s – 1.2926 s – 1.2977 s  
- **Throughput:** 7.7058 Melem/s – 7.7363 Melem/s – 7.7704 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 8.0559 ms – 8.4955 ms – 8.8739 ms  
- **Throughput:** 11.269 Melem/s – 11.771 Melem/s – 12.413 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 93.058 ms – 103.29 ms – 111.84 ms  
- **Throughput:** 8.9415 Melem/s – 9.6817 Melem/s – 10.746 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 1.0450 s – 1.3097 s – 1.5679 s  
- **Throughput:** 6.3780 Melem/s – 7.6351 Melem/s – 9.5697 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 16.344 ms – 16.525 ms – 16.685 ms  
- **Throughput:** 5.9934 Melem/s – 6.0516 Melem/s – 6.1186 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 160.53 ms – 161.65 ms – 162.60 ms  
- **Throughput:** 6.1502 Melem/s – 6.1861 Melem/s – 6.2296 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 1.5934 s – 1.6220 s – 1.6497 s  
- **Throughput:** 6.0618 Melem/s – 6.1653 Melem/s – 6.2759 Melem/s