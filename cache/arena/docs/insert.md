# Cache Arena: INSERT
**Test Machine:** MacBook M4 Pro (14 cores)

every operation inserts a key not yet seen, into a full cache.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | 13.99 | 7.88 | 2.86 | **43.59** | 8.17 |
| sync | 64 | 34.74 | 22.43 | 0.32 | **50.05** | 4.85 |
| async | 64 | 38.99 | 24.96 | 1.38 | **53.74** | 3.11 |

## Peak entries

The highest entry count the sampler saw during the run, against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 1 | 9984 | 9984 | 10000 | 10048 | 10002 |
| sync | 64 | 9984 | 9984 | 10000 | 10048 | 10002 |
| async | 64 | 9984 | 9984 | 10000 | 10048 | 10002 |

## Expected

No gets, so no hit ratio. Peak ≤ 10000.

Interpretation: [FINDINGS.md](./FINDINGS.md).
