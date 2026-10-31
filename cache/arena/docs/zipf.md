# Cache Arena: ZIPF
**Test Machine:** MacBook M4 Pro (14 cores)

Zipf-distributed reads over ten times the capacity, inserting on a miss.

Mops/s: operations completed per second across all workers over a 10 s run at capacity 10000. Best per row in bold, leaving out quick_cache, which is a yardstick and is not ranked. Italics mean quick_cache is above the best ranked engine. `-` means the engine has no API for that mode.

## Throughput

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | **30.15** | 23.57 | 5.57 | *42.44* | 3.66 |
| sync | 64 | **31.34** | 23.91 | 1.11 | 21.02 | 3.59 |
| async | 8 | **30.72** | 20.88 | 5.39 | *47.01* | 3.86 |
| async | 64 | **31.28** | 25.87 | 4.01 | 24.85 | 3.75 |
| mixed | 64 | **26.20** | 19.39 | - | 21.30 | - |

## Hit ratio and peak entries

Hit ratio over the run, then the highest entry count the sampler saw against the capacity.

| Mode | Workers | fibre_cache.s3fifo | fibre_cache.tinylfu | moka | quick_cache | stretto |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| sync | 8 | 0.777 / 9984 | 0.774 / 9984 | 0.788 / 10000 | 0.785 / 10048 | 0.652 / 10002 |
| sync | 64 | 0.778 / 9984 | 0.774 / 9984 | 0.785 / 10000 | 0.785 / 10048 | 0.615 / 10003 |
| async | 8 | 0.777 / 9984 | 0.774 / 9984 | 0.787 / 10000 | 0.785 / 10048 | 0.668 / 10002 |
| async | 64 | 0.777 / 9984 | 0.774 / 9984 | 0.788 / 10000 | 0.785 / 10048 | 0.650 / 10002 |
| mixed | 64 | 0.777 / 9984 | 0.774 / 9984 | - | 0.785 / 10048 | - |

## Expected

| | Hit ratio | Peak |
| :--- | :--- | :--- |
| Ceiling | 0.800 (the 10000 most frequent keys always resident) | 10000 |
| Expected | 0.77 to 0.800 | ≤ 10000 |
| Abnormal | above 0.800 or below 0.77 | above 10000 |

Interpretation: [FINDINGS.md](./FINDINGS.md).
