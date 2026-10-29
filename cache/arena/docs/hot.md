# Cache Arena: HOT
**Test Machine:** MacBook M4 Pro (14 cores)

every worker reads the same resident key.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 11.23 | 5.10 | 5.19 | **17.65** | 4.95 |
| async | 8 | **10.71** | 5.27 | 4.88 | 7.81 | 4.84 |

## Peak entries

The highest entry count the sampler saw during the run, against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 9558 | 9515 | 10000 | 9772 | 10000 |
| async | 8 | 9515 | 9541 | 10000 | 9723 | 10000 |

## Expected

Hit ratio exactly 1.0. Peak ≤ 10000.

Interpretation: [FINDINGS.md](./FINDINGS.md).
