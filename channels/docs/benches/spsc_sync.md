# Fibre Benchmark: `SpscSync`
**Test Machine:** MacBook M4 Pro

Measured with the real-concurrency harness: a spawned producer thread and a spawned
consumer thread per iteration (`benches/spsc_bounded.rs`). Numbers are not comparable
to older revisions of this file, which measured a same-thread send/recv loop with no
cross-thread synchronization.

## Results

### `SpscSync/Cap-1_Items-1000`
- **Time:** 3.9490 ms – 3.9984 ms – 4.0953 ms  
- **Throughput:** 244.18 Kelem/s – 250.10 Kelem/s – 253.23 Kelem/s

### `SpscSync/Cap-1_Items-100000`
- **Time:** 264.34 ms – 315.91 ms – 363.73 ms  
- **Throughput:** 274.93 Kelem/s – 316.54 Kelem/s – 378.30 Kelem/s

### `SpscSync/Cap-1_Items-1000000`
- **Time:** 3.3348 s – 3.5054 s – 3.6700 s  
- **Throughput:** 272.48 Kelem/s – 285.28 Kelem/s – 299.87 Kelem/s

---

### `SpscSync/Cap-128_Items-1000`
- **Time:** 65.912 µs – 67.380 µs – 69.718 µs  
- **Throughput:** 14.344 Melem/s – 14.841 Melem/s – 15.172 Melem/s

### `SpscSync/Cap-128_Items-100000`
- **Time:** 3.8093 ms – 3.8405 ms – 3.8627 ms  
- **Throughput:** 25.889 Melem/s – 26.039 Melem/s – 26.252 Melem/s

### `SpscSync/Cap-128_Items-1000000`
- **Time:** 36.526 ms – 37.015 ms – 37.578 ms  
- **Throughput:** 26.611 Melem/s – 27.016 Melem/s – 27.378 Melem/s

---

### `SpscSync/Cap-1024_Items-1000`
- **Time:** 32.443 µs – 33.411 µs – 34.654 µs  
- **Throughput:** 28.857 Melem/s – 29.930 Melem/s – 30.823 Melem/s

### `SpscSync/Cap-1024_Items-100000`
- **Time:** 2.2816 ms – 2.3488 ms – 2.3970 ms  
- **Throughput:** 41.719 Melem/s – 42.576 Melem/s – 43.830 Melem/s

### `SpscSync/Cap-1024_Items-1000000`
- **Time:** 22.069 ms – 22.496 ms – 23.311 ms  
- **Throughput:** 42.898 Melem/s – 44.453 Melem/s – 45.313 Melem/s

## Batch Results (`SpscSyncBatch`)

`send_batch`/`recv_batch` with the same spawned producer/consumer harness. Axes:
capacity × total items × batch size.

### `SpscSyncBatch/Cap-1_Items-1000_Batch-8`
- **Time:** 4.2135 ms – 4.2592 ms – 4.3070 ms  
- **Throughput:** 232.18 Kelem/s – 234.78 Kelem/s – 237.33 Kelem/s

### `SpscSyncBatch/Cap-1_Items-1000_Batch-64`
- **Time:** 4.2476 ms – 4.2757 ms – 4.3073 ms  
- **Throughput:** 232.17 Kelem/s – 233.88 Kelem/s – 235.43 Kelem/s

### `SpscSyncBatch/Cap-1_Items-1000_Batch-512`
- **Time:** 4.2345 ms – 4.2705 ms – 4.3079 ms  
- **Throughput:** 232.13 Kelem/s – 234.16 Kelem/s – 236.16 Kelem/s

### `SpscSyncBatch/Cap-1_Items-100000_Batch-8`
- **Time:** 409.72 ms – 421.18 ms – 433.07 ms  
- **Throughput:** 230.91 Kelem/s – 237.43 Kelem/s – 244.07 Kelem/s

### `SpscSyncBatch/Cap-1_Items-100000_Batch-64`
- **Time:** 427.30 ms – 435.97 ms – 445.42 ms  
- **Throughput:** 224.51 Kelem/s – 229.37 Kelem/s – 234.03 Kelem/s

### `SpscSyncBatch/Cap-1_Items-100000_Batch-512`
- **Time:** 424.81 ms – 426.63 ms – 428.36 ms  
- **Throughput:** 233.45 Kelem/s – 234.40 Kelem/s – 235.40 Kelem/s

### `SpscSyncBatch/Cap-1_Items-1000000_Batch-8`
- **Time:** 4.1052 s – 4.1238 s – 4.1414 s  
- **Throughput:** 241.46 Kelem/s – 242.50 Kelem/s – 243.59 Kelem/s

### `SpscSyncBatch/Cap-1_Items-1000000_Batch-64`
- **Time:** 4.1928 s – 4.2145 s – 4.2379 s  
- **Throughput:** 235.97 Kelem/s – 237.28 Kelem/s – 238.51 Kelem/s

### `SpscSyncBatch/Cap-1_Items-1000000_Batch-512`
- **Time:** 4.2372 s – 4.2849 s – 4.3478 s  
- **Throughput:** 230.00 Kelem/s – 233.38 Kelem/s – 236.01 Kelem/s

---

### `SpscSyncBatch/Cap-128_Items-1000_Batch-8`
- **Time:** 58.470 µs – 59.268 µs – 59.774 µs  
- **Throughput:** 16.730 Melem/s – 16.873 Melem/s – 17.103 Melem/s

### `SpscSyncBatch/Cap-128_Items-1000_Batch-64`
- **Time:** 60.444 µs – 60.836 µs – 61.172 µs  
- **Throughput:** 16.347 Melem/s – 16.438 Melem/s – 16.544 Melem/s

### `SpscSyncBatch/Cap-128_Items-1000_Batch-512`
- **Time:** 60.118 µs – 60.390 µs – 60.803 µs  
- **Throughput:** 16.447 Melem/s – 16.559 Melem/s – 16.634 Melem/s

### `SpscSyncBatch/Cap-128_Items-100000_Batch-8`
- **Time:** 3.6098 ms – 3.6568 ms – 3.7155 ms  
- **Throughput:** 26.914 Melem/s – 27.346 Melem/s – 27.702 Melem/s

### `SpscSyncBatch/Cap-128_Items-100000_Batch-64`
- **Time:** 3.7375 ms – 3.8391 ms – 3.9487 ms  
- **Throughput:** 25.325 Melem/s – 26.048 Melem/s – 26.756 Melem/s

### `SpscSyncBatch/Cap-128_Items-100000_Batch-512`
- **Time:** 4.1180 ms – 4.1473 ms – 4.1703 ms  
- **Throughput:** 23.979 Melem/s – 24.112 Melem/s – 24.284 Melem/s

### `SpscSyncBatch/Cap-128_Items-1000000_Batch-8`
- **Time:** 34.625 ms – 35.617 ms – 36.211 ms  
- **Throughput:** 27.616 Melem/s – 28.077 Melem/s – 28.881 Melem/s

### `SpscSyncBatch/Cap-128_Items-1000000_Batch-64`
- **Time:** 34.610 ms – 35.698 ms – 36.819 ms  
- **Throughput:** 27.160 Melem/s – 28.013 Melem/s – 28.894 Melem/s

### `SpscSyncBatch/Cap-128_Items-1000000_Batch-512`
- **Time:** 40.594 ms – 40.959 ms – 41.172 ms  
- **Throughput:** 24.289 Melem/s – 24.415 Melem/s – 24.634 Melem/s

---

### `SpscSyncBatch/Cap-1024_Items-1000_Batch-8`
- **Time:** 31.935 µs – 32.588 µs – 33.173 µs  
- **Throughput:** 30.145 Melem/s – 30.687 Melem/s – 31.314 Melem/s

### `SpscSyncBatch/Cap-1024_Items-1000_Batch-64`
- **Time:** 30.610 µs – 30.973 µs – 31.453 µs  
- **Throughput:** 31.793 Melem/s – 32.286 Melem/s – 32.669 Melem/s

### `SpscSyncBatch/Cap-1024_Items-1000_Batch-512`
- **Time:** 29.052 µs – 30.010 µs – 31.133 µs  
- **Throughput:** 32.120 Melem/s – 33.323 Melem/s – 34.422 Melem/s

### `SpscSyncBatch/Cap-1024_Items-100000_Batch-8`
- **Time:** 673.89 µs – 693.39 µs – 715.80 µs  
- **Throughput:** 139.70 Melem/s – 144.22 Melem/s – 148.39 Melem/s

### `SpscSyncBatch/Cap-1024_Items-100000_Batch-64`
- **Time:** 661.33 µs – 693.14 µs – 733.48 µs  
- **Throughput:** 136.34 Melem/s – 144.27 Melem/s – 151.21 Melem/s

### `SpscSyncBatch/Cap-1024_Items-100000_Batch-512`
- **Time:** 533.60 µs – 540.68 µs – 545.85 µs  
- **Throughput:** 183.20 Melem/s – 184.95 Melem/s – 187.41 Melem/s

### `SpscSyncBatch/Cap-1024_Items-1000000_Batch-8`
- **Time:** 6.5033 ms – 6.5859 ms – 6.6786 ms  
- **Throughput:** 149.73 Melem/s – 151.84 Melem/s – 153.77 Melem/s

### `SpscSyncBatch/Cap-1024_Items-1000000_Batch-64`
- **Time:** 5.6793 ms – 5.7377 ms – 5.8250 ms  
- **Throughput:** 171.67 Melem/s – 174.29 Melem/s – 176.08 Melem/s

### `SpscSyncBatch/Cap-1024_Items-1000000_Batch-512`
- **Time:** 5.3594 ms – 5.5660 ms – 5.9541 ms  
- **Throughput:** 167.95 Melem/s – 179.66 Melem/s – 186.59 Melem/s
