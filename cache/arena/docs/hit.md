# Cache Arena: HIT
**Test Machine:** MacBook M4 Pro (14 cores)

every operation reads a resident key.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold, leaving out quick_cache, which is a yardstick and is not ranked. Italics mean quick_cache is above the best ranked engine. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | **44.40** | 23.64 | 12.39 | *166.44* | 23.77 |
| sync | 8 | **108.22** | 37.55 | 6.42 | 94.76 | 4.27 |
| async | 64 | **224.77** | 47.58 | 28.12 | 104.90 | 4.76 |

## Hit ratio and peak entries

Hit ratio over the run, then the highest entry count the sampler saw against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | 0.953 / 9531 | 0.955 / 9549 | 1.000 / 10000 | 0.963 / 9635 | 1.000 / 10000 |
| sync | 8 | 0.956 / 9565 | 0.956 / 9559 | 1.000 / 10000 | 0.963 / 9631 | 1.000 / 10000 |
| async | 64 | 0.954 / 9539 | 0.949 / 9494 | 1.000 / 10000 | 0.975 / 9748 | 1.000 / 10000 |

## Expected

Hit ratio 1.0, or 0.95 and up for a cache with per-shard budgets. Peak ≤ 10000.

Interpretation: [FINDINGS.md](./FINDINGS.md).
