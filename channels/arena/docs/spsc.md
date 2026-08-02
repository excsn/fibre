# Channel Arena: SPSC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer, one consumer

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

## sync

| Capacity | P×C | fibre | crossbeam | flume | kanal | std |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 0.561 | 0.516 | 0.451 | **7.16** | 0.509 |
| 1 | 1x1 | 0.263 | 7.90 | 0.675 | **8.57** | 0.281 |
| 128 | 1x1 | 30.0 | 46.9 | 30.1 | 25.9 | **53.1** |
| 1024 | 1x1 | 46.6 | 74.6 | 6.23 | 51.8 | **118.4** |
| unbounded | 1x1 | - | 144.1 | 6.59 | **145.2** | 138.5 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: kanal at capacity unbounded (2.0x, 90.4 to 181.6). Those medians do not support a comparison.

## sync, batched (512 items per call)

| Capacity | P×C | fibre | flume |
| :--- | :--- | ---: | ---: |
| 128 | 1x1 | **24.5 (0.8x)** | 19.9 (0.7x) |
| 1024 | 1x1 | **470.4 (10.1x)** | 77.1 (12.4x) |
| unbounded | 1x1 | - | 113.6 (17.2x) |

Native batch: fibre none, flume receive only.

## async

| Capacity | P×C | fibre | async-channel | flume | kanal | tokio |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | **24.3** | - | 7.58 | 20.6 | - |
| 1 | 1x1 | 11.6 | 3.57 | 7.67 | **26.4** | 7.77 |
| 128 | 1x1 | **109.1** | 25.1 | 40.5 | 83.8 | 33.6 |
| 1024 | 1x1 | **154.9** | 20.0 | 20.6 | 86.7 | 22.7 |
| unbounded | 1x1 | - | 30.1 | 51.9 | **105.8** | 64.1 |

## async, batched (512 items per call)

| Capacity | P×C | fibre | flume | tokio |
| :--- | :--- | ---: | ---: | ---: |
| 128 | 1x1 | **174.3 (1.6x)** | 56.0 (1.4x) | 45.7 (1.4x) |
| 1024 | 1x1 | **223.8 (1.4x)** | 58.1 (2.8x) | 43.2 (1.9x) |
| unbounded | 1x1 | - | 90.1 (1.7x) | **111.6 (1.7x)** |

Native batch: fibre none, flume receive only, tokio receive only.

Interpretation: [FINDINGS.md](./FINDINGS.md).
