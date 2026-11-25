# Fibre Benchmark: `TopicSpmcSync`
**Test Machine:** MacBook M4 Pro

## Results

`TopicSpmcSync/Subs-{Subs}_MailboxCap-{MailboxCap}_Items-{Items}`

| Subs | MailboxCap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 10000 | 1.7946 ms / 1.8044 ms / 1.8105 ms | 5.5233 Melem/s / 5.5421 Melem/s / 5.5722 Melem/s |
| 1 | 1 | 100000 | 13.306 ms / 13.395 ms / 13.472 ms | 7.4227 Melem/s / 7.4652 Melem/s / 7.5153 Melem/s |
| 1 | 128 | 10000 | 1.5510 ms / 1.6241 ms / 1.6541 ms | 6.0455 Melem/s / 6.1572 Melem/s / 6.4474 Melem/s |
| 1 | 128 | 100000 | 16.533 ms / 16.588 ms / 16.639 ms | 6.0099 Melem/s / 6.0286 Melem/s / 6.0486 Melem/s |
| 4 | 1 | 10000 | 3.5674 ms / 3.6522 ms / 3.7490 ms | 10.670 Melem/s / 10.952 Melem/s / 11.213 Melem/s |
| 4 | 1 | 100000 | 34.709 ms / 35.344 ms / 35.922 ms | 11.135 Melem/s / 11.317 Melem/s / 11.524 Melem/s |
| 4 | 128 | 10000 | 5.9755 ms / 6.0701 ms / 6.1773 ms | 6.4753 Melem/s / 6.5896 Melem/s / 6.6940 Melem/s |
| 4 | 128 | 100000 | 59.591 ms / 60.829 ms / 62.291 ms | 6.4215 Melem/s / 6.5759 Melem/s / 6.7124 Melem/s |
| 14 | 1 | 10000 | 169.92 ms / 188.16 ms / 200.35 ms | 698.78 Kelem/s / 744.05 Kelem/s / 823.91 Kelem/s |
| 14 | 1 | 100000 | 1.1159 s / 1.5955 s / 1.9443 s | 720.06 Kelem/s / 877.47 Kelem/s / 1.2546 Melem/s |
| 14 | 128 | 10000 | 172.24 ms / 187.66 ms / 198.85 ms | 704.05 Kelem/s / 746.04 Kelem/s / 812.84 Kelem/s |
| 14 | 128 | 100000 | 1.7258 s / 1.8761 s / 1.9963 s | 701.31 Kelem/s / 746.23 Kelem/s / 811.21 Kelem/s |
