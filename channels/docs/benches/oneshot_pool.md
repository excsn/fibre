# Fibre Benchmark: Oneshot Pool
**Test Machine:** MacBook M4 Pro

Every group runs both pools as a matrix axis: `Pool-Pair` is `oneshot::pair_pool()` and `Pool-Host` is `oneshot::OneshotHostPool` with the same workload over record-shaped cells.

## Full cycle

Channel created, sent and received inside the timed loop; a pooled creation is a freelist pop and its retirement a push, with no allocation.

### `OneshotPoolAsync/Pool-Pair_Ops-100`
- **Time:** 703.73 ns - 705.46 ns - 707.61 ns
- **Throughput:** 141.32 Melem/s - 141.75 Melem/s - 142.10 Melem/s

### `OneshotPoolAsync/Pool-Pair_Ops-1000`
- **Time:** 6.8707 µs - 6.8858 µs - 6.8967 µs
- **Throughput:** 145.00 Melem/s - 145.23 Melem/s - 145.54 Melem/s

### `OneshotPoolAsync/Pool-Host_Ops-100`
- **Time:** 700.75 ns - 701.96 ns - 703.28 ns
- **Throughput:** 142.19 Melem/s - 142.46 Melem/s - 142.70 Melem/s

### `OneshotPoolAsync/Pool-Host_Ops-1000`
- **Time:** 6.8813 µs - 6.9020 µs - 6.9138 µs
- **Throughput:** 144.64 Melem/s - 144.88 Melem/s - 145.32 Melem/s

## Transfer only

Channels pre-created outside the timed loop; send, recv and the retirement push are measured.

### `OneshotPoolAsyncXfer/Pool-Pair_Ops-100`
- **Time:** 521.66 ns - 523.31 ns - 526.38 ns
- **Throughput:** 189.98 Melem/s - 191.09 Melem/s - 191.70 Melem/s

### `OneshotPoolAsyncXfer/Pool-Pair_Ops-1000`
- **Time:** 5.0064 µs - 5.0178 µs - 5.0362 µs
- **Throughput:** 198.56 Melem/s - 199.29 Melem/s - 199.75 Melem/s

### `OneshotPoolAsyncXfer/Pool-Host_Ops-100`
- **Time:** 543.90 ns - 547.83 ns - 549.83 ns
- **Throughput:** 181.87 Melem/s - 182.54 Melem/s - 183.86 Melem/s

### `OneshotPoolAsyncXfer/Pool-Host_Ops-1000`
- **Time:** 5.1933 µs - 5.2143 µs - 5.2288 µs
- **Throughput:** 191.25 Melem/s - 191.78 Melem/s - 192.56 Melem/s

## Batch creation

`pair_batch(n)` / `pair_init_batch(n, ..)` create all n channels in one freelist operation inside the timed loop, then exchange them.

### `OneshotPoolBatchAsync/Pool-Pair_Ops-100`
- **Time:** 780.56 ns - 785.60 ns - 792.95 ns
- **Throughput:** 126.11 Melem/s - 127.29 Melem/s - 128.11 Melem/s

### `OneshotPoolBatchAsync/Pool-Pair_Ops-1000`
- **Time:** 7.3353 µs - 7.3597 µs - 7.4035 µs
- **Throughput:** 135.07 Melem/s - 135.88 Melem/s - 136.33 Melem/s

### `OneshotPoolBatchAsync/Pool-Host_Ops-100`
- **Time:** 818.52 ns - 823.30 ns - 828.18 ns
- **Throughput:** 120.75 Melem/s - 121.46 Melem/s - 122.17 Melem/s

### `OneshotPoolBatchAsync/Pool-Host_Ops-1000`
- **Time:** 7.7952 µs - 7.8192 µs - 7.8513 µs
- **Throughput:** 127.37 Melem/s - 127.89 Melem/s - 128.28 Melem/s

## Inside a request record

Each op carries a request record. The pair pool allocates the record per request alongside its pooled channel; the host pool's record is the pool cell itself with the reply slot embedded, so its arm performs no allocation at all.

### `OneshotPoolRecordAsync/Pool-Pair_Ops-100`
- **Time:** 2.4555 µs - 2.4923 µs - 2.5257 µs
- **Throughput:** 39.593 Melem/s - 40.124 Melem/s - 40.725 Melem/s

### `OneshotPoolRecordAsync/Pool-Pair_Ops-1000`
- **Time:** 23.539 µs - 23.657 µs - 23.781 µs
- **Throughput:** 42.050 Melem/s - 42.271 Melem/s - 42.483 Melem/s

### `OneshotPoolRecordAsync/Pool-Host_Ops-100`
- **Time:** 1.1569 µs - 1.1587 µs - 1.1609 µs
- **Throughput:** 86.140 Melem/s - 86.306 Melem/s - 86.440 Melem/s

### `OneshotPoolRecordAsync/Pool-Host_Ops-1000`
- **Time:** 11.592 µs - 11.630 µs - 11.669 µs
- **Throughput:** 85.695 Melem/s - 85.988 Melem/s - 86.266 Melem/s

## Cross-thread

Channels pre-created, a second OS thread sends them all, the receiver drains them; the receiver parks on 0 of 10000 receives for both pools.

### `OneshotPoolHandoff/Pool-Pair_Ops-1000`
- **Time:** 6.1077 µs - 6.1402 µs - 6.1605 µs
- **Throughput:** 162.32 Melem/s - 162.86 Melem/s - 163.73 Melem/s

### `OneshotPoolHandoff/Pool-Pair_Ops-10000`
- **Time:** 52.674 µs - 53.622 µs - 54.259 µs
- **Throughput:** 184.30 Melem/s - 186.49 Melem/s - 189.85 Melem/s

### `OneshotPoolHandoff/Pool-Host_Ops-1000`
- **Time:** 6.1688 µs - 6.2675 µs - 6.3731 µs
- **Throughput:** 156.91 Melem/s - 159.55 Melem/s - 162.10 Melem/s

### `OneshotPoolHandoff/Pool-Host_Ops-10000`
- **Time:** 52.521 µs - 52.824 µs - 53.003 µs
- **Throughput:** 188.67 Melem/s - 189.31 Melem/s - 190.40 Melem/s

## Wake latency

Ping-pong between a task and a peer thread, one channel each way per round, so both sides park every round; the receiver parks on 9995 (pair) and 9998 (host) of 10000 receives. Throughput is round trips per second.

### `OneshotPoolPingPong/Pool-Pair_Ops-1000`
- **Time:** 5.4211 ms - 5.5753 ms - 5.6503 ms
- **Throughput:** 176.98 Kelem/s - 179.36 Kelem/s - 184.46 Kelem/s

### `OneshotPoolPingPong/Pool-Pair_Ops-10000`
- **Time:** 55.034 ms - 55.716 ms - 56.361 ms
- **Throughput:** 177.43 Kelem/s - 179.48 Kelem/s - 181.71 Kelem/s

### `OneshotPoolPingPong/Pool-Host_Ops-1000`
- **Time:** 5.4337 ms - 5.5045 ms - 5.5629 ms
- **Throughput:** 179.76 Kelem/s - 181.67 Kelem/s - 184.04 Kelem/s

### `OneshotPoolPingPong/Pool-Host_Ops-10000`
- **Time:** 53.929 ms - 54.979 ms - 55.826 ms
- **Throughput:** 179.13 Kelem/s - 181.89 Kelem/s - 185.43 Kelem/s
