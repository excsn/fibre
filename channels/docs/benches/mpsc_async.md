# Fibre Benchmark: `MpscAsync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscAsync`)

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
- **Time:** 8.9140 ms – 9.0460 ms – 9.1912 ms  
- **Throughput:** 10.880 Melem/s – 11.055 Melem/s – 11.218 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-1000000`
- **Time:** 90.012 ms – 90.240 ms – 90.750 ms  
- **Throughput:** 11.020 Melem/s – 11.080 Melem/s – 11.109 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-10000000`
- **Time:** 900.12 ms – 902.40 ms – 907.50 ms  
- **Throughput:** 11.020 Melem/s – 11.080 Melem/s – 11.109 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-100000`
- **Time:** 9.4271 ms – 9.4623 ms – 9.4919 ms  
- **Throughput:** 10.535 Melem/s – 10.568 Melem/s – 10.608 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-1000000`
- **Time:** 93.871 ms – 94.156 ms – 94.595 ms  
- **Throughput:** 10.571 Melem/s – 10.621 Melem/s – 10.653 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-10000000`
- **Time:** 918.71 ms – 931.06 ms – 943.39 ms  
- **Throughput:** 10.600 Melem/s – 10.740 Melem/s – 10.885 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-100000`
- **Time:** 10.735 ms – 10.759 ms – 10.806 ms  
- **Throughput:** 9.2543 Melem/s – 9.2946 Melem/s – 9.3155 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-1000000`
- **Time:** 105.08 ms – 106.12 ms – 107.61 ms  
- **Throughput:** 9.2925 Melem/s – 9.4232 Melem/s – 9.5168 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-10000000`
- **Time:** 1.0383 s – 1.0544 s – 1.0674 s  
- **Throughput:** 9.3687 Melem/s – 9.4843 Melem/s – 9.6308 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-100000`
- **Time:** 3.3935 ms – 3.4090 ms – 3.4257 ms  
- **Throughput:** 29.191 Melem/s – 29.334 Melem/s – 29.468 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-1000000`
- **Time:** 34.115 ms – 34.512 ms – 34.753 ms  
- **Throughput:** 28.774 Melem/s – 28.976 Melem/s – 29.312 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-10000000`
- **Time:** 349.05 ms – 351.14 ms – 353.35 ms  
- **Throughput:** 28.301 Melem/s – 28.478 Melem/s – 28.649 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-100000`
- **Time:** 3.5567 ms – 3.5614 ms – 3.5673 ms  
- **Throughput:** 28.032 Melem/s – 28.079 Melem/s – 28.116 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-1000000`
- **Time:** 35.323 ms – 35.509 ms – 35.939 ms  
- **Throughput:** 27.825 Melem/s – 28.162 Melem/s – 28.310 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-10000000`
- **Time:** 353.39 ms – 357.66 ms – 361.88 ms  
- **Throughput:** 27.634 Melem/s – 27.960 Melem/s – 28.297 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-100000`
- **Time:** 3.7624 ms – 3.7765 ms – 3.7938 ms  
- **Throughput:** 26.359 Melem/s – 26.480 Melem/s – 26.579 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-1000000`
- **Time:** 36.943 ms – 37.310 ms – 37.678 ms  
- **Throughput:** 26.541 Melem/s – 26.803 Melem/s – 27.069 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-10000000`
- **Time:** 375.41 ms – 381.87 ms – 387.98 ms  
- **Throughput:** 25.775 Melem/s – 26.187 Melem/s – 26.637 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.6451 ms – 1.6468 ms – 1.6506 ms  
- **Throughput:** 60.584 Melem/s – 60.723 Melem/s – 60.785 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-1000000`
- **Time:** 16.385 ms – 16.420 ms – 16.453 ms  
- **Throughput:** 60.780 Melem/s – 60.901 Melem/s – 61.031 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-10000000`
- **Time:** 163.82 ms – 164.19 ms – 164.49 ms  
- **Throughput:** 60.795 Melem/s – 60.905 Melem/s – 61.041 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-100000`
- **Time:** 4.2100 ms – 4.2829 ms – 4.3876 ms  
- **Throughput:** 22.792 Melem/s – 23.349 Melem/s – 23.753 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-1000000`
- **Time:** 42.055 ms – 42.158 ms – 42.253 ms  
- **Throughput:** 23.667 Melem/s – 23.720 Melem/s – 23.778 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-10000000`
- **Time:** 426.47 ms – 428.16 ms – 429.90 ms  
- **Throughput:** 23.261 Melem/s – 23.356 Melem/s – 23.448 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-100000`
- **Time:** 4.2383 ms – 4.2746 ms – 4.3001 ms  
- **Throughput:** 23.255 Melem/s – 23.394 Melem/s – 23.595 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-1000000`
- **Time:** 42.153 ms – 42.322 ms – 42.617 ms  
- **Throughput:** 23.465 Melem/s – 23.628 Melem/s – 23.723 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-10000000`
- **Time:** 437.56 ms – 447.68 ms – 458.62 ms  
- **Throughput:** 21.805 Melem/s – 22.337 Melem/s – 22.854 Melem/s