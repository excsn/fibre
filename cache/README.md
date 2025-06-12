
### Feature Comparison: `fibre_cache` vs. Moka, Caffeine, Stretto

| Feature | Moka / Caffeine | Stretto / Ristretto | **Our `fibre_cache` (Current Plan)** | Status & Notes |
| :--- | :--- | :--- | :--- | :--- |
| **Concurrency Model** | Sharded, Fine-Grained Locks | Sharded, Fine-Grained Locks | ✅ Sharded, Custom Hybrid Lock | **Complete.** Our `HybridRwLock` is a high-performance solution. |
| **Sync & Async API** | ✅ (via `moka`) | ✅ (via `stretto`) | ✅ | **Complete.** Both `Cache` and `AsyncCache` handles with proper async blocking. |
| **`K: Clone` Required?** | Yes | No (`u64` keys) | ✅ **No.** (`Arc` based) | **Complete.** Core goal achieved. |
| **TTL / TTI Support** | ✅ | ✅ | ✅ | **Complete.** Handled by the background Janitor task. |
| **Cost-Based Eviction** | ✅ | ✅ | ✅ | **Complete.** Integrated with `CostGate` and `insert(..., cost)`. |
| **Eviction Policy** | Segmented-LRU (approximates LFU) | Sampled-LFU (TinyLFU) | ✅ **TinyLFU** (Pluggable) | **Complete.** We have a solid `TinyLfu` implementation. |
| **Admission Policy** | Implicit (part of Seg-LRU) | ✅ Probabilistic "Doorkeeper" | ✅ **Integrated into `TinyLfu`** (Pluggable) | **Complete.** Our policy now handles admission based on frequency. |
| **`clear()` Method** | ✅ | ✅ | ✅ | **Complete.** |
| **Eviction Listener** | ✅ | ✅ | ✅ | **Complete.** Handled by the background `Notifier` task. |
| **In-place Mutation** | ✅ (`compute`) | ❌ | ✅ (`compute`) | **Complete.** |
| **`entry` API (Atomic Get-or-Insert)** | ✅ (Excellent `entry` API) | ❌ | ✅ | **Complete.** Mimics `HashMap`'s powerful and safe API. |
| **Metrics / Stats** | ✅ | ✅ | ✅ | **Complete.** |
| **Cache Loader (on miss)** | ✅ (Core feature) | ❌ | ❌ | **Major Gap.** This is the most significant missing feature for general-purpose use. |
| **Return Stale, Refresh Async** | ✅ | ❌ | ❌ | **Deferred.** A powerful feature for latency-sensitive apps, but requires a cache loader. |
| **Weak/Soft References** | ✅ | ❌ | ❌ | **Deferred.** An advanced memory management feature. Our `Arc`-based design is strong-only. |
| **Bulk Operations (`insert_all`)**| ✅ | ✅ | ❌ | **Minor Gap.** Can be simulated with a loop, but a dedicated method is more efficient. |
| **Persistence (`serde`)** | ❌ (Not built-in) | ❌ (Not built-in) | ⏳ **Next Step** | The final milestone in our current plan. |
