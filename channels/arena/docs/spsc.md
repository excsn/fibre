# Channel Arena: SPSC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer, one consumer

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

## sync

| Capacity | P×C | fibre | crossbeam | flume | kanal | std |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 0.572 | 0.516 | 0.451 | **7.16** | 0.509 |
| 1 | 1x1 | 0.265 | 7.90 | 0.675 | **8.57** | 0.281 |
| 128 | 1x1 | 28.6 | 46.9 | 30.1 | 25.9 | **53.1** |
| 1024 | 1x1 | 44.4 | 74.6 | 6.23 | 51.8 | **118.4** |
| unbounded | 1x1 | - | 144.1 | 6.59 | **145.2** | 138.5 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: kanal at capacity unbounded (2.0x, 90.4 to 181.6). Those medians do not support a comparison.

## sync, batched (512 items per call)

| Capacity | P×C | fibre | flume |
| :--- | :--- | ---: | ---: |
| 128 | 1x1 | **25.2 (0.9x)** | 19.9 (0.7x) |
| 1024 | 1x1 | **447.2 (10.1x)** | 77.1 (12.4x) |
| unbounded | 1x1 | - | 113.6 (17.2x) |

Native batch: fibre none, flume receive only.

## async

| Capacity | P×C | fibre | async-channel | flume | kanal | tokio |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | **25.7** | - | 7.58 | 20.6 | - |
| 1 | 1x1 | 12.2 | 3.57 | 7.67 | **26.4** | 7.77 |
| 128 | 1x1 | **115.8** | 25.1 | 40.5 | 83.8 | 33.6 |
| 1024 | 1x1 | **152.7** | 20.0 | 20.6 | 86.7 | 22.7 |
| unbounded | 1x1 | - | 30.1 | 51.9 | **105.8** | 64.1 |

## async, batched (512 items per call)

| Capacity | P×C | fibre | flume | tokio |
| :--- | :--- | ---: | ---: | ---: |
| 128 | 1x1 | **172.7 (1.5x)** | 56.0 (1.4x) | 45.7 (1.4x) |
| 1024 | 1x1 | **207.5 (1.4x)** | 58.1 (2.8x) | 43.2 (1.9x) |
| unbounded | 1x1 | - | 90.1 (1.7x) | **111.6 (1.7x)** |

Native batch: fibre none, flume receive only, tokio receive only.

## Implementations

| Library | Built with |
| :--- | :--- |
| fibre | `fibre::spsc::bounded_sync(n)`, `bounded_async(n)`, `rendezvous::rendezvous()`, `rendezvous::rendezvous_async()` |
| async-channel | `async_channel::bounded(n)`, `unbounded()` |
| crossbeam | `crossbeam_channel::bounded(n)`, `unbounded()`, `bounded(0)` |
| flume | `flume::bounded(n)`, `unbounded()`, `bounded(0)` |
| kanal | `kanal::bounded(n)`, `bounded_async(n)`, `unbounded()`, `unbounded_async()`, `bounded(0)`, `bounded_async(0)` |
| std | `std::sync::mpsc::sync_channel(n)`, `channel()`, `sync_channel(0)` |
| tokio | `tokio::sync::mpsc::channel(n)`, `unbounded_channel()` |

Interpretation: [FINDINGS.md](./FINDINGS.md).
