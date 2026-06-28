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
- **Time:** 8.9112 ms – 8.9745 ms – 9.0601 ms  
- **Throughput:** 11.037 Melem/s – 11.143 Melem/s – 11.222 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-1000000`
- **Time:** 89.753 ms – 90.331 ms – 90.957 ms  
- **Throughput:** 10.994 Melem/s – 11.070 Melem/s – 11.142 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-10000000`
- **Time:** 901.18 ms – 907.22 ms – 913.38 ms  
- **Throughput:** 10.948 Melem/s – 11.023 Melem/s – 11.097 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-100000`
- **Time:** 9.3087 ms – 9.3324 ms – 9.3675 ms  
- **Throughput:** 10.675 Melem/s – 10.715 Melem/s – 10.743 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-1000000`
- **Time:** 92.119 ms – 93.014 ms – 94.177 ms  
- **Throughput:** 10.618 Melem/s – 10.751 Melem/s – 10.856 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-10000000`
- **Time:** 921.92 ms – 928.70 ms – 935.37 ms  
- **Throughput:** 10.691 Melem/s – 10.768 Melem/s – 10.847 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-100000`
- **Time:** 10.234 ms – 10.319 ms – 10.426 ms  
- **Throughput:** 9.5911 Melem/s – 9.6907 Melem/s – 9.7712 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-1000000`
- **Time:** 102.56 ms – 104.09 ms – 105.43 ms  
- **Throughput:** 9.4854 Melem/s – 9.6070 Melem/s – 9.7500 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-10000000`
- **Time:** 1.0223 s – 1.0326 s – 1.0434 s  
- **Throughput:** 9.5841 Melem/s – 9.6847 Melem/s – 9.7823 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-100000`
- **Time:** 3.4509 ms – 3.5816 ms – 3.7432 ms  
- **Throughput:** 26.715 Melem/s – 27.921 Melem/s – 28.978 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-1000000`
- **Time:** 34.039 ms – 34.524 ms – 35.355 ms  
- **Throughput:** 28.285 Melem/s – 28.965 Melem/s – 29.378 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-10000000`
- **Time:** 344.24 ms – 347.08 ms – 350.03 ms  
- **Throughput:** 28.569 Melem/s – 28.812 Melem/s – 29.049 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-100000`
- **Time:** 3.6105 ms – 3.7042 ms – 3.7915 ms  
- **Throughput:** 26.375 Melem/s – 26.997 Melem/s – 27.697 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-1000000`
- **Time:** 34.696 ms – 34.916 ms – 35.104 ms  
- **Throughput:** 28.486 Melem/s – 28.640 Melem/s – 28.822 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-10000000`
- **Time:** 349.05 ms – 354.25 ms – 359.42 ms  
- **Throughput:** 27.822 Melem/s – 28.229 Melem/s – 28.649 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-100000`
- **Time:** 3.6191 ms – 3.6400 ms – 3.6780 ms  
- **Throughput:** 27.188 Melem/s – 27.472 Melem/s – 27.631 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-1000000`
- **Time:** 36.934 ms – 38.024 ms – 39.266 ms  
- **Throughput:** 25.467 Melem/s – 26.299 Melem/s – 27.075 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-10000000`
- **Time:** 383.55 ms – 394.35 ms – 404.85 ms  
- **Throughput:** 24.701 Melem/s – 25.358 Melem/s – 26.072 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.6439 ms – 1.6513 ms – 1.6602 ms  
- **Throughput:** 60.235 Melem/s – 60.557 Melem/s – 60.829 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-1000000`
- **Time:** 16.361 ms – 16.424 ms – 16.478 ms  
- **Throughput:** 60.685 Melem/s – 60.888 Melem/s – 61.121 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-10000000`
- **Time:** 164.14 ms – 164.62 ms – 165.31 ms  
- **Throughput:** 60.491 Melem/s – 60.745 Melem/s – 60.925 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-100000`
- **Time:** 3.8773 ms – 3.9304 ms – 3.9768 ms  
- **Throughput:** 25.146 Melem/s – 25.442 Melem/s – 25.791 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-1000000`
- **Time:** 39.039 ms – 39.496 ms – 40.001 ms  
- **Throughput:** 24.999 Melem/s – 25.319 Melem/s – 25.615 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-10000000`
- **Time:** 390.60 ms – 395.68 ms – 401.37 ms  
- **Throughput:** 24.914 Melem/s – 25.273 Melem/s – 25.601 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-100000`
- **Time:** 3.9252 ms – 3.9665 ms – 4.0054 ms  
- **Throughput:** 24.966 Melem/s – 25.211 Melem/s – 25.476 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-1000000`
- **Time:** 39.753 ms – 40.045 ms – 40.346 ms  
- **Throughput:** 24.786 Melem/s – 24.972 Melem/s – 25.155 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-10000000`
- **Time:** 394.51 ms – 397.73 ms – 400.64 ms  
- **Throughput:** 24.960 Melem/s – 25.143 Melem/s – 25.348 Melem/s