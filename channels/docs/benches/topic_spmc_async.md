# Fibre Benchmark: `TopicSpmcAsync`
**Test Machine:** MacBook M4 Pro

## Results

`TopicSpmcAsync/Subs-{Subs}_MailboxCap-{MailboxCap}_Items-{Items}`

| Subs | MailboxCap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 10000 | 1.2469 ms / 1.2584 ms / 1.2718 ms | 7.8631 Melem/s / 7.9468 Melem/s / 8.0196 Melem/s |
| 1 | 1 | 100000 | 12.092 ms / 12.788 ms / 13.169 ms | 7.5934 Melem/s / 7.8195 Melem/s / 8.2699 Melem/s |
| 1 | 128 | 10000 | 2.2724 ms / 2.6894 ms / 2.8773 ms | 3.4754 Melem/s / 3.7184 Melem/s / 4.4007 Melem/s |
| 1 | 128 | 100000 | 24.891 ms / 27.554 ms / 28.707 ms | 3.4835 Melem/s / 3.6293 Melem/s / 4.0174 Melem/s |
| 4 | 1 | 10000 | 3.9204 ms / 3.9463 ms / 3.9990 ms | 10.002 Melem/s / 10.136 Melem/s / 10.203 Melem/s |
| 4 | 1 | 100000 | 39.530 ms / 40.099 ms / 40.523 ms | 9.8710 Melem/s / 9.9753 Melem/s / 10.119 Melem/s |
| 4 | 128 | 10000 | 3.1907 ms / 3.2013 ms / 3.2121 ms | 12.453 Melem/s / 12.495 Melem/s / 12.537 Melem/s |
| 4 | 128 | 100000 | 32.042 ms / 32.348 ms / 32.927 ms | 12.148 Melem/s / 12.365 Melem/s / 12.484 Melem/s |
| 14 | 1 | 10000 | 9.6583 ms / 9.7436 ms / 9.8304 ms | 14.242 Melem/s / 14.368 Melem/s / 14.495 Melem/s |
| 14 | 1 | 100000 | 92.849 ms / 94.064 ms / 95.400 ms | 14.675 Melem/s / 14.883 Melem/s / 15.078 Melem/s |
| 14 | 128 | 10000 | 12.011 ms / 12.167 ms / 12.378 ms | 11.311 Melem/s / 11.507 Melem/s / 11.656 Melem/s |
| 14 | 128 | 100000 | 117.48 ms / 119.12 ms / 121.22 ms | 11.550 Melem/s / 11.753 Melem/s / 11.917 Melem/s |
