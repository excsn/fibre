# Fibre Benchmark: Oneshot Pool
**Test Machine:** MacBook M4 Pro

Every group runs both pools as a matrix axis: `Pool-Pair` is `oneshot::pair_pool()` and `Pool-Host` is `oneshot::OneshotHostPool` with the same workload over record-shaped cells.

## Full cycle

Channel created, sent and received inside the timed loop; a pooled creation is a freelist pop and its retirement a push, with no allocation.

`OneshotPoolAsync/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 100 | 723.54 ns / 725.03 ns / 729.59 ns | 137.06 Melem/s / 137.92 Melem/s / 138.21 Melem/s |
| Pair | 1000 | 7.1112 µs / 7.1484 µs / 7.2589 µs | 137.76 Melem/s / 139.89 Melem/s / 140.62 Melem/s |
| Host | 100 | 704.98 ns / 711.88 ns / 717.00 ns | 139.47 Melem/s / 140.47 Melem/s / 141.85 Melem/s |
| Host | 1000 | 6.8840 µs / 6.9543 µs / 7.0323 µs | 142.20 Melem/s / 143.80 Melem/s / 145.26 Melem/s |

## Transfer only

Channels pre-created outside the timed loop; send, recv and the retirement push are measured.

`OneshotPoolAsyncXfer/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 100 | 491.12 ns / 495.27 ns / 501.05 ns | 199.58 Melem/s / 201.91 Melem/s / 203.62 Melem/s |
| Pair | 1000 | 4.7627 µs / 4.8257 µs / 4.9226 µs | 203.14 Melem/s / 207.22 Melem/s / 209.96 Melem/s |
| Host | 100 | 509.58 ns / 549.48 ns / 602.51 ns | 165.97 Melem/s / 181.99 Melem/s / 196.24 Melem/s |
| Host | 1000 | 5.0611 µs / 5.6234 µs / 5.9716 µs | 167.46 Melem/s / 177.83 Melem/s / 197.58 Melem/s |

## Batch creation

`pair_batch(n)` / `pair_init_batch(n, ..)` create all n channels in one freelist operation inside the timed loop, then exchange them.

`OneshotPoolBatchAsync/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 100 | 666.13 ns / 673.48 ns / 680.67 ns | 146.91 Melem/s / 148.48 Melem/s / 150.12 Melem/s |
| Pair | 1000 | 6.6549 µs / 6.7179 µs / 6.7520 µs | 148.10 Melem/s / 148.86 Melem/s / 150.27 Melem/s |
| Host | 100 | 752.92 ns / 754.51 ns / 756.74 ns | 132.15 Melem/s / 132.54 Melem/s / 132.82 Melem/s |
| Host | 1000 | 7.2379 µs / 7.2685 µs / 7.3064 µs | 136.87 Melem/s / 137.58 Melem/s / 138.16 Melem/s |

## Inside a request record

Each op carries a request record. The pair pool allocates the record per request alongside its pooled channel; the host pool's record is the pool cell itself with the reply slot embedded, so its arm performs no allocation at all.

`OneshotPoolRecordAsync/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 100 | 2.3435 µs / 2.3844 µs / 2.4243 µs | 41.249 Melem/s / 41.940 Melem/s / 42.671 Melem/s |
| Pair | 1000 | 23.565 µs / 23.807 µs / 24.001 µs | 41.664 Melem/s / 42.004 Melem/s / 42.436 Melem/s |
| Host | 100 | 922.47 ns / 923.68 ns / 924.69 ns | 108.14 Melem/s / 108.26 Melem/s / 108.40 Melem/s |
| Host | 1000 | 9.1471 µs / 9.1595 µs / 9.1770 µs | 108.97 Melem/s / 109.18 Melem/s / 109.32 Melem/s |

## Cross-thread

Channels pre-created, a second OS thread sends them all, the receiver drains them; the receiver parks on 0 of 10000 receives for both pools.

`OneshotPoolHandoff/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 1000 | 5.9409 µs / 5.9968 µs / 6.0232 µs | 166.02 Melem/s / 166.76 Melem/s / 168.32 Melem/s |
| Pair | 10000 | 50.359 µs / 50.536 µs / 50.732 µs | 197.12 Melem/s / 197.88 Melem/s / 198.57 Melem/s |
| Host | 1000 | 6.0091 µs / 6.0257 µs / 6.0340 µs | 165.73 Melem/s / 165.96 Melem/s / 166.41 Melem/s |
| Host | 10000 | 51.450 µs / 51.533 µs / 51.613 µs | 193.75 Melem/s / 194.05 Melem/s / 194.36 Melem/s |

## Wake latency

Ping-pong between a task and a peer thread, one channel each way per round, so both sides park every round; the receiver parks on 9995 (pair) and 9998 (host) of 10000 receives. Throughput is round trips per second.

`OneshotPoolPingPong/Pool-{Pool}_Ops-{Ops}`

| Pool | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| Pair | 1000 | 5.1859 ms / 5.2922 ms / 5.5574 ms | 179.94 Kelem/s / 188.96 Kelem/s / 192.83 Kelem/s |
| Pair | 10000 | 51.992 ms / 53.225 ms / 54.731 ms | 182.71 Kelem/s / 187.88 Kelem/s / 192.34 Kelem/s |
| Host | 1000 | 5.1495 ms / 5.2437 ms / 5.5085 ms | 181.54 Kelem/s / 190.70 Kelem/s / 194.19 Kelem/s |
| Host | 10000 | 51.460 ms / 52.204 ms / 53.989 ms | 185.22 Kelem/s / 191.56 Kelem/s / 194.32 Kelem/s |
