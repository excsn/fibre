# Channel Arena: MPSC
**Test Machine:** MacBook M4 Pro (14 cores)

many producers, one consumer

Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported. Bracketed figures in batched tables are the gain over the single-item row above.

## sync

| Capacity | P×C | fibre | crossbeam | flume | kanal | std |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | 0.576 | 0.569 | 0.464 | **8.31** | 0.544 |
| rendezvous | 16x1 | 0.699 | 0.176 | 0.213 | **1.44** | 0.174 |
| rendezvous | 64x1 | **0.662** | 0.170 | 0.212 | 0.453 | 0.169 |
| 1 | 1x1 | 7.34 | 8.53 | 0.667 | **9.73** | 0.256 |
| 1 | 16x1 | 1.22 | 0.194 | 0.212 | **1.44** | 0.235 |
| 1 | 64x1 | **1.17** | 0.157 | 0.208 | 0.503 | 0.230 |
| 128 | 1x1 | 42.5 | 46.0 | 29.3 | 25.9 | **53.6** |
| 128 | 16x1 | 2.71 | **13.1** | 0.212 | 1.23 | 0.210 |
| 128 | 64x1 | 0.687 | **0.953** | 0.196 | 0.479 | 0.192 |
| 1024 | 1x1 | **176.0** | 62.4 | 7.14 | 42.6 | 117.4 |
| 1024 | 16x1 | 3.33 | **14.5** | 0.248 | 1.63 | 0.416 |
| 1024 | 64x1 | 0.863 | **21.0** | 0.233 | 0.490 | 0.216 |
| unbounded | 1x1 | **231.5** | 143.3 | 5.64 | 142.1 | 133.5 |
| unbounded | 16x1 | 13.0 | 17.6 | 42.1 | **43.5** | 7.68 |
| unbounded | 64x1 | 19.3 | 23.5 | **44.1** | 8.16 | 8.61 |

## sync, batched (512 items per call)

| Capacity | P×C | fibre | flume |
| :--- | :--- | ---: | ---: |
| 128 | 1x1 | **45.7 (1.1x)** | 20.3 (0.7x) |
| 128 | 16x1 | 3.26 (1.2x) | **3.42 (16.2x)** |
| 128 | 64x1 | 0.768 (1.1x) | **1.40 (7.1x)** |
| 1024 | 1x1 | **370.3 (2.1x)** | 74.4 (10.4x) |
| 1024 | 16x1 | 2.77 (0.8x) | **17.3 (69.7x)** |
| 1024 | 64x1 | 0.852 (1.0x) | **6.74 (29.0x)** |
| unbounded | 1x1 | **326.0 (1.4x)** | 103.0 (18.2x) |
| unbounded | 16x1 | **208.7 (16.1x)** | 52.0 (1.2x) |
| unbounded | 64x1 | **206.0 (10.7x)** | 61.9 (1.4x) |

Native batch: fibre none, flume receive only.

## async

| Capacity | P×C | fibre | async-channel | flume | kanal | tokio |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| rendezvous | 1x1 | **24.6** | - | 7.78 | 21.1 | - |
| rendezvous | 16x1 | **6.70** | - | 0.898 | 6.09 | - |
| rendezvous | 64x1 | **3.61** | - | 0.566 | 1.01 | - |
| 1 | 1x1 | 9.48 | 3.76 | 7.59 | **26.6** | 8.03 |
| 1 | 16x1 | 8.27 | 1.64 | 0.879 | **16.5** | 7.73 |
| 1 | 64x1 | 6.05 | 1.66 | 0.569 | **8.85** | 6.95 |
| 128 | 1x1 | 65.6 | 24.6 | 39.0 | **82.4** | 33.7 |
| 128 | 16x1 | **64.5** | 3.42 | 3.33 | 14.7 | 5.66 |
| 128 | 64x1 | **63.8** | 1.25 | 0.922 | 19.5 | 2.53 |
| 1024 | 1x1 | 75.4 | 20.5 | 21.6 | **81.7** | 22.7 |
| 1024 | 16x1 | **75.8** | 7.36 | 4.90 | 4.48 | 6.13 |
| 1024 | 64x1 | **74.3** | 4.05 | 1.34 | 9.95 | 2.46 |
| unbounded | 1x1 | **111.6** | 29.9 | 53.8 | 105.6 | 62.9 |
| unbounded | 16x1 | 13.0 | 10.2 | 25.0 | **44.5** | 2.20 |
| unbounded | 64x1 | 12.4 | 10.0 | **26.4** | 24.8 | 2.11 |

Unreliable, sample spread over 2x on a 1×1 cell where the rest of the matrix holds within a few percent: flume at capacity 1024 (2.0x, 16.3 to 32.8). Those medians do not support a comparison.

## async, batched (512 items per call)

| Capacity | P×C | fibre | flume | tokio |
| :--- | :--- | ---: | ---: | ---: |
| 128 | 1x1 | **227.0 (3.5x)** | 51.0 (1.3x) | 44.6 (1.3x) |
| 128 | 16x1 | **219.4 (3.4x)** | 7.14 (2.1x) | 2.85 (0.5x) |
| 128 | 64x1 | **197.6 (3.1x)** | 4.50 (4.9x) | 2.51 (1.0x) |
| 1024 | 1x1 | **295.5 (3.9x)** | 58.4 (2.7x) | 42.4 (1.9x) |
| 1024 | 16x1 | **268.8 (3.5x)** | 17.6 (3.6x) | 2.07 (0.3x) |
| 1024 | 64x1 | **273.5 (3.7x)** | 15.5 (11.5x) | 2.22 (0.9x) |
| unbounded | 1x1 | **197.9 (1.8x)** | 90.2 (1.7x) | 110.9 (1.8x) |
| unbounded | 16x1 | **196.2 (15.1x)** | 35.4 (1.4x) | 2.94 (1.3x) |
| unbounded | 64x1 | **197.2 (15.9x)** | 39.9 (1.5x) | 2.12 (1.0x) |

Native batch: fibre none, flume receive only, tokio receive only.

Interpretation: [FINDINGS.md](./FINDINGS.md).
