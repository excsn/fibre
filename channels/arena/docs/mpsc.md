# Channel Arena: MPSC
**Test Machine:** MacBook M4 Pro (14 cores)

many producers, one consumer

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

## sync

| Capacity | P×C | fibre | crossbeam | crossfire | flume | kanal | std |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 0.539 | 0.528 | - | 0.443 | **6.42** | 0.527 |
| rendezvous | 16x1 | 0.654 | 0.179 | - | 0.211 | **1.38** | 0.177 |
| rendezvous | 64x1 | **0.638** | 0.168 | - | 0.193 | 0.390 | 0.164 |
| 1 | 1x1 | 6.84 | 8.25 | **10.8** | 0.668 | 10.1 | 0.277 |
| 1 | 16x1 | 1.23 | 0.188 | 1.83 | 0.211 | **2.17** | 0.237 |
| 1 | 64x1 | **1.25** | 0.166 | 0.586 | 0.213 | 0.522 | 0.233 |
| 128 | 1x1 | **77.8** | 43.5 | 27.9 | 29.2 | 24.8 | 44.7 |
| 128 | 16x1 | **35.6** | 11.2 | 8.09 | 0.219 | 1.02 | 0.213 |
| 128 | 64x1 | **32.8** | 0.275 | 6.01 | 0.213 | 0.322 | 0.200 |
| 1024 | 1x1 | **151.9** | 65.5 | 27.8 | 7.10 | 49.3 | 144.0 |
| 1024 | 16x1 | **25.8** | 13.6 | 14.3 | 0.249 | 1.19 | 0.247 |
| 1024 | 64x1 | **25.9** | 0.969 | 11.7 | 0.232 | 0.423 | 0.224 |
| unbounded | 1x1 | **232.0** | 143.6 | 154.6 | 9.85 | 144.4 | 133.3 |
| unbounded | 16x1 | 13.1 | 23.0 | 17.0 | 38.6 | **45.6** | 7.93 |
| unbounded | 64x1 | 18.1 | 30.8 | 32.9 | **42.3** | 11.3 | 9.71 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre | flume |
| :--- | :--- | ---: | ---: |
| 128 | 1x1 | **27.5 (0.4x)** | 19.6 (0.7x) |
| 128 | 16x1 | **27.9 (0.8x)** | 3.44 (15.7x) |
| 128 | 64x1 | **28.3 (0.9x)** | 1.35 (6.4x) |
| 1024 | 1x1 | **251.8 (1.7x)** | 73.0 (10.3x) |
| 1024 | 16x1 | **235.7 (9.1x)** | 18.0 (72.2x) |
| 1024 | 64x1 | **241.2 (9.3x)** | 6.53 (28.2x) |
| unbounded | 1x1 | **332.0 (1.4x)** | 93.9 (9.5x) |
| unbounded | 16x1 | **230.9 (17.7x)** | 47.5 (1.2x) |
| unbounded | 64x1 | **210.7 (11.6x)** | 55.1 (1.3x) |

Native batch: fibre none, flume receive only.

## async

| Capacity | P×C | fibre | async-channel | crossfire | flume | kanal | tokio |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 22.9 | - | - | 7.45 | **23.0** | - |
| rendezvous | 16x1 | **7.51** | - | - | 0.991 | 4.57 | - |
| rendezvous | 64x1 | **4.46** | - | - | 0.608 | 2.65 | - |
| 1 | 1x1 | 8.90 | 3.83 | 3.81 | 7.36 | **29.1** | 8.37 |
| 1 | 16x1 | 8.21 | 1.76 | 0.563 | 0.929 | **20.9** | 7.86 |
| 1 | 64x1 | 7.04 | 1.79 | 0.471 | 0.605 | **10.8** | 6.88 |
| 128 | 1x1 | 63.3 | 24.0 | 69.8 | 38.3 | **78.9** | 33.2 |
| 128 | 16x1 | **63.4** | 3.66 | 11.4 | 4.02 | 9.13 | 6.07 |
| 128 | 64x1 | **65.7** | 1.34 | 7.31 | 1.05 | 8.67 | 2.49 |
| 1024 | 1x1 | **75.9** | 28.1 | 27.5 | 24.7 | 58.8 | 26.0 |
| 1024 | 16x1 | **76.2** | 8.07 | 13.0 | 5.95 | 2.45 | 6.12 |
| 1024 | 64x1 | **73.9** | 4.27 | 12.6 | 1.70 | 4.88 | 2.65 |
| unbounded | 1x1 | 108.6 | 30.0 | **109.2** | 60.5 | 105.2 | 63.4 |
| unbounded | 16x1 | 13.2 | 10.00 | **48.0** | 31.4 | 45.2 | 2.61 |
| unbounded | 64x1 | 12.6 | 9.42 | 17.0 | **32.2** | 22.5 | 2.45 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: crossfire at capacity 1 (2.7x, 2.19 to 5.90), crossfire at capacity 1024 (2.4x, 27.2 to 66.4). Those medians do not support a comparison.

## async, batched (512 items per call)

| Capacity | P×C | fibre | flume | tokio |
| :--- | :--- | ---: | ---: | ---: |
| 128 | 1x1 | **216.6 (3.4x)** | 49.2 (1.3x) | 44.1 (1.3x) |
| 128 | 16x1 | **212.9 (3.4x)** | 7.39 (1.8x) | 2.94 (0.5x) |
| 128 | 64x1 | **204.8 (3.1x)** | 4.66 (4.4x) | 2.38 (1.0x) |
| 1024 | 1x1 | **302.5 (4.0x)** | 58.1 (2.4x) | 43.3 (1.7x) |
| 1024 | 16x1 | **292.4 (3.8x)** | 18.5 (3.1x) | 2.26 (0.4x) |
| 1024 | 64x1 | **294.3 (4.0x)** | 16.1 (9.5x) | 2.42 (0.9x) |
| unbounded | 1x1 | **184.5 (1.7x)** | 93.0 (1.5x) | 110.9 (1.7x) |
| unbounded | 16x1 | **211.6 (16.1x)** | 41.2 (1.3x) | 2.76 (1.1x) |
| unbounded | 64x1 | **210.9 (16.7x)** | 43.6 (1.4x) | 2.09 (0.9x) |

Native batch: fibre none, flume receive only, tokio receive only.

## Implementations

| Library | Built with |
| :--- | :--- |
| fibre | `fibre::mpsc::bounded(n)`, `bounded_async(n)`, `unbounded()`, `unbounded_async()`, `rendezvous::rendezvous()`, `rendezvous::rendezvous_async()` |
| async-channel | `async_channel::bounded(n)`, `unbounded()` |
| crossbeam | `crossbeam_channel::bounded(n)`, `unbounded()`, `bounded(0)` |
| crossfire | `crossfire::mpsc::bounded_blocking(n)`, `bounded_async(n)`, `unbounded_blocking()`, `unbounded_async()` |
| flume | `flume::bounded(n)`, `unbounded()`, `bounded(0)` |
| kanal | `kanal::bounded(n)`, `bounded_async(n)`, `unbounded()`, `unbounded_async()`, `bounded(0)`, `bounded_async(0)` |
| std | `std::sync::mpsc::sync_channel(n)`, `channel()`, `sync_channel(0)` |
| tokio | `tokio::sync::mpsc::channel(n)`, `unbounded_channel()` |

Interpretation: [FINDINGS.md](./FINDINGS.md).
