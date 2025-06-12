# Fibre Benchmark: Async vs Sync
**Test Machine:** MacBook M4 Pro

## Legend

b0 - Rendevous
b1 - Capacity 1
bn - Unbounded
contended - core_count * 64

## Async

### `async::mpmc/b0`
- **Time:** 79.352 ms – 81.551 ms – 84.112 ms  
- **Throughput:** 12.466 Melem/s – 12.858 Melem/s – 13.214 Melem/s

### `async::mpmc/b0_contended`
- **Time:** 65.072 ms – 70.306 ms – 76.104 ms  
- **Throughput:** 13.778 Melem/s – 14.915 Melem/s – 16.114 Melem/s

### `async::mpmc/b1`
- **Time:** 43.752 ms – 47.376 ms – 51.508 ms  
- **Throughput:** 20.357 Melem/s – 22.133 Melem/s – 23.966 Melem/s

### `async::mpmc/bn`
- **Time:** 23.318 ms – 24.378 ms – 26.058 ms  
- **Throughput:** 40.240 Melem/s – 43.013 Melem/s – 44.968 Melem/s

---

### `async::mpsc/b0`
- **Time:** 194.18 ms – 198.85 ms – 204.14 ms  
- **Throughput:** 5.1365 Melem/s – 5.2732 Melem/s – 5.3999 Melem/s

### `async::mpsc/b0_contended`
- **Time:** 631.15 ms – 689.02 ms – 748.67 ms  
- **Throughput:** 1.4006 Melem/s – 1.5218 Melem/s – 1.6614 Melem/s

### `async::mpsc/b1`
- **Time:** 46.901 ms – 51.818 ms – 59.962 ms  
- **Throughput:** 17.487 Melem/s – 20.236 Melem/s – 22.357 Melem/s

### `async::mpsc/bn`
- **Time:** 16.612 ms – 16.948 ms – 17.140 ms  
- **Throughput:** 61.179 Melem/s – 61.871 Melem/s – 63.122 Melem/s

---

### `async::spsc/b0`
- **Time:** 50.517 ms – 52.499 ms – 54.438 ms  
- **Throughput:** 19.262 Melem/s – 19.973 Melem/s – 20.757 Melem/s

### `async::spsc/b1`
- **Time:** 115.24 ms – 154.28 ms – 195.70 ms  
- **Throughput:** 5.3581 Melem/s – 6.7965 Melem/s – 9.0994 Melem/s

## Sync

### `sync::mpmc/b0`
- **Time:** 383.19 ms – 389.00 ms – 395.06 ms  
- **Throughput:** 2.6542 Melem/s – 2.6956 Melem/s – 2.7364 Melem/s

### `sync::mpmc/b0_contended`
- **Time:** 672.33 ms – 680.04 ms – 690.00 ms  
- **Throughput:** 1.5197 Melem/s – 1.5419 Melem/s – 1.5596 Melem/s

### `sync::mpmc/b1`
- **Time:** 247.30 ms – 260.17 ms – 275.33 ms  
- **Throughput:** 3.8085 Melem/s – 4.0303 Melem/s – 4.2401 Melem/s

### `sync::mpmc/bn`
- **Time:** 42.586 ms – 46.115 ms – 50.393 ms  
- **Throughput:** 20.808 Melem/s – 22.738 Melem/s – 24.623 Melem/s

---

### `sync::mpsc/b0`
- **Time:** 1.0611 s – 1.1050 s – 1.1488 s  
- **Throughput:** 912.79 Kelem/s – 948.97 Kelem/s – 988.19 Kelem/s

### `sync::mpsc/b0_contended`
- **Time:** 1.9495 s – 1.9582 s – 1.9674 s  
- **Throughput:** 532.97 Kelem/s – 535.49 Kelem/s – 537.88 Kelem/s

### `sync::mpsc/b1`
- **Time:** 280.27 ms – 298.21 ms – 316.58 ms  
- **Throughput:** 3.3122 Melem/s – 3.5162 Melem/s – 3.7413 Melem/s

### `sync::mpsc/bn`
- **Time:** 21.592 ms – 24.221 ms – 26.237 ms  
- **Throughput:** 39.966 Melem/s – 43.293 Melem/s – 48.563 Melem/s

---

### `sync::spsc/b0`
- **Time:** 130.00 ms – 132.45 ms – 135.46 ms  
- **Throughput:** 7.7411 Melem/s – 7.9166 Melem/s – 8.0658 Melem/s

### `sync::spsc/b1`
- **Time:** 115.79 ms – 118.39 ms – 121.78 ms  
- **Throughput:** 8.6107 Melem/s – 8.8571 Melem/s – 9.0560 Melem/s
