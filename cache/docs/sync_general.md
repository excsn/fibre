SyncGeneral/Workload-Read100_Zipf_Cap-1000000_Ops-1000000_Threads-1
                        time:   [80.079 ms 84.438 ms 93.425 ms]
                        change: [-14.403% -6.7606% +1.2948%] (p = 0.14 > 0.05)
                        No change in performance detected.
SyncGeneral/Workload-Read100_Zipf_Cap-1000000_Ops-1000000_Threads-4
                        time:   [42.892 ms 44.563 ms 48.370 ms]
                        thrpt:  [20.674 Melem/s 22.440 Melem/s 23.315 Melem/s]
                 change:
                        time:   [-26.853% -21.129% -15.339%] (p = 0.00 < 0.05)
                        thrpt:  [+18.119% +26.790% +36.711%]
                        Performance has improved.
SyncGeneral/Workload-Read100_Zipf_Cap-1000000_Ops-1000000_Threads-8
                        time:   [59.880 ms 60.488 ms 61.125 ms]
                        thrpt:  [16.360 Melem/s 16.532 Melem/s 16.700 Melem/s]
                 change:
                        time:   [-16.530% -10.248% -3.8598%] (p = 0.01 < 0.05)
                        thrpt:  [+4.0148% +11.418% +19.804%]
                        Performance has improved.
Benchmarking SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-1: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.6s or enable flat sampling.
SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-1
                        time:   [122.31 ms 127.10 ms 135.33 ms]
                        thrpt:  [7.3891 Melem/s 7.8675 Melem/s 8.1759 Melem/s]
                 change:
                        time:   [-18.021% -5.8178% +5.9600%] (p = 0.43 > 0.05)
                        thrpt:  [-5.6248% +6.1772% +21.982%]
                        No change in performance detected.
Benchmarking SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-4: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.2s or enable flat sampling.
SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-4
                        time:   [72.580 ms 75.777 ms 82.316 ms]
                        thrpt:  [12.148 Melem/s 13.197 Melem/s 13.778 Melem/s]
                 change:
                        time:   [-17.133% -6.4756% +5.3622%] (p = 0.32 > 0.05)
                        thrpt:  [-5.0893% +6.9240% +20.676%]
                        No change in performance detected.
Benchmarking SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-8: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.5s or enable flat sampling.
SyncGeneral/Workload-Read75Write25_Zipf_Cap-1000000_Ops-1000000_Threads-8
                        time:   [81.013 ms 82.723 ms 86.243 ms]
                        thrpt:  [11.595 Melem/s 12.089 Melem/s 12.344 Melem/s]
                 change:
                        time:   [-28.024% -22.376% -16.135%] (p = 0.00 < 0.05)
                        thrpt:  [+19.240% +28.826% +38.936%]
                        Performance has improved.
Found 2 outliers among 10 measurements (20.00%)
  2 (20.00%) high mild
SyncGeneral/Workload-Write100_Zipf_Cap-1000000_Ops-1000000_Threads-1
                        time:   [268.84 ms 278.24 ms 289.47 ms]
                        thrpt:  [3.4545 Melem/s 3.5940 Melem/s 3.7197 Melem/s]
                 change:
                        time:   [-19.992% -11.939% -2.1689%] (p = 0.04 < 0.05)
                        thrpt:  [+2.2170% +13.557% +24.988%]
                        Performance has improved.
SyncGeneral/Workload-Write100_Zipf_Cap-1000000_Ops-1000000_Threads-4
                        time:   [208.24 ms 220.87 ms 231.69 ms]
                        thrpt:  [4.3160 Melem/s 4.5276 Melem/s 4.8022 Melem/s]
                 change:
                        time:   [-12.854% -6.0952% +0.0727%] (p = 0.08 > 0.05)
                        thrpt:  [-0.0726% +6.4909% +14.751%]
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
SyncGeneral/Workload-Write100_Zipf_Cap-1000000_Ops-1000000_Threads-8
                        time:   [242.01 ms 247.13 ms 253.12 ms]
                        thrpt:  [3.9508 Melem/s 4.0465 Melem/s 4.1321 Melem/s]
                 change:
                        time:   [-19.247% -16.209% -12.211%] (p = 0.00 < 0.05)
                        thrpt:  [+13.910% +19.344% +23.835%]
                        Performance has improved.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
SyncGeneral/Workload-Compute_SameKey_Cap-1000000_Ops-1000000_Threads-1
                        time:   [12.680 ms 12.729 ms 12.803 ms]
                        thrpt:  [78.104 Melem/s 78.562 Melem/s 78.861 Melem/s]
                 change:
                        time:   [+21.339% +22.275% +23.508%] (p = 0.00 < 0.05)
                        thrpt:  [-19.034% -18.217% -17.586%]
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
Benchmarking SyncGeneral/Workload-Compute_SameKey_Cap-1000000_Ops-1000000_Threads-4: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 7.6s or enable flat sampling.
SyncGeneral/Workload-Compute_SameKey_Cap-1000000_Ops-1000000_Threads-4
                        time:   [103.98 ms 107.71 ms 114.12 ms]
                        thrpt:  [8.7627 Melem/s 9.2838 Melem/s 9.6173 Melem/s]
                 change:
                        time:   [-23.149% -20.050% -16.576%] (p = 0.00 < 0.05)
                        thrpt:  [+19.870% +25.078% +30.122%]
                        Performance has improved.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high severe
SyncGeneral/Workload-Compute_SameKey_Cap-1000000_Ops-1000000_Threads-8
                        time:   [252.03 ms 259.34 ms 266.33 ms]
                        thrpt:  [3.7547 Melem/s 3.8559 Melem/s 3.9677 Melem/s]
                 change:
                        time:   [+8.4577% +12.098% +15.721%] (p = 0.00 < 0.05)
                        thrpt:  [-13.585% -10.792% -7.7981%]
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) low mild
Benchmarking SyncGeneral/Workload-Compute_Zipf_Cap-1000000_Ops-1000000_Threads-1: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.4s or enable flat sampling.
SyncGeneral/Workload-Compute_Zipf_Cap-1000000_Ops-1000000_Threads-1
                        time:   [74.737 ms 80.529 ms 86.959 ms]
                        thrpt:  [11.500 Melem/s 12.418 Melem/s 13.380 Melem/s]
                 change:
                        time:   [+0.8503% +8.9595% +17.832%] (p = 0.06 > 0.05)
                        thrpt:  [-15.133% -8.2228% -0.8431%]
                        No change in performance detected.
SyncGeneral/Workload-Compute_Zipf_Cap-1000000_Ops-1000000_Threads-4
                        time:   [39.090 ms 41.027 ms 43.061 ms]
                        thrpt:  [23.223 Melem/s 24.374 Melem/s 25.582 Melem/s]
                 change:
                        time:   [-33.109% -24.207% -14.896%] (p = 0.00 < 0.05)
                        thrpt:  [+17.503% +31.938% +49.496%]
                        Performance has improved.
Benchmarking SyncGeneral/Workload-Compute_Zipf_Cap-1000000_Ops-1000000_Threads-8: Warming up for 3.0000 s
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 5.4s or enable flat sampling.
SyncGeneral/Workload-Compute_Zipf_Cap-1000000_Ops-1000000_Threads-8
                        time:   [60.267 ms 60.605 ms 61.202 ms]
                        thrpt:  [16.339 Melem/s 16.500 Melem/s 16.593 Melem/s]
                 change:
                        time:   [-16.395% -11.182% -6.5566%] (p = 0.00 < 0.05)
                        thrpt:  [+7.0167% +12.590% +19.610%]
                        Performance has improved.