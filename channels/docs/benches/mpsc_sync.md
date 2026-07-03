# Fibre Benchmark: `MpscSync`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpscUnboundedSync`)

### `MpscUnboundedSync/Prod-1_Items-100000`
- **Time:** 480.52 µs – 496.84 µs – 509.60 µs  
- **Throughput:** 196.23 Melem/s – 201.27 Melem/s – 208.11 Melem/s

### `MpscUnboundedSync/Prod-1_Items-1000000`
- **Time:** 4.6009 ms – 4.7551 ms – 5.0657 ms  
- **Throughput:** 197.41 Melem/s – 210.30 Melem/s – 217.35 Melem/s

### `MpscUnboundedSync/Prod-1_Items-10000000`
- **Time:** 46.309 ms – 47.393 ms – 48.601 ms  
- **Throughput:** 205.76 Melem/s – 211.00 Melem/s – 215.94 Melem/s

---

### `MpscUnboundedSync/Prod-4_Items-100000`
- **Time:** 4.3888 ms – 4.6043 ms – 4.7374 ms  
- **Throughput:** 21.109 Melem/s – 21.719 Melem/s – 22.785 Melem/s

### `MpscUnboundedSync/Prod-4_Items-1000000`
- **Time:** 46.203 ms – 47.668 ms – 50.004 ms  
- **Throughput:** 19.999 Melem/s – 20.978 Melem/s – 21.644 Melem/s

### `MpscUnboundedSync/Prod-4_Items-10000000`
- **Time:** 485.68 ms – 501.40 ms – 520.14 ms  
- **Throughput:** 19.226 Melem/s – 19.944 Melem/s – 20.590 Melem/s

---

### `MpscUnboundedSync/Prod-14_Items-100000`
- **Time:** 7.1674 ms – 7.1761 ms – 7.1907 ms  
- **Throughput:** 13.907 Melem/s – 13.935 Melem/s – 13.952 Melem/s

### `MpscUnboundedSync/Prod-14_Items-1000000`
- **Time:** 75.669 ms – 75.862 ms – 76.079 ms  
- **Throughput:** 13.144 Melem/s – 13.182 Melem/s – 13.215 Melem/s

### `MpscUnboundedSync/Prod-14_Items-10000000`
- **Time:** 755.37 ms – 764.96 ms – 771.95 ms  
- **Throughput:** 12.954 Melem/s – 13.073 Melem/s – 13.239 Melem/s


## Bounded Results (`MpscBoundedSync`)

### Capacity: 1 (`Cap-1`)

#### `MpscBoundedSync/Cap-1_Prod-1_Items-100000`
- **Time:** 19.101 ms – 21.123 ms – 22.965 ms  
- **Throughput:** 4.3544 Melem/s – 4.7342 Melem/s – 5.2353 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-1000000`
- **Time:** 173.52 ms – 176.54 ms – 179.78 ms  
- **Throughput:** 5.5625 Melem/s – 5.6645 Melem/s – 5.7630 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-1_Items-10000000`
- **Time:** 1.7432 s – 1.7946 s – 1.8528 s  
- **Throughput:** 5.3973 Melem/s – 5.5724 Melem/s – 5.7364 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-4_Items-100000`
- **Time:** 33.409 ms – 34.632 ms – 36.399 ms  
- **Throughput:** 2.7473 Melem/s – 2.8875 Melem/s – 2.9932 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-1000000`
- **Time:** 311.38 ms – 322.55 ms – 334.86 ms  
- **Throughput:** 2.9864 Melem/s – 3.1003 Melem/s – 3.2115 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-4_Items-10000000`
- **Time:** 3.1835 s – 3.3194 s – 3.4704 s  
- **Throughput:** 2.8815 Melem/s – 3.0126 Melem/s – 3.1412 Melem/s

---

#### `MpscBoundedSync/Cap-1_Prod-14_Items-100000`
- **Time:** 89.018 ms – 89.582 ms – 89.935 ms  
- **Throughput:** 1.1119 Melem/s – 1.1163 Melem/s – 1.1234 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-1000000`
- **Time:** 903.09 ms – 908.02 ms – 914.55 ms  
- **Throughput:** 1.0934 Melem/s – 1.1013 Melem/s – 1.1073 Melem/s

#### `MpscBoundedSync/Cap-1_Prod-14_Items-10000000`
- **Time:** 9.0113 s – 9.1163 s – 9.2233 s  
- **Throughput:** 1.0842 Melem/s – 1.0969 Melem/s – 1.1097 Melem/s


### Capacity: 4 (`Cap-4`)

#### `MpscBoundedSync/Cap-4_Prod-1_Items-100000`
- **Time:** 14.269 ms – 14.323 ms – 14.387 ms  
- **Throughput:** 6.9507 Melem/s – 6.9818 Melem/s – 7.0084 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-1000000`
- **Time:** 141.15 ms – 142.30 ms – 144.45 ms  
- **Throughput:** 6.9228 Melem/s – 7.0272 Melem/s – 7.0848 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-1_Items-10000000`
- **Time:** 1.4116 s – 1.4254 s – 1.4402 s  
- **Throughput:** 6.9436 Melem/s – 7.0157 Melem/s – 7.0843 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-4_Items-100000`
- **Time:** 20.512 ms – 21.311 ms – 21.976 ms  
- **Throughput:** 4.5504 Melem/s – 4.6925 Melem/s – 4.8752 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-1000000`
- **Time:** 185.22 ms – 186.68 ms – 188.31 ms  
- **Throughput:** 5.3104 Melem/s – 5.3567 Melem/s – 5.3989 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-4_Items-10000000`
- **Time:** 1.8558 s – 1.8623 s – 1.8693 s  
- **Throughput:** 5.3497 Melem/s – 5.3698 Melem/s – 5.3884 Melem/s

---

#### `MpscBoundedSync/Cap-4_Prod-14_Items-100000`
- **Time:** 44.964 ms – 45.016 ms – 45.090 ms  
- **Throughput:** 2.2178 Melem/s – 2.2214 Melem/s – 2.2240 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-1000000`
- **Time:** 447.16 ms – 448.05 ms – 449.02 ms  
- **Throughput:** 2.2271 Melem/s – 2.2319 Melem/s – 2.2363 Melem/s

#### `MpscBoundedSync/Cap-4_Prod-14_Items-10000000`
- **Time:** 4.4633 s – 4.4793 s – 4.4924 s  
- **Throughput:** 2.2260 Melem/s – 2.2325 Melem/s – 2.2405 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpscBoundedSync/Cap-128_Prod-1_Items-100000`
- **Time:** 10.907 ms – 11.443 ms – 11.954 ms  
- **Throughput:** 8.3657 Melem/s – 8.7388 Melem/s – 9.1685 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-1000000`
- **Time:** 112.29 ms – 112.95 ms – 113.28 ms  
- **Throughput:** 8.8274 Melem/s – 8.8536 Melem/s – 8.9058 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-1_Items-10000000`
- **Time:** 1.1099 s – 1.1204 s – 1.1355 s  
- **Throughput:** 8.8065 Melem/s – 8.9252 Melem/s – 9.0100 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-4_Items-100000`
- **Time:** 9.8223 ms – 9.9810 ms – 10.387 ms  
- **Throughput:** 9.6272 Melem/s – 10.019 Melem/s – 10.181 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-1000000`
- **Time:** 95.544 ms – 97.544 ms – 99.855 ms  
- **Throughput:** 10.015 Melem/s – 10.252 Melem/s – 10.466 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-4_Items-10000000`
- **Time:** 969.07 ms – 1.0840 s – 1.2105 s  
- **Throughput:** 8.2611 Melem/s – 9.2248 Melem/s – 10.319 Melem/s

---

#### `MpscBoundedSync/Cap-128_Prod-14_Items-100000`
- **Time:** 35.143 ms – 35.874 ms – 36.609 ms  
- **Throughput:** 2.7315 Melem/s – 2.7875 Melem/s – 2.8455 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-1000000`
- **Time:** 354.82 ms – 358.25 ms – 361.95 ms  
- **Throughput:** 2.7628 Melem/s – 2.7913 Melem/s – 2.8184 Melem/s

#### `MpscBoundedSync/Cap-128_Prod-14_Items-10000000`
- **Time:** 3.6203 s – 3.6416 s – 3.6658 s  
- **Throughput:** 2.7279 Melem/s – 2.7461 Melem/s – 2.7622 Melem/s