# Channel Arena: SPMC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer broadcasting to many consumers (every consumer receives every item)

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

Only fibre appears here: no other library in the set has a comparable broadcast channel, and `tokio::sync::broadcast` drops items for lagging consumers rather than applying backpressure. Throughput is per item sent, so a 1×64 row does 64 times the receive work per unit shown.

## sync

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 0.232 |
| 1 | 1x16 | 0.032 |
| 1 | 1x64 | 0.009 |
| 128 | 1x1 | 18.6 |
| 128 | 1x16 | 0.050 |
| 128 | 1x64 | 0.011 |
| 1024 | 1x1 | 21.5 |
| 1024 | 1x16 | 0.043 |
| 1024 | 1x64 | 0.024 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: fibre at capacity 1 (3.5x, 0.067 to 0.237). Those medians do not support a comparison.

## sync, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 30.6 (1.6x) |
| 128 | 1x16 | 3.74 (74.8x) |
| 128 | 1x64 | 1.12 (102.2x) |
| 1024 | 1x1 | 98.2 (4.6x) |
| 1024 | 1x16 | 15.0 (349.8x) |
| 1024 | 1x64 | 4.64 (193.5x) |

Native batch: fibre send and receive.

## async

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 1 | 1x1 | 7.94 |
| 1 | 1x16 | 0.328 |
| 1 | 1x64 | 0.083 |
| 128 | 1x1 | 52.1 |
| 128 | 1x16 | 0.764 |
| 128 | 1x64 | 0.207 |
| 1024 | 1x1 | 18.3 |
| 1024 | 1x16 | 0.814 |
| 1024 | 1x64 | 0.213 |

## async, batched (512 items per call)

| Capacity | P×C | fibre |
| :--- | :--- | ---: |
| 128 | 1x1 | 124.8 (2.4x) |
| 128 | 1x16 | 22.0 (28.8x) |
| 128 | 1x64 | 8.97 (43.4x) |
| 1024 | 1x1 | 145.9 (8.0x) |
| 1024 | 1x16 | 52.3 (64.3x) |
| 1024 | 1x64 | 22.0 (103.4x) |

Native batch: fibre send and receive.

## Implementations

| Library | Built with |
| :--- | :--- |
| fibre | `fibre::spmc::bounded(n)`, `bounded_async(n)` |

Interpretation: [FINDINGS.md](./FINDINGS.md).
