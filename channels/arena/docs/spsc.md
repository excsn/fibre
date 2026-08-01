# Channel Arena: SPSC
**Test Machine:** MacBook M4 Pro (14 cores)

one producer, one consumer

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

## sync

| Capacity | P×C | fibre | crossbeam | flume | kanal | std |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 0.527 | 0.460 | 0.404 | **4.34** | 0.486 |
| 1 | 1x1 | 0.226 | 5.14 | 0.583 | **5.20** | 0.222 |
| 128 | 1x1 | 19.5 | **30.3** | 20.5 | 14.0 | 23.8 |
| 1024 | 1x1 | 23.4 | 38.5 | 6.54 | 27.0 | **116.5** |
| unbounded | 1x1 | - | 77.5 | 5.41 | **95.7** | 84.9 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre | flume |
| :--- | :--- | ---: | ---: |
| 128 | 1x1 | **19.4 (1.0x)** | 13.9 (0.7x) |
| 1024 | 1x1 | **285.5 (12.2x)** | 48.7 (7.4x) |
| unbounded | 1x1 | - | 74.0 (13.7x) |

Native batch: fibre none, flume receive only.

## async

| Capacity | P×C | fibre | async-channel | flume | kanal | tokio |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | **15.0** | - | 4.44 | 12.9 | - |
| 1 | 1x1 | 7.29 | 2.40 | 4.36 | **17.1** | 5.18 |
| 128 | 1x1 | **71.5** | 15.5 | 28.6 | 52.1 | 20.1 |
| 1024 | 1x1 | **102.4** | 12.7 | 9.72 | 51.4 | 13.2 |
| unbounded | 1x1 | - | 18.6 | 39.4 | **68.9** | 41.2 |

## async, batched (512 items per call)

| Capacity | P×C | fibre | flume | tokio |
| :--- | :--- | ---: | ---: | ---: |
| 128 | 1x1 | **112.8 (1.6x)** | 24.4 (0.9x) | 29.2 (1.5x) |
| 1024 | 1x1 | **139.7 (1.4x)** | 37.7 (3.9x) | 28.3 (2.1x) |
| unbounded | 1x1 | - | 62.7 (1.6x) | **72.3 (1.8x)** |

Native batch: fibre none, flume receive only, tokio receive only.

Interpretation: [FINDINGS.md](./FINDINGS.md).
