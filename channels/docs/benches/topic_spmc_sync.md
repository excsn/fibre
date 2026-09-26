# Fibre Benchmark: `TopicSpmcSync`
**Test Machine:** MacBook M4 Pro

## Results

`TopicSpmcSync/Subs-{Subs}_MailboxCap-{MailboxCap}_Items-{Items}`

| Subs | MailboxCap | Items | Time (low / median / high) | Throughput (low / median / high) |
|---|---|---|---:|---:|
| 1 | 1 | 10000 | 1.2332 ms / 1.2391 ms / 1.2490 ms | 8.0064 Melem/s / 8.0702 Melem/s / 8.1090 Melem/s |
| 1 | 1 | 100000 | 5.4923 ms / 5.5337 ms / 5.5745 ms | 17.939 Melem/s / 18.071 Melem/s / 18.207 Melem/s |
| 1 | 128 | 10000 | 1.1468 ms / 1.1509 ms / 1.1585 ms | 8.6319 Melem/s / 8.6890 Melem/s / 8.7201 Melem/s |
| 1 | 128 | 100000 | 9.2659 ms / 9.3053 ms / 9.3365 ms | 10.711 Melem/s / 10.747 Melem/s / 10.792 Melem/s |
| 4 | 1 | 10000 | 3.3155 ms / 3.5708 ms / 3.8892 ms | 10.285 Melem/s / 11.202 Melem/s / 12.065 Melem/s |
| 4 | 1 | 100000 | 32.873 ms / 33.679 ms / 34.443 ms | 11.613 Melem/s / 11.877 Melem/s / 12.168 Melem/s |
| 4 | 128 | 10000 | 4.2012 ms / 4.3062 ms / 4.5013 ms | 8.8864 Melem/s / 9.2888 Melem/s / 9.5211 Melem/s |
| 4 | 128 | 100000 | 38.007 ms / 38.558 ms / 39.049 ms | 10.244 Melem/s / 10.374 Melem/s / 10.524 Melem/s |
| 14 | 1 | 10000 | 194.96 ms / 198.35 ms / 201.33 ms | 695.36 Kelem/s / 705.81 Kelem/s / 718.11 Kelem/s |
| 14 | 1 | 100000 | 1.8222 s / 1.9265 s / 1.9991 s | 700.32 Kelem/s / 726.71 Kelem/s / 768.29 Kelem/s |
| 14 | 128 | 10000 | 194.14 ms / 198.32 ms / 202.42 ms | 691.63 Kelem/s / 705.93 Kelem/s / 721.12 Kelem/s |
| 14 | 128 | 100000 | 1.9320 s / 1.9561 s / 1.9780 s | 707.77 Kelem/s / 715.73 Kelem/s / 724.63 Kelem/s |
