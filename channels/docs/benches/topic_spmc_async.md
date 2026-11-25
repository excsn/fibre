# Fibre Benchmark: `TopicSpmcAsync`
**Test Machine:** MacBook M4 Pro

## Results

`TopicSpmcAsync/Subs-{Subs}_MailboxCap-{MailboxCap}_Items-{Items}`

| Subs | MailboxCap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 10000 | 1.1965 ms / 1.2054 ms / 1.2218 ms | 8.1845 Melem/s / 8.2962 Melem/s / 8.3575 Melem/s |
| 1 | 1 | 100000 | 9.1675 ms / 9.4514 ms / 9.6783 ms | 10.332 Melem/s / 10.580 Melem/s / 10.908 Melem/s |
| 1 | 128 | 10000 | 1.2205 ms / 1.2395 ms / 1.2634 ms | 7.9151 Melem/s / 8.0677 Melem/s / 8.1934 Melem/s |
| 1 | 128 | 100000 | 11.817 ms / 11.925 ms / 12.002 ms | 8.3320 Melem/s / 8.3856 Melem/s / 8.4622 Melem/s |
| 4 | 1 | 10000 | 4.9090 ms / 4.9716 ms / 5.0197 ms | 7.9686 Melem/s / 8.0457 Melem/s / 8.1483 Melem/s |
| 4 | 1 | 100000 | 48.198 ms / 48.976 ms / 49.448 ms | 8.0893 Melem/s / 8.1673 Melem/s / 8.2991 Melem/s |
| 4 | 128 | 10000 | 2.4820 ms / 2.5078 ms / 2.5384 ms | 15.758 Melem/s / 15.951 Melem/s / 16.116 Melem/s |
| 4 | 128 | 100000 | 24.290 ms / 24.397 ms / 24.625 ms | 16.244 Melem/s / 16.395 Melem/s / 16.468 Melem/s |
| 14 | 1 | 10000 | 10.136 ms / 10.288 ms / 10.355 ms | 13.520 Melem/s / 13.608 Melem/s / 13.812 Melem/s |
| 14 | 1 | 100000 | 104.82 ms / 105.87 ms / 106.62 ms | 13.131 Melem/s / 13.224 Melem/s / 13.356 Melem/s |
| 14 | 128 | 10000 | 6.4468 ms / 6.5115 ms / 6.6346 ms | 21.102 Melem/s / 21.500 Melem/s / 21.716 Melem/s |
| 14 | 128 | 100000 | 63.793 ms / 64.864 ms / 66.605 ms | 21.020 Melem/s / 21.583 Melem/s / 21.946 Melem/s |
