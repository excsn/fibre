# Fibre Benchmark: Oneshot
**Test Machine:** MacBook M4 Pro

Every group runs the clonable `oneshot()`, the single-sender `oneshot::exclusive()`, and `tokio::sync::oneshot` as reference.

## Full cycle

Channel created, sent and received inside the timed loop, so the per-channel allocation is included.

`{Group}/Ops-{Ops}`

| Group | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| OneshotAsync | 100 | 2.4812 µs / 2.5527 µs / 2.6236 µs | 38.116 Melem/s / 39.174 Melem/s / 40.302 Melem/s |
| OneshotAsync | 1000 | 24.815 µs / 25.054 µs / 25.167 µs | 39.735 Melem/s / 39.914 Melem/s / 40.299 Melem/s |
| OneshotExclusiveAsync | 100 | 1.9155 µs / 1.9374 µs / 1.9741 µs | 50.656 Melem/s / 51.615 Melem/s / 52.207 Melem/s |
| OneshotExclusiveAsync | 1000 | 19.005 µs / 19.208 µs / 19.432 µs | 51.463 Melem/s / 52.063 Melem/s / 52.617 Melem/s |
| OneshotTokioAsync | 100 | 2.4402 µs / 2.4726 µs / 2.5080 µs | 39.873 Melem/s / 40.444 Melem/s / 40.981 Melem/s |
| OneshotTokioAsync | 1000 | 24.013 µs / 24.235 µs / 24.434 µs | 40.927 Melem/s / 41.263 Melem/s / 41.644 Melem/s |

## Transfer only

Channels pre-created outside the timed loop, so only send and recv are measured.

`{Group}/Ops-{Ops}`

| Group | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| OneshotAsyncXfer | 100 | 1.4853 µs / 1.4915 µs / 1.4983 µs | 66.743 Melem/s / 67.047 Melem/s / 67.328 Melem/s |
| OneshotAsyncXfer | 1000 | 14.539 µs / 14.583 µs / 14.635 µs | 68.328 Melem/s / 68.574 Melem/s / 68.783 Melem/s |
| OneshotExclusiveAsyncXfer | 100 | 1.0631 µs / 1.0672 µs / 1.0711 µs | 93.364 Melem/s / 93.707 Melem/s / 94.065 Melem/s |
| OneshotExclusiveAsyncXfer | 1000 | 10.258 µs / 10.453 µs / 10.703 µs | 93.436 Melem/s / 95.668 Melem/s / 97.481 Melem/s |
| OneshotTokioAsyncXfer | 100 | 1.4398 µs / 1.4590 µs / 1.4797 µs | 67.583 Melem/s / 68.538 Melem/s / 69.453 Melem/s |
| OneshotTokioAsyncXfer | 1000 | 14.282 µs / 14.422 µs / 14.564 µs | 68.661 Melem/s / 69.339 Melem/s / 70.020 Melem/s |

## Inside a request record

An `Arc<Req>` per op alongside the channel, modelling a oneshot carried by an in-flight request: one allocation for the record plus one for the channel.

`{Group}/Ops-{Ops}`

| Group | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| OneshotRecordAsync | 100 | 4.3279 µs / 4.3703 µs / 4.4170 µs | 22.640 Melem/s / 22.882 Melem/s / 23.106 Melem/s |
| OneshotRecordAsync | 1000 | 42.858 µs / 43.888 µs / 44.755 µs | 22.344 Melem/s / 22.785 Melem/s / 23.333 Melem/s |
| OneshotExclusiveRecordAsync | 100 | 3.7933 µs / 3.8096 µs / 3.8305 µs | 26.106 Melem/s / 26.249 Melem/s / 26.362 Melem/s |
| OneshotExclusiveRecordAsync | 1000 | 37.829 µs / 38.152 µs / 38.543 µs | 25.945 Melem/s / 26.211 Melem/s / 26.434 Melem/s |
| OneshotTokioRecordAsync | 100 | 4.1814 µs / 4.2371 µs / 4.2796 µs | 23.367 Melem/s / 23.601 Melem/s / 23.915 Melem/s |
| OneshotTokioRecordAsync | 1000 | 41.678 µs / 41.823 µs / 42.042 µs | 23.786 Melem/s / 23.911 Melem/s / 23.993 Melem/s |

## Cross-thread

Channels pre-created, then a second OS thread sends all of them while the receiver awaits; the sender wins the race outright, so the receiver parks on 0 of 10000 receives (78 of 10000 for tokio).

`{Group}/Ops-{Ops}`

| Group | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| OneshotHandoff | 1000 | 13.460 µs / 13.524 µs / 13.556 µs | 73.770 Melem/s / 73.940 Melem/s / 74.296 Melem/s |
| OneshotHandoff | 10000 | 109.70 µs / 110.53 µs / 112.10 µs | 89.206 Melem/s / 90.475 Melem/s / 91.155 Melem/s |
| OneshotExclusiveHandoff | 1000 | 11.167 µs / 11.292 µs / 11.395 µs | 87.760 Melem/s / 88.559 Melem/s / 89.553 Melem/s |
| OneshotExclusiveHandoff | 10000 | 92.130 µs / 92.403 µs / 92.749 µs | 107.82 Melem/s / 108.22 Melem/s / 108.54 Melem/s |
| OneshotTokioHandoff | 1000 | 14.455 µs / 14.585 µs / 14.731 µs | 67.884 Melem/s / 68.564 Melem/s / 69.182 Melem/s |
| OneshotTokioHandoff | 10000 | 125.73 µs / 126.58 µs / 127.39 µs | 78.496 Melem/s / 79.000 Melem/s / 79.537 Melem/s |

## Wake latency

Ping-pong against a second OS thread, one channel each way per round; neither side can run ahead so the receiver parks on 9995 or more of 10000 receives. Throughput is round trips per second.

`{Group}/Ops-{Ops}`

| Group | Ops | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---:|---:|
| OneshotPingPong | 1000 | 5.7552 ms / 5.8886 ms / 6.0417 ms | 165.52 Kelem/s / 169.82 Kelem/s / 173.76 Kelem/s |
| OneshotPingPong | 10000 | 57.644 ms / 58.769 ms / 59.870 ms | 167.03 Kelem/s / 170.16 Kelem/s / 173.48 Kelem/s |
| OneshotExclusivePingPong | 1000 | 5.5610 ms / 5.7823 ms / 5.8921 ms | 169.72 Kelem/s / 172.94 Kelem/s / 179.82 Kelem/s |
| OneshotExclusivePingPong | 10000 | 56.731 ms / 57.700 ms / 59.021 ms | 169.43 Kelem/s / 173.31 Kelem/s / 176.27 Kelem/s |
| OneshotTokioPingPong | 1000 | 6.0263 ms / 6.1202 ms / 6.2185 ms | 160.81 Kelem/s / 163.39 Kelem/s / 165.94 Kelem/s |
| OneshotTokioPingPong | 10000 | 59.797 ms / 60.166 ms / 60.973 ms | 164.01 Kelem/s / 166.21 Kelem/s / 167.23 Kelem/s |
