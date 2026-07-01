# Fibre Benchmark: `MpscAsync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedAsync`)

### `MpscUnboundedAsync/Prod-1_Items-100000`
- **Time:** 2.6948 ms – 2.7324 ms – 2.7737 ms  
- **Throughput:** 36.053 Melem/s – 36.598 Melem/s – 37.109 Melem/s

### `MpscUnboundedAsync/Prod-1_Items-1000000`
- **Time:** 27.383 ms – 27.515 ms – 27.684 ms  
- **Throughput:** 36.121 Melem/s – 36.344 Melem/s – 36.519 Melem/s

---

### `MpscUnboundedAsync/Prod-4_Items-100000`
- **Time:** 10.018 ms – 10.257 ms – 10.555 ms  
- **Throughput:** 11.094 Melem/s – 11.484 Melem/s – 11.940 Melem/s

### `MpscUnboundedAsync/Prod-4_Items-1000000`
- **Time:** 82.804 ms – 84.680 ms – 86.671 ms  
- **Throughput:** 11.538 Melem/s – 11.809 Melem/s – 12.077 Melem/s

---

### `MpscUnboundedAsync/Prod-14_Items-100000`
- **Time:** 18.140 ms – 18.390 ms – 18.620 ms  
- **Throughput:** 5.3707 Melem/s – 5.4378 Melem/s – 5.5126 Melem/s

### `MpscUnboundedAsync/Prod-14_Items-1000000`
- **Time:** 102.22 ms – 103.08 ms – 104.12 ms  
- **Throughput:** 5.3914 Melem/s – 5.4067 Melem/s – 5.4158 Melem/s

---

## Bounded Results (`MpscBoundedAsync`)

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-100000`
- **Time:** 9.2331 ms – 9.3021 ms – 9.4013 ms  
- **Throughput:** 10.637 Melem/s – 10.750 Melem/s – 10.831 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-1000000`
- **Time:** 93.518 ms – 96.340 ms – 98.585 ms  
- **Throughput:** 10.143 Melem/s – 10.380 Melem/s – 10.693 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-10000000`
- **Time:** 944.27 ms – 950.32 ms – 956.51 ms  
- **Throughput:** 10.455 Melem/s – 10.523 Melem/s – 10.590 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-100000`
- **Time:** 9.5547 ms – 9.5804 ms – 9.5953 ms  
- **Throughput:** 10.422 Melem/s – 10.438 Melem/s – 10.466 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-1000000`
- **Time:** 95.022 ms – 95.820 ms – 96.402 ms  
- **Throughput:** 10.373 Melem/s – 10.436 Melem/s – 10.524 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-10000000`
- **Time:** 995.30 ms – 1.0063 s – 1.0162 s  
- **Throughput:** 9.8408 Melem/s – 9.9375 Melem/s – 10.047 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-100000`
- **Time:** 10.619 ms – 10.676 ms – 10.747 ms  
- **Throughput:** 9.3049 Melem/s – 9.3667 Melem/s – 9.4173 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-1000000`
- **Time:** 105.49 ms – 106.02 ms – 106.73 ms  
- **Throughput:** 9.3693 Melem/s – 9.4324 Melem/s – 9.4794 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-10000000`
- **Time:** 1.0456 s – 1.0600 s – 1.0761 s  
- **Throughput:** 9.2927 Melem/s – 9.4339 Melem/s – 9.5635 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-100000`
- **Time:** 3.4537 ms – 3.5038 ms – 3.6194 ms  
- **Throughput:** 27.629 Melem/s – 28.540 Melem/s – 28.954 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-1000000`
- **Time:** 34.538 ms – 34.799 ms – 35.066 ms  
- **Throughput:** 28.518 Melem/s – 28.737 Melem/s – 28.954 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-10000000`
- **Time:** 351.45 ms – 354.64 ms – 357.93 ms  
- **Throughput:** 27.939 Melem/s – 28.198 Melem/s – 28.453 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-100000`
- **Time:** 3.6022 ms – 3.6105 ms – 3.6230 ms  
- **Throughput:** 27.602 Melem/s – 27.697 Melem/s – 27.761 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-1000000`
- **Time:** 36.003 ms – 36.339 ms – 36.642 ms  
- **Throughput:** 27.291 Melem/s – 27.519 Melem/s – 27.776 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-10000000`
- **Time:** 360.94 ms – 367.12 ms – 374.06 ms  
- **Throughput:** 26.733 Melem/s – 27.239 Melem/s – 27.706 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-100000`
- **Time:** 3.7876 ms – 3.8172 ms – 3.8349 ms  
- **Throughput:** 26.076 Melem/s – 26.197 Melem/s – 26.402 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-1000000`
- **Time:** 37.649 ms – 37.841 ms – 38.032 ms  
- **Throughput:** 26.293 Melem/s – 26.426 Melem/s – 26.561 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-10000000`
- **Time:** 385.21 ms – 392.51 ms – 399.49 ms  
- **Throughput:** 25.032 Melem/s – 25.477 Melem/s – 25.960 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.5824 ms – 1.5923 ms – 1.6051 ms  
- **Throughput:** 62.302 Melem/s – 62.803 Melem/s – 63.196 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-1000000`
- **Time:** 15.768 ms – 15.793 ms – 15.811 ms  
- **Throughput:** 63.249 Melem/s – 63.318 Melem/s – 63.420 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-10000000`
- **Time:** 157.34 ms – 157.62 ms – 157.83 ms  
- **Throughput:** 63.358 Melem/s – 63.445 Melem/s – 63.555 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-100000`
- **Time:** 1.6574 ms – 1.7161 ms – 1.7861 ms  
- **Throughput:** 55.986 Melem/s – 58.271 Melem/s – 60.334 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-1000000`
- **Time:** 15.746 ms – 15.821 ms – 15.898 ms  
- **Throughput:** 62.901 Melem/s – 63.207 Melem/s – 63.509 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-10000000`
- **Time:** 157.88 ms – 158.22 ms – 158.67 ms  
- **Throughput:** 63.022 Melem/s – 63.204 Melem/s – 63.339 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-100000`
- **Time:** 1.6160 ms – 1.6207 ms – 1.6268 ms  
- **Throughput:** 61.471 Melem/s – 61.703 Melem/s – 61.883 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-1000000`
- **Time:** 15.839 ms – 15.957 ms – 16.071 ms  
- **Throughput:** 62.224 Melem/s – 62.667 Melem/s – 63.137 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-10000000`
- **Time:** 158.67 ms – 159.09 ms – 159.52 ms  
- **Throughput:** 62.690 Melem/s – 62.859 Melem/s – 63.025 Melem/s