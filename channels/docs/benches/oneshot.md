# Fibre Benchmark: Oneshot
**Test Machine:** MacBook M4 Pro

Every group runs the clonable `oneshot()`, the single-sender `oneshot::exclusive()`, and `tokio::sync::oneshot` as reference.

## Full cycle

Channel created, sent and received inside the timed loop, so the per-channel allocation is included.

### `OneshotAsync/Ops-100`
- **Time:** 2.3243 µs - 2.3472 µs - 2.3652 µs
- **Throughput:** 42.279 Melem/s - 42.604 Melem/s - 43.024 Melem/s

### `OneshotAsync/Ops-1000`
- **Time:** 21.926 µs - 22.431 µs - 22.798 µs
- **Throughput:** 43.863 Melem/s - 44.581 Melem/s - 45.607 Melem/s

### `OneshotExclusiveAsync/Ops-100`
- **Time:** 1.8373 µs - 1.8403 µs - 1.8451 µs
- **Throughput:** 54.196 Melem/s - 54.338 Melem/s - 54.426 Melem/s

### `OneshotExclusiveAsync/Ops-1000`
- **Time:** 18.060 µs - 18.271 µs - 18.457 µs
- **Throughput:** 54.181 Melem/s - 54.732 Melem/s - 55.371 Melem/s

### `OneshotTokioAsync/Ops-100`
- **Time:** 2.4569 µs - 2.4816 µs - 2.5118 µs
- **Throughput:** 39.811 Melem/s - 40.297 Melem/s - 40.702 Melem/s

### `OneshotTokioAsync/Ops-1000`
- **Time:** 24.050 µs - 24.191 µs - 24.365 µs
- **Throughput:** 41.043 Melem/s - 41.338 Melem/s - 41.580 Melem/s

## Transfer only

Channels pre-created outside the timed loop, so only send and recv are measured.

### `OneshotAsyncXfer/Ops-100`
- **Time:** 1.4562 µs - 1.4585 µs - 1.4603 µs
- **Throughput:** 68.479 Melem/s - 68.565 Melem/s - 68.670 Melem/s

### `OneshotAsyncXfer/Ops-1000`
- **Time:** 14.344 µs - 14.383 µs - 14.417 µs
- **Throughput:** 69.363 Melem/s - 69.525 Melem/s - 69.714 Melem/s

### `OneshotExclusiveAsyncXfer/Ops-100`
- **Time:** 1.0412 µs - 1.0476 µs - 1.0542 µs
- **Throughput:** 94.860 Melem/s - 95.458 Melem/s - 96.041 Melem/s

### `OneshotExclusiveAsyncXfer/Ops-1000`
- **Time:** 10.124 µs - 10.230 µs - 10.326 µs
- **Throughput:** 96.841 Melem/s - 97.750 Melem/s - 98.775 Melem/s

### `OneshotTokioAsyncXfer/Ops-100`
- **Time:** 1.4289 µs - 1.4335 µs - 1.4364 µs
- **Throughput:** 69.616 Melem/s - 69.759 Melem/s - 69.983 Melem/s

### `OneshotTokioAsyncXfer/Ops-1000`
- **Time:** 14.032 µs - 14.204 µs - 14.355 µs
- **Throughput:** 69.661 Melem/s - 70.402 Melem/s - 71.264 Melem/s

## Inside a request record

An `Arc<Req>` per op alongside the channel, modelling a oneshot carried by an in-flight request: one allocation for the record plus one for the channel.

### `OneshotRecordAsync/Ops-100`
- **Time:** 4.5296 µs - 4.5764 µs - 4.6234 µs
- **Throughput:** 21.629 Melem/s - 21.851 Melem/s - 22.077 Melem/s

### `OneshotRecordAsync/Ops-1000`
- **Time:** 44.540 µs - 44.967 µs - 45.678 µs
- **Throughput:** 21.893 Melem/s - 22.238 Melem/s - 22.452 Melem/s

### `OneshotExclusiveRecordAsync/Ops-100`
- **Time:** 3.8562 µs - 3.9702 µs - 4.1332 µs
- **Throughput:** 24.194 Melem/s - 25.188 Melem/s - 25.933 Melem/s

### `OneshotExclusiveRecordAsync/Ops-1000`
- **Time:** 38.128 µs - 39.124 µs - 41.092 µs
- **Throughput:** 24.335 Melem/s - 25.560 Melem/s - 26.228 Melem/s

### `OneshotTokioRecordAsync/Ops-100`
- **Time:** 4.3494 µs - 4.4147 µs - 4.5061 µs
- **Throughput:** 22.192 Melem/s - 22.651 Melem/s - 22.991 Melem/s

### `OneshotTokioRecordAsync/Ops-1000`
- **Time:** 43.921 µs - 44.361 µs - 44.804 µs
- **Throughput:** 22.320 Melem/s - 22.542 Melem/s - 22.768 Melem/s

## Cross-thread

Channels pre-created, then a second OS thread sends all of them while the receiver awaits; the sender wins the race outright, so the receiver parks on 0 of 10000 receives (78 of 10000 for tokio).

### `OneshotHandoff/Ops-1000`
- **Time:** 13.230 µs - 13.247 µs - 13.257 µs
- **Throughput:** 75.429 Melem/s - 75.489 Melem/s - 75.586 Melem/s

### `OneshotHandoff/Ops-10000`
- **Time:** 101.69 µs - 103.93 µs - 108.29 µs
- **Throughput:** 92.344 Melem/s - 96.218 Melem/s - 98.338 Melem/s

### `OneshotExclusiveHandoff/Ops-1000`
- **Time:** 12.071 µs - 12.101 µs - 12.130 µs
- **Throughput:** 82.440 Melem/s - 82.637 Melem/s - 82.845 Melem/s

### `OneshotExclusiveHandoff/Ops-10000`
- **Time:** 89.607 µs - 90.279 µs - 90.925 µs
- **Throughput:** 109.98 Melem/s - 110.77 Melem/s - 111.60 Melem/s

### `OneshotTokioHandoff/Ops-1000`
- **Time:** 14.553 µs - 14.601 µs - 14.694 µs
- **Throughput:** 68.057 Melem/s - 68.489 Melem/s - 68.715 Melem/s

### `OneshotTokioHandoff/Ops-10000`
- **Time:** 118.20 µs - 118.80 µs - 119.31 µs
- **Throughput:** 83.815 Melem/s - 84.172 Melem/s - 84.605 Melem/s

## Wake latency

Ping-pong against a second OS thread, one channel each way per round; neither side can run ahead so the receiver parks on 9995 or more of 10000 receives. Throughput is round trips per second.

### `OneshotPingPong/Ops-1000`
- **Time:** 5.5220 ms - 5.7619 ms - 5.9087 ms
- **Throughput:** 169.24 Kelem/s - 173.56 Kelem/s - 181.09 Kelem/s

### `OneshotPingPong/Ops-10000`
- **Time:** 57.512 ms - 58.591 ms - 59.355 ms
- **Throughput:** 168.48 Kelem/s - 170.67 Kelem/s - 173.88 Kelem/s

### `OneshotExclusivePingPong/Ops-1000`
- **Time:** 5.5293 ms - 5.7898 ms - 5.9619 ms
- **Throughput:** 167.73 Kelem/s - 172.72 Kelem/s - 180.85 Kelem/s

### `OneshotExclusivePingPong/Ops-10000`
- **Time:** 58.155 ms - 58.998 ms - 59.766 ms
- **Throughput:** 167.32 Kelem/s - 169.50 Kelem/s - 171.95 Kelem/s

### `OneshotTokioPingPong/Ops-1000`
- **Time:** 5.7333 ms - 5.8641 ms - 6.0450 ms
- **Throughput:** 165.42 Kelem/s - 170.53 Kelem/s - 174.42 Kelem/s

### `OneshotTokioPingPong/Ops-10000`
- **Time:** 57.428 ms - 58.405 ms - 59.601 ms
- **Throughput:** 167.78 Kelem/s - 171.22 Kelem/s - 174.13 Kelem/s
