# Channel Arena: SPMC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer broadcasting to many consumers (every consumer receives every item)

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

Only fibre appears here: no other library in the set has a comparable broadcast channel, and `tokio::sync::broadcast` drops items for lagging consumers rather than applying backpressure. Throughput is per item sent, so a 1×64 row does 64 times the receive work per unit shown.

## sync

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 0.229 |
| 1 | 1x16 | 0.024 |
| 1 | 1x64 | 0.007 |
| 128 | 1x1 | 10.6 |
| 128 | 1x16 | 0.012 |
| 128 | 1x64 | 0.004 |
| 1024 | 1x1 | 10.4 |
| 1024 | 1x16 | 0.073 |
| 1024 | 1x64 | 0.005 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 20.9 (2.0x) |
| 128 | 1x16 | 2.93 (244.2x) |
| 128 | 1x64 | 0.808 (202.0x) |
| 1024 | 1x1 | 59.5 (5.7x) |
| 1024 | 1x16 | 10.3 (140.8x) |
| 1024 | 1x64 | 3.60 (720.0x) |

Native batch: fibre send and receive.

## async

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 5.81 |
| 1 | 1x16 | 0.247 |
| 1 | 1x64 | 0.061 |
| 128 | 1x1 | 24.2 |
| 128 | 1x16 | 0.594 |
| 128 | 1x64 | 0.165 |
| 1024 | 1x1 | 11.4 |
| 1024 | 1x16 | 0.612 |
| 1024 | 1x64 | 0.173 |

## async, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 72.8 (3.0x) |
| 128 | 1x16 | 15.7 (26.4x) |
| 128 | 1x64 | 6.80 (41.2x) |
| 1024 | 1x1 | 95.6 (8.4x) |
| 1024 | 1x16 | 37.3 (61.0x) |
| 1024 | 1x64 | 15.3 (88.7x) |

Native batch: fibre send and receive.

Interpretation: [FINDINGS.md](./FINDINGS.md).
