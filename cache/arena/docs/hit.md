# Cache Arena: HIT
**Test Machine:** MacBook M4 Pro (14 cores)

every operation reads a resident key.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | 47.75 | 24.28 | 12.87 | **168.34** | 24.52 |
| sync | 8 | **108.10** | 38.67 | 6.08 | 81.95 | 4.09 |
| async | 64 | **217.80** | 48.04 | 17.86 | 96.60 | 4.97 |

## Hit ratio and peak entries

Hit ratio over the run, then the highest entry count the sampler saw against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | 0.960 / 9596 | 0.955 / 9552 | 1.000 / 10000 | 0.967 / 9670 | 1.000 / 10000 |
| sync | 8 | 0.956 / 9561 | 0.964 / 9637 | 1.000 / 10000 | 0.967 / 9668 | 1.000 / 10000 |
| async | 64 | 0.956 / 9567 | 0.951 / 9507 | 1.000 / 10000 | 0.973 / 9727 | 1.000 / 10000 |

## Expected

Hit ratio 1.0, or 0.95 and up for a cache with per-shard budgets. Peak ≤ 10000.

Interpretation: [FINDINGS.md](./FINDINGS.md).
