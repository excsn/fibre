# Channel Arena: SPMC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer broadcasting to many consumers (every consumer receives every item)

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

Only fibre appears here: no other library in the set has a comparable broadcast channel, and `tokio::sync::broadcast` drops items for lagging consumers rather than applying backpressure. Throughput is per item sent, so a 1×64 row does 64 times the receive work per unit shown.

## sync

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 0.246 |
| 1 | 1x16 | 0.032 |
| 1 | 1x64 | 0.010 |
| 128 | 1x1 | 14.9 |
| 128 | 1x16 | 0.016 |
| 128 | 1x64 | 0.005 |
| 1024 | 1x1 | 17.7 |
| 1024 | 1x16 | 0.029 |
| 1024 | 1x64 | 0.007 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 30.4 (2.0x) |
| 128 | 1x16 | 3.77 (238.3x) |
| 128 | 1x64 | 1.02 (216.4x) |
| 1024 | 1x1 | 81.6 (4.6x) |
| 1024 | 1x16 | 13.6 (467.4x) |
| 1024 | 1x64 | 4.10 (589.4x) |

Native batch: fibre send and receive.

## async

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 7.62 |
| 1 | 1x16 | 0.354 |
| 1 | 1x64 | 0.078 |
| 128 | 1x1 | 43.1 |
| 128 | 1x16 | 0.755 |
| 128 | 1x64 | 0.204 |
| 1024 | 1x1 | 20.4 |
| 1024 | 1x16 | 0.759 |
| 1024 | 1x64 | 0.212 |

## async, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 112.8 (2.6x) |
| 128 | 1x16 | 24.0 (31.7x) |
| 128 | 1x64 | 9.93 (48.7x) |
| 1024 | 1x1 | 142.9 (7.0x) |
| 1024 | 1x16 | 58.2 (76.7x) |
| 1024 | 1x64 | 23.6 (111.1x) |

Native batch: fibre send and receive.

Interpretation: [FINDINGS.md](./FINDINGS.md).
