# Cache Arena: HOT
**Test Machine:** MacBook M4 Pro (14 cores)

every worker reads the same resident key.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold, leaving out quick_cache, which is a yardstick and is not ranked. Italics mean quick_cache is above the best ranked engine. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | **11.01** | 5.06 | 5.25 | *14.34* | 5.05 |
| async | 8 | **10.53** | 5.48 | 5.08 | 8.14 | 4.83 |

## Peak entries

The highest entry count the sampler saw during the run, against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 9520 | 9499 | 10000 | 9719 | 10000 |
| async | 8 | 9593 | 9521 | 10000 | 9711 | 10000 |

## Expected

Hit ratio exactly 1.0. Peak ≤ 10000.

Interpretation: [FINDINGS.md](./FINDINGS.md).
