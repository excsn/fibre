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

_Engine: `mpsc::bounded_v3` (fusion2 credit-before-claim port)._

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-100000`
- **Time:** 9.0175 ms – 9.1021 ms – 9.2561 ms  
- **Throughput:** 10.804 Melem/s – 10.986 Melem/s – 11.089 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-1000000`
- **Time:** 89.276 ms – 89.486 ms – 89.769 ms  
- **Throughput:** 11.140 Melem/s – 11.175 Melem/s – 11.201 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-1_Items-10000000`
- **Time:** 888.87 ms – 892.90 ms – 897.00 ms  
- **Throughput:** 11.148 Melem/s – 11.199 Melem/s – 11.250 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-100000`
- **Time:** 9.0095 ms – 9.0216 ms – 9.0358 ms  
- **Throughput:** 11.067 Melem/s – 11.084 Melem/s – 11.099 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-1000000`
- **Time:** 89.877 ms – 90.069 ms – 90.381 ms  
- **Throughput:** 11.064 Melem/s – 11.103 Melem/s – 11.126 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-4_Items-10000000`
- **Time:** 890.15 ms – 898.12 ms – 907.08 ms  
- **Throughput:** 11.024 Melem/s – 11.134 Melem/s – 11.234 Melem/s

---

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-100000`
- **Time:** 9.8474 ms – 9.8544 ms – 9.8681 ms  
- **Throughput:** 10.134 Melem/s – 10.148 Melem/s – 10.155 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-1000000`
- **Time:** 97.144 ms – 97.911 ms – 98.595 ms  
- **Throughput:** 10.143 Melem/s – 10.213 Melem/s – 10.294 Melem/s

#### `MpscBoundedAsync/Cap-1_Prod-14_Items-10000000`
- **Time:** 965.26 ms – 969.87 ms – 974.58 ms  
- **Throughput:** 10.261 Melem/s – 10.311 Melem/s – 10.360 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-100000`
- **Time:** 3.1594 ms – 3.1757 ms – 3.1954 ms  
- **Throughput:** 31.295 Melem/s – 31.489 Melem/s – 31.652 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-1000000`
- **Time:** 31.814 ms – 31.903 ms – 31.985 ms  
- **Throughput:** 31.265 Melem/s – 31.345 Melem/s – 31.432 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-1_Items-10000000`
- **Time:** 319.94 ms – 322.35 ms – 324.90 ms  
- **Throughput:** 30.778 Melem/s – 31.022 Melem/s – 31.256 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-100000`
- **Time:** 3.3363 ms – 3.3606 ms – 3.3824 ms  
- **Throughput:** 29.565 Melem/s – 29.757 Melem/s – 29.974 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-1000000`
- **Time:** 32.980 ms – 33.200 ms – 33.501 ms  
- **Throughput:** 29.850 Melem/s – 30.121 Melem/s – 30.321 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-4_Items-10000000`
- **Time:** 337.55 ms – 344.15 ms – 352.81 ms  
- **Throughput:** 28.344 Melem/s – 29.057 Melem/s – 29.625 Melem/s

---

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-100000`
- **Time:** 3.5850 ms – 3.6088 ms – 3.6285 ms  
- **Throughput:** 27.560 Melem/s – 27.710 Melem/s – 27.894 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-1000000`
- **Time:** 34.838 ms – 34.904 ms – 34.997 ms  
- **Throughput:** 28.574 Melem/s – 28.650 Melem/s – 28.705 Melem/s

#### `MpscBoundedAsync/Cap-4_Prod-14_Items-10000000`
- **Time:** 349.33 ms – 354.40 ms – 360.04 ms  
- **Throughput:** 27.775 Melem/s – 28.216 Melem/s – 28.626 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-100000`
- **Time:** 1.4791 ms – 1.4803 ms – 1.4820 ms  
- **Throughput:** 67.477 Melem/s – 67.552 Melem/s – 67.609 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-1000000`
- **Time:** 14.630 ms – 14.645 ms – 14.675 ms  
- **Throughput:** 68.143 Melem/s – 68.282 Melem/s – 68.353 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-1_Items-10000000`
- **Time:** 146.66 ms – 146.83 ms – 147.06 ms  
- **Throughput:** 67.999 Melem/s – 68.108 Melem/s – 68.186 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-100000`
- **Time:** 1.4877 ms – 1.4923 ms – 1.4953 ms  
- **Throughput:** 66.875 Melem/s – 67.012 Melem/s – 67.218 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-1000000`
- **Time:** 14.768 ms – 14.794 ms – 14.816 ms  
- **Throughput:** 67.496 Melem/s – 67.597 Melem/s – 67.715 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-4_Items-10000000`
- **Time:** 147.29 ms – 147.65 ms – 147.93 ms  
- **Throughput:** 67.600 Melem/s – 67.730 Melem/s – 67.893 Melem/s

---

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-100000`
- **Time:** 1.5316 ms – 1.5374 ms – 1.5493 ms  
- **Throughput:** 64.546 Melem/s – 65.044 Melem/s – 65.293 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-1000000`
- **Time:** 14.818 ms – 14.878 ms – 14.909 ms  
- **Throughput:** 67.074 Melem/s – 67.212 Melem/s – 67.488 Melem/s

#### `MpscBoundedAsync/Cap-128_Prod-14_Items-10000000`
- **Time:** 148.26 ms – 148.50 ms – 148.70 ms  
- **Throughput:** 67.252 Melem/s – 67.342 Melem/s – 67.451 Melem/s


## Bounded Batch Results (`MpscBoundedAsyncBatch`)

_Engine: `mpsc::bounded_v3`. Each producer sends its share via `send_batch_mut().await`,
the consumer drains via `recv_batch_mut().await`, both reusing a caller-owned buffer.
Cells are **median throughput (Melem/s)** over the batch-size axis {8, 64, 512}._

### Capacity: 1 (`Cap-1`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 9.17 | 8.79 | 7.76 |
| 1 | 1M | 8.81 | 8.62 | 7.55 |
| 1 | 10M | 9.10 | 8.73 | 7.43 |
| 4 | 100k | 8.41 | 8.14 | 7.08 |
| 4 | 1M | 8.47 | 8.22 | 6.96 |
| 4 | 10M | 8.39 | 8.17 | 6.92 |
| 14 | 100k | 7.68 | 7.45 | 5.97 |
| 14 | 1M | 7.52 | 7.56 | 5.92 |
| 14 | 10M | 7.49 | 7.26 | 5.79 |

### Capacity: 4 (`Cap-4`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 33.0 | 33.4 | 28.2 |
| 1 | 1M | 33.2 | 33.3 | 28.1 |
| 1 | 10M | 33.3 | 33.6 | 28.1 |
| 4 | 100k | 32.6 | 32.1 | 25.4 |
| 4 | 1M | 33.1 | 32.5 | 25.9 |
| 4 | 10M | 33.1 | 32.4 | 25.7 |
| 14 | 100k | 30.9 | 29.5 | 21.9 |
| 14 | 1M | 31.6 | 30.4 | 22.1 |
| 14 | 10M | 31.9 | 30.4 | 21.8 |

### Capacity: 128 (`Cap-128`)

| Prod | Items | Batch-8 | Batch-64 | Batch-512 |
|---|---|---|---|---|
| 1 | 100k | 133 | 198 | 224 |
| 1 | 1M | 133 | 203 | 229 |
| 1 | 10M | 127 | 206 | 229 |
| 4 | 100k | 57.1 | 122 | 220 |
| 4 | 1M | 56.6 | 128 | 227 |
| 4 | 10M | 56.5 | 130 | 227 |
| 14 | 100k | 40.2 | 111 | 212 |
| 14 | 1M | 36.9 | 117 | 214 |
| 14 | 10M | 35.4 | 119 | 222 |

**Notes.** The `_mut` batch APIs lifted every cell (e.g. `Cap-128/Prod-1/Batch-8`
~54 → ~133 Melem/s). The async trend still inverts sync: **bigger batches win**. At
`Cap-128` throughput climbs with batch size — `Cap-128/Prod-1` goes ~133 → ~203 → ~229
Melem/s across `Batch-8 → 64 → 512` (and the multi-producer rows more steeply, e.g.
`Prod-14` ~35 → ~119 → ~222), since larger batches amortize the per-wake poll/waker
overhead that dominates the async path. Small caps (1, 4) are wake-bound and slightly
*prefer* small batches (`Batch-512` dips a little there).
