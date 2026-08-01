# Fibre Benchmark: Oneshot
**Test Machine:** MacBook M4 Pro

Six suites: the clonable `oneshot()` (packed-word core, 2026-08 rebuild), the single-sender `oneshot::exclusive()`, and `tokio::sync::oneshot` as reference. Plain suites time create+send+recv per op; `Xfer` suites pre-create the channels and time only send+recv (the per-channel Arc allocation dominates the full cycle, so `Xfer` is the number that compares the transfer protocols).

Pre-rebuild baseline for `OneshotAsync` (mutex + three-atomic core): 10.29 Melem/s at Ops-100, 10.37 Melem/s at Ops-1000.

## Results

### `OneshotAsync/Ops-100`
- **Time:** 3.7309 µs - 3.7778 µs - 3.8286 µs
- **Throughput:** 26.119 Melem/s - 26.471 Melem/s - 26.803 Melem/s

### `OneshotAsync/Ops-1000`
- **Time:** 37.160 µs - 50.115 µs - 65.289 µs
- **Throughput:** 15.316 Melem/s - 19.954 Melem/s - 26.911 Melem/s

### `OneshotAsyncXfer/Ops-100`
- **Time:** 2.3017 µs - 2.3029 µs - 2.3053 µs
- **Throughput:** 43.379 Melem/s - 43.423 Melem/s - 43.446 Melem/s

### `OneshotAsyncXfer/Ops-1000`
- **Time:** 22.709 µs - 22.739 µs - 22.771 µs
- **Throughput:** 43.915 Melem/s - 43.978 Melem/s - 44.035 Melem/s

### `OneshotExclusiveAsync/Ops-100`
- **Time:** 2.9547 µs - 2.9705 µs - 2.9819 µs
- **Throughput:** 33.535 Melem/s - 33.665 Melem/s - 33.845 Melem/s

### `OneshotExclusiveAsync/Ops-1000`
- **Time:** 29.022 µs - 29.148 µs - 29.282 µs
- **Throughput:** 34.150 Melem/s - 34.307 Melem/s - 34.456 Melem/s

### `OneshotExclusiveAsyncXfer/Ops-100`
- **Time:** 1.6552 µs - 1.6648 µs - 1.6707 µs
- **Throughput:** 59.855 Melem/s - 60.067 Melem/s - 60.416 Melem/s

### `OneshotExclusiveAsyncXfer/Ops-1000`
- **Time:** 15.974 µs - 16.056 µs - 16.105 µs
- **Throughput:** 62.093 Melem/s - 62.281 Melem/s - 62.603 Melem/s

### `OneshotTokioAsync/Ops-100`
- **Time:** 3.808 µs - 3.835 µs - 3.874 µs
- **Throughput:** 25.810 Melem/s - 26.076 Melem/s - 26.257 Melem/s

### `OneshotTokioAsync/Ops-1000`
- **Time:** 37.559 µs - 37.744 µs - 37.944 µs
- **Throughput:** 26.354 Melem/s - 26.494 Melem/s - 26.625 Melem/s

### `OneshotTokioAsyncXfer/Ops-100`
- **Time:** 2.235 µs - 2.271 µs - 2.329 µs
- **Throughput:** 42.941 Melem/s - 44.030 Melem/s - 44.742 Melem/s

### `OneshotTokioAsyncXfer/Ops-1000`
- **Time:** 26.189 µs - 30.614 µs - 35.488 µs
- **Throughput:** 28.179 Melem/s - 32.665 Melem/s - 38.184 Melem/s
