# Cache Arena: ZIPF
**Test Machine:** MacBook M4 Pro (14 cores)

Zipf-distributed reads over ten times the capacity, inserting on a miss.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 32.36 | 23.62 | 5.58 | **45.77** | 3.35 |
| sync | 64 | **32.47** | 23.61 | 1.13 | 25.05 | 4.44 |
| async | 8 | 25.85 | 18.84 | 5.67 | **46.95** | 3.75 |
| async | 64 | **33.39** | 25.74 | 4.24 | 26.34 | 3.98 |
| mixed | 64 | 25.71 | 18.46 | - | **27.27** | - |

## Hit ratio and peak entries

Hit ratio over the run, then the highest entry count the sampler saw against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 0.777 / 9984 | 0.774 / 9984 | 0.788 / 10000 | 0.785 / 10048 | 0.590 / 10002 |
| sync | 64 | 0.777 / 9984 | 0.774 / 9984 | 0.785 / 10000 | 0.785 / 10048 | 0.638 / 10002 |
| async | 8 | 0.777 / 9984 | 0.774 / 9984 | 0.787 / 10000 | 0.785 / 10048 | 0.644 / 10002 |
| async | 64 | 0.777 / 9984 | 0.774 / 9984 | 0.788 / 10000 | 0.785 / 10048 | 0.648 / 10002 |
| mixed | 64 | 0.777 / 9984 | 0.774 / 9984 | - | 0.785 / 10048 | - |

## Expected

| | Hit ratio | Peak |
| :--- | :--- | :--- |
| Ceiling | 0.800 (the 10000 most frequent keys always resident) | 10000 |
| Expected | 0.77 to 0.800 | ≤ 10000 |
| Abnormal | above 0.800 or below 0.77 | above 10000 |

Interpretation: [FINDINGS.md](./FINDINGS.md).
