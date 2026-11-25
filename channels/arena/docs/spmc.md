# Channel Arena: SPMC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer broadcasting to many consumers (every consumer receives every item)

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

Only fibre appears here: no other library in the set has a comparable broadcast channel, and `tokio::sync::broadcast` drops items for lagging consumers rather than applying backpressure. Throughput is per item sent, so a 1×64 row does 64 times the receive work per unit shown.

## sync

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 0.237 |
| 1 | 1x16 | 0.031 |
| 1 | 1x64 | 0.010 |
| 128 | 1x1 | 16.7 |
| 128 | 1x16 | 0.014 |
| 128 | 1x64 | 0.005 |
| 1024 | 1x1 | 18.5 |
| 1024 | 1x16 | 0.018 |
| 1024 | 1x64 | 0.007 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 32.2 (1.9x) |
| 128 | 1x16 | 3.73 (266.7x) |
| 128 | 1x64 | 1.22 (243.8x) |
| 1024 | 1x1 | 87.1 (4.7x) |
| 1024 | 1x16 | 14.5 (807.2x) |
| 1024 | 1x64 | 3.86 (551.7x) |

Native batch: fibre send and receive.

## async

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 8.21 |
| 1 | 1x16 | 0.382 |
| 1 | 1x64 | 0.086 |
| 128 | 1x1 | 42.8 |
| 128 | 1x16 | 0.736 |
| 128 | 1x64 | 0.201 |
| 1024 | 1x1 | 18.2 |
| 1024 | 1x16 | 0.741 |
| 1024 | 1x64 | 0.206 |

## async, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 127.2 (3.0x) |
| 128 | 1x16 | 24.4 (33.1x) |
| 128 | 1x64 | 9.71 (48.3x) |
| 1024 | 1x1 | 146.7 (8.1x) |
| 1024 | 1x16 | 60.4 (81.6x) |
| 1024 | 1x64 | 23.0 (111.7x) |

Native batch: fibre send and receive.

## Implementations

| Library | Built with |
| :--- | :--- |
| fibre | `fibre::spmc::bounded(n)`, `bounded_async(n)` |

Interpretation: [FINDINGS.md](./FINDINGS.md).
