# Fibre Benchmark: `MpscAsync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedAsync`)

### `MpscUnboundedAsync/Prod-1_Items-100000`
- **Time:** 772.77 µs – 773.85 µs – 776.03 µs
- **Throughput:** 128.86 Melem/s – 129.22 Melem/s – 129.41 Melem/s

### `MpscUnboundedAsync/Prod-1_Items-1000000`
- **Time:** 8.1088 ms – 8.2165 ms – 8.3176 ms  
- **Throughput:** 120.23 Melem/s – 121.71 Melem/s – 123.32 Melem/s

### `MpscUnboundedAsync/Prod-1_Items-10000000`
- **Time:** 81.548 ms – 81.799 ms – 82.232 ms  
- **Throughput:** 121.61 Melem/s – 122.25 Melem/s – 122.63 Melem/s

---

### `MpscUnboundedAsync/Prod-4_Items-100000`
- **Time:** 5.4942 ms – 5.8235 ms – 6.1898 ms  
- **Throughput:** 16.156 Melem/s – 17.172 Melem/s – 18.201 Melem/s

### `MpscUnboundedAsync/Prod-4_Items-1000000`
- **Time:** 36.689 ms – 37.412 ms – 39.224 ms  
- **Throughput:** 25.495 Melem/s – 26.729 Melem/s – 27.256 Melem/s

### `MpscUnboundedAsync/Prod-4_Items-10000000`
- **Time:** 368.06 ms – 373.84 ms – 380.26 ms  
- **Throughput:** 26.298 Melem/s – 26.749 Melem/s – 27.170 Melem/s

---

### `MpscUnboundedAsync/Prod-14_Items-100000`
- **Time:** 7.2790 ms – 7.3105 ms – 7.3447 ms  
- **Throughput:** 13.615 Melem/s – 13.679 Melem/s – 13.738 Melem/s

### `MpscUnboundedAsync/Prod-14_Items-1000000`
- **Time:** 76.616 ms – 76.796 ms – 77.035 ms  
- **Throughput:** 12.981 Melem/s – 13.021 Melem/s – 13.052 Melem/s

### `MpscUnboundedAsync/Prod-14_Items-10000000`
- **Time:** 773.45 ms – 775.82 ms – 778.17 ms  
- **Throughput:** 12.851 Melem/s – 12.890 Melem/s – 12.929 Melem/s

---

## Bounded Results (`MpscBoundedAsync`)

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-100000`
- **Time:** 9.0369 ms – 9.0916 ms – 9.1207 ms  
- **Throughput:** 10.964 Melem/s – 10.999 Melem/s – 11.066 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-1000000`
- **Time:** 90.821 ms – 91.185 ms – 91.432 ms  
- **Throughput:** 10.937 Melem/s – 10.967 Melem/s – 11.011 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-10000000`
- **Time:** 922.75 ms – 931.13 ms – 938.94 ms  
- **Throughput:** 10.650 Melem/s – 10.740 Melem/s – 10.837 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-100000`
- **Time:** 9.4837 ms – 9.5135 ms – 9.5470 ms  
- **Throughput:** 10.474 Melem/s – 10.511 Melem/s – 10.544 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-1000000`
- **Time:** 94.313 ms – 94.680 ms – 94.931 ms  
- **Throughput:** 10.534 Melem/s – 10.562 Melem/s – 10.603 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-10000000`
- **Time:** 938.52 ms – 946.04 ms – 954.06 ms  
- **Throughput:** 10.481 Melem/s – 10.570 Melem/s – 10.655 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-100000`
- **Time:** 10.399 ms – 10.478 ms – 10.561 ms  
- **Throughput:** 9.4685 Melem/s – 9.5439 Melem/s – 9.6159 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-1000000`
- **Time:** 102.01 ms – 102.67 ms – 103.29 ms  
- **Throughput:** 9.6819 Melem/s – 9.7396 Melem/s – 9.8026 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-10000000`
- **Time:** 1.0131 s – 1.0193 s – 1.0255 s  
- **Throughput:** 9.7514 Melem/s – 9.8109 Melem/s – 9.8712 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-100000`
- **Time:** 3.4106 ms – 3.4127 ms – 3.4158 ms  
- **Throughput:** 29.276 Melem/s – 29.303 Melem/s – 29.320 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-1000000`
- **Time:** 34.070 ms – 34.237 ms – 34.435 ms  
- **Throughput:** 29.040 Melem/s – 29.208 Melem/s – 29.351 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-10000000`
- **Time:** 343.52 ms – 347.16 ms – 350.94 ms  
- **Throughput:** 28.495 Melem/s – 28.805 Melem/s – 29.111 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-100000`
- **Time:** 3.5314 ms – 3.5398 ms – 3.5515 ms  
- **Throughput:** 28.157 Melem/s – 28.250 Melem/s – 28.317 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-1000000`
- **Time:** 35.282 ms – 35.461 ms – 35.571 ms  
- **Throughput:** 28.112 Melem/s – 28.200 Melem/s – 28.343 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-10000000`
- **Time:** 347.37 ms – 353.44 ms – 359.32 ms  
- **Throughput:** 27.830 Melem/s – 28.293 Melem/s – 28.788 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-100000`
- **Time:** 3.6328 ms – 3.6437 ms – 3.6666 ms  
- **Throughput:** 27.273 Melem/s – 27.444 Melem/s – 27.527 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-1000000`
- **Time:** 35.499 ms – 35.955 ms – 36.354 ms  
- **Throughput:** 27.507 Melem/s – 27.813 Melem/s – 28.170 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-10000000`
- **Time:** 357.87 ms – 363.61 ms – 369.22 ms  
- **Throughput:** 27.084 Melem/s – 27.502 Melem/s – 27.943 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.6022 ms – 1.6113 ms – 1.6222 ms  
- **Throughput:** 61.645 Melem/s – 62.062 Melem/s – 62.414 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-1000000`
- **Time:** 15.586 ms – 15.674 ms – 15.743 ms  
- **Throughput:** 63.522 Melem/s – 63.800 Melem/s – 64.161 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-10000000`
- **Time:** 159.81 ms – 160.50 ms – 161.39 ms  
- **Throughput:** 61.962 Melem/s – 62.305 Melem/s – 62.573 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-100000`
- **Time:** 1.6048 ms – 1.6097 ms – 1.6129 ms  
- **Throughput:** 61.999 Melem/s – 62.125 Melem/s – 62.315 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-1000000`
- **Time:** 15.865 ms – 15.894 ms – 15.945 ms  
- **Throughput:** 62.718 Melem/s – 62.916 Melem/s – 63.032 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-10000000`
- **Time:** 158.02 ms – 158.70 ms – 159.40 ms  
- **Throughput:** 62.735 Melem/s – 63.011 Melem/s – 63.283 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-100000`
- **Time:** 1.6239 ms – 1.6277 ms – 1.6347 ms  
- **Throughput:** 61.175 Melem/s – 61.437 Melem/s – 61.582 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-1000000`
- **Time:** 15.883 ms – 15.937 ms – 16.002 ms  
- **Throughput:** 62.492 Melem/s – 62.747 Melem/s – 62.960 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-10000000`
- **Time:** 157.37 ms – 158.12 ms – 158.90 ms  
- **Throughput:** 62.934 Melem/s – 63.244 Melem/s – 63.543 Melem/s