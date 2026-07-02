# Fibre Benchmark: `MpmcV2 Async`
**Test Machine:** MacBook M4 Pro

## Unbounded Baseline Results (`MpmcUnboundedAsync`)

### `MpmcUnboundedAsync/Prod-1_Cons-1_Items-100000`
- **Time:** 1.4288 ms – 1.4591 ms – 1.5086 ms  
- **Throughput:** 66.287 Melem/s – 68.536 Melem/s – 69.990 Melem/s

### `MpmcUnboundedAsync/Prod-1_Cons-1_Items-1000000`
- **Time:** 12.274 ms – 12.314 ms – 12.344 ms  
- **Throughput:** 81.012 Melem/s – 81.211 Melem/s – 81.475 Melem/s

---

### `MpmcUnboundedAsync/Prod-1_Cons-4_Items-100000`
- **Time:** 2.5041 ms – 2.5112 ms – 2.5218 ms  
- **Throughput:** 39.655 Melem/s – 39.822 Melem/s – 39.934 Melem/s

### `MpmcUnboundedAsync/Prod-1_Cons-4_Items-1000000`
- **Time:** 23.970 ms – 24.005 ms – 24.041 ms  
- **Throughput:** 41.595 Melem/s – 41.658 Melem/s – 41.718 Melem/s

---

### `MpmcUnboundedAsync/Prod-1_Cons-14_Items-100000`
- **Time:** 3.8596 ms – 3.9481 ms – 4.0048 ms  
- **Throughput:** 24.970 Melem/s – 25.328 Melem/s – 25.910 Melem/s

### `MpmcUnboundedAsync/Prod-1_Cons-14_Items-1000000`
- **Time:** 40.394 ms – 40.489 ms – 40.592 ms  
- **Throughput:** 24.635 Melem/s – 24.698 Melem/s – 24.756 Melem/s

---

### `MpmcUnboundedAsync/Prod-4_Cons-1_Items-100000`
- **Time:** 3.6758 ms – 3.7579 ms – 3.9300 ms  
- **Throughput:** 25.445 Melem/s – 26.610 Melem/s – 27.205 Melem/s

### `MpmcUnboundedAsync/Prod-4_Cons-1_Items-1000000`
- **Time:** 47.353 ms – 51.667 ms – 56.631 ms  
- **Throughput:** 17.658 Melem/s – 19.355 Melem/s – 21.118 Melem/s

---

### `MpmcUnboundedAsync/Prod-4_Cons-4_Items-100000`
- **Time:** 6.4854 ms – 6.8433 ms – 7.4364 ms  
- **Throughput:** 13.447 Melem/s – 14.613 Melem/s – 15.419 Melem/s

### `MpmcUnboundedAsync/Prod-4_Cons-4_Items-1000000`
- **Time:** 90.811 ms – 92.175 ms – 92.898 ms  
- **Throughput:** 10.765 Melem/s – 10.849 Melem/s – 11.012 Melem/s

---

### `MpmcUnboundedAsync/Prod-4_Cons-14_Items-100000`
- **Time:** 6.4115 ms – 6.6077 ms – 7.0243 ms  
- **Throughput:** 14.236 Melem/s – 15.134 Melem/s – 15.597 Melem/s

### `MpmcUnboundedAsync/Prod-4_Cons-14_Items-1000000`
- **Time:** 83.762 ms – 84.502 ms – 85.676 ms  
- **Throughput:** 11.672 Melem/s – 11.834 Melem/s – 11.939 Melem/s

---

### `MpmcUnboundedAsync/Prod-14_Cons-1_Items-100000`
- **Time:** 7.7411 ms – 7.7540 ms – 7.7703 ms  
- **Throughput:** 12.870 Melem/s – 12.897 Melem/s – 12.918 Melem/s

### `MpmcUnboundedAsync/Prod-14_Cons-1_Items-1000000`
- **Time:** 78.872 ms – 79.374 ms – 79.727 ms  
- **Throughput:** 12.543 Melem/s – 12.599 Melem/s – 12.679 Melem/s

---

### `MpmcUnboundedAsync/Prod-14_Cons-4_Items-100000`
- **Time:** 8.6192 ms – 8.6891 ms – 8.7242 ms  
- **Throughput:** 11.462 Melem/s – 11.509 Melem/s – 11.602 Melem/s

### `MpmcUnboundedAsync/Prod-14_Cons-4_Items-1000000`
- **Time:** 89.354 ms – 89.871 ms – 90.255 ms  
- **Throughput:** 11.080 Melem/s – 11.127 Melem/s – 11.191 Melem/s

---

### `MpmcUnboundedAsync/Prod-14_Cons-14_Items-100000`
- **Time:** 8.6089 ms – 8.7201 ms – 8.7921 ms  
- **Throughput:** 11.374 Melem/s – 11.468 Melem/s – 11.616 Melem/s

### `MpmcUnboundedAsync/Prod-14_Cons-14_Items-1000000`
- **Time:** 88.544 ms – 90.033 ms – 91.498 ms  
- **Throughput:** 10.929 Melem/s – 11.107 Melem/s – 11.294 Melem/s

---

## Bounded Results (`MpmcAsync`)

### Capacity: 4 (`Cap-4`)

#### `MpmcAsync/Cap-4_Prod-1_Cons-1_Items-100000`
- **Time:** 3.1281 ms – 3.2045 ms – 3.2965 ms  
- **Throughput:** 30.335 Melem/s – 31.206 Melem/s – 31.968 Melem/s

#### `MpmcAsync/Cap-4_Prod-1_Cons-1_Items-1000000`
- **Time:** 31.536 ms – 31.630 ms – 31.769 ms  
- **Throughput:** 31.478 Melem/s – 31.615 Melem/s – 31.710 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-1_Cons-4_Items-100000`
- **Time:** 5.7460 ms – 5.8130 ms – 5.9007 ms  
- **Throughput:** 16.947 Melem/s – 17.203 Melem/s – 17.403 Melem/s

#### `MpmcAsync/Cap-4_Prod-1_Cons-4_Items-1000000`
- **Time:** 54.574 ms – 55.167 ms – 56.162 ms  
- **Throughput:** 17.806 Melem/s – 18.127 Melem/s – 18.324 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-1_Cons-14_Items-100000`
- **Time:** 9.9050 ms – 10.033 ms – 10.142 ms  
- **Throughput:** 9.8602 Melem/s – 9.9674 Melem/s – 10.096 Melem/s

#### `MpmcAsync/Cap-4_Prod-1_Cons-14_Items-1000000`
- **Time:** 98.224 ms – 98.623 ms – 99.500 ms  
- **Throughput:** 10.050 Melem/s – 10.140 Melem/s – 10.181 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-4_Cons-1_Items-100000`
- **Time:** 5.6293 ms – 5.6612 ms – 5.6925 ms  
- **Throughput:** 17.567 Melem/s – 17.664 Melem/s – 17.764 Melem/s

#### `MpmcAsync/Cap-4_Prod-4_Cons-1_Items-1000000`
- **Time:** 56.865 ms – 57.089 ms – 57.355 ms  
- **Throughput:** 17.435 Melem/s – 17.516 Melem/s – 17.586 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-4_Cons-4_Items-100000`
- **Time:** 9.8786 ms – 10.078 ms – 10.263 ms  
- **Throughput:** 9.7436 Melem/s – 9.9224 Melem/s – 10.123 Melem/s

#### `MpmcAsync/Cap-4_Prod-4_Cons-4_Items-1000000`
- **Time:** 99.681 ms – 102.33 ms – 105.19 ms  
- **Throughput:** 9.5064 Melem/s – 9.7720 Melem/s – 10.032 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-4_Cons-14_Items-100000`
- **Time:** 12.916 ms – 13.485 ms – 13.866 ms  
- **Throughput:** 7.2119 Melem/s – 7.4154 Melem/s – 7.7426 Melem/s

#### `MpmcAsync/Cap-4_Prod-4_Cons-14_Items-1000000`
- **Time:** 133.33 ms – 134.48 ms – 135.29 ms  
- **Throughput:** 7.3913 Melem/s – 7.4359 Melem/s – 7.5001 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-14_Cons-1_Items-100000`
- **Time:** 10.875 ms – 10.904 ms – 10.957 ms  
- **Throughput:** 9.1266 Melem/s – 9.1714 Melem/s – 9.1952 Melem/s

#### `MpmcAsync/Cap-4_Prod-14_Cons-1_Items-1000000`
- **Time:** 109.62 ms – 110.06 ms – 110.48 ms  
- **Throughput:** 9.0515 Melem/s – 9.0860 Melem/s – 9.1227 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-14_Cons-4_Items-100000`
- **Time:** 13.997 ms – 14.463 ms – 14.748 ms  
- **Throughput:** 6.7805 Melem/s – 6.9143 Melem/s – 7.1443 Melem/s

#### `MpmcAsync/Cap-4_Prod-14_Cons-4_Items-1000000`
- **Time:** 142.28 ms – 145.82 ms – 148.17 ms  
- **Throughput:** 6.7491 Melem/s – 6.8579 Melem/s – 7.0286 Melem/s

---

#### `MpmcAsync/Cap-4_Prod-14_Cons-14_Items-100000`
- **Time:** 11.229 ms – 11.297 ms – 11.343 ms  
- **Throughput:** 8.8156 Melem/s – 8.8522 Melem/s – 8.9056 Melem/s

#### `MpmcAsync/Cap-4_Prod-14_Cons-14_Items-1000000`
- **Time:** 111.44 ms – 112.56 ms – 114.13 ms  
- **Throughput:** 8.7616 Melem/s – 8.8844 Melem/s – 8.9736 Melem/s


### Capacity: 128 (`Cap-128`)

#### `MpmcAsync/Cap-128_Prod-1_Cons-1_Items-100000`
- **Time:** 1.2903 ms – 1.2957 ms – 1.3022 ms  
- **Throughput:** 76.794 Melem/s – 77.176 Melem/s – 77.501 Melem/s

#### `MpmcAsync/Cap-128_Prod-1_Cons-1_Items-1000000`
- **Time:** 12.757 ms – 12.892 ms – 12.954 ms  
- **Throughput:** 77.197 Melem/s – 77.568 Melem/s – 78.391 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-1_Cons-4_Items-100000`
- **Time:** 2.7543 ms – 2.7749 ms – 2.7962 ms  
- **Throughput:** 35.763 Melem/s – 36.037 Melem/s – 36.307 Melem/s

#### `MpmcAsync/Cap-128_Prod-1_Cons-4_Items-1000000`
- **Time:** 27.509 ms – 27.624 ms – 27.788 ms  
- **Throughput:** 35.987 Melem/s – 36.201 Melem/s – 36.352 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-1_Cons-14_Items-100000`
- **Time:** 4.1203 ms – 4.1320 ms – 4.1410 ms  
- **Throughput:** 24.149 Melem/s – 24.202 Melem/s – 24.270 Melem/s

#### `MpmcAsync/Cap-128_Prod-1_Cons-14_Items-1000000`
- **Time:** 41.351 ms – 41.493 ms – 41.670 ms  
- **Throughput:** 23.998 Melem/s – 24.100 Melem/s – 24.183 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-4_Cons-1_Items-100000`
- **Time:** 2.6326 ms – 2.6530 ms – 2.6648 ms  
- **Throughput:** 37.527 Melem/s – 37.694 Melem/s – 37.985 Melem/s

#### `MpmcAsync/Cap-128_Prod-4_Cons-1_Items-1000000`
- **Time:** 26.795 ms – 26.968 ms – 27.149 ms  
- **Throughput:** 36.834 Melem/s – 37.081 Melem/s – 37.320 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-4_Cons-4_Items-100000`
- **Time:** 4.3614 ms – 4.3846 ms – 4.4006 ms  
- **Throughput:** 22.724 Melem/s – 22.807 Melem/s – 22.929 Melem/s

#### `MpmcAsync/Cap-128_Prod-4_Cons-4_Items-1000000`
- **Time:** 44.327 ms – 44.554 ms – 44.746 ms  
- **Throughput:** 22.349 Melem/s – 22.445 Melem/s – 22.560 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-4_Cons-14_Items-100000`
- **Time:** 5.6997 ms – 5.8834 ms – 6.0231 ms  
- **Throughput:** 16.603 Melem/s – 16.997 Melem/s – 17.545 Melem/s

#### `MpmcAsync/Cap-128_Prod-4_Cons-14_Items-1000000`
- **Time:** 59.632 ms – 60.031 ms – 60.469 ms  
- **Throughput:** 16.537 Melem/s – 16.658 Melem/s – 16.770 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-14_Cons-1_Items-100000`
- **Time:** 3.9470 ms – 3.9661 ms – 3.9945 ms  
- **Throughput:** 25.034 Melem/s – 25.214 Melem/s – 25.336 Melem/s

#### `MpmcAsync/Cap-128_Prod-14_Cons-1_Items-1000000`
- **Time:** 39.837 ms – 40.003 ms – 40.180 ms  
- **Throughput:** 24.888 Melem/s – 24.998 Melem/s – 25.102 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-14_Cons-4_Items-100000`
- **Time:** 6.1071 ms – 6.2509 ms – 6.3528 ms  
- **Throughput:** 15.741 Melem/s – 15.998 Melem/s – 16.374 Melem/s

#### `MpmcAsync/Cap-128_Prod-14_Cons-4_Items-1000000`
- **Time:** 62.744 ms – 63.246 ms – 63.755 ms  
- **Throughput:** 15.685 Melem/s – 15.811 Melem/s – 15.938 Melem/s

---

#### `MpmcAsync/Cap-128_Prod-14_Cons-14_Items-100000`
- **Time:** 4.6909 ms – 4.7219 ms – 4.7626 ms  
- **Throughput:** 20.997 Melem/s – 21.178 Melem/s – 21.318 Melem/s

#### `MpmcAsync/Cap-128_Prod-14_Cons-14_Items-1000000`
- **Time:** 45.999 ms – 46.128 ms – 46.315 ms  
- **Throughput:** 21.591 Melem/s – 21.679 Melem/s – 21.739 Melem/s