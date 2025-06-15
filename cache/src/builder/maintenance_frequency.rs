
/// Recommended for most general-purpose workloads.
///
/// Balances responsiveness with low overhead by running maintenance, on average,
/// once every 16 inserts per shard. This keeps the memory footprint smooth and
/// predictable without impacting performance for most applications.
pub const RESPONSIVE: u32 = 16;

/// Recommended for write-heavy workloads where insert throughput is critical.
///
/// Reduces CPU overhead by running maintenance less frequently (once every 64
/// inserts per shard). This is ideal for applications that can tolerate
/// slightly larger, temporary spikes in memory usage in exchange for maximizing
/// the speed of individual insert operations.
pub const THROUGHPUT: u32 = 64;

/// Recommended for specialized, extremely high-throughput systems.
///
/// Minimizes maintenance-related CPU chatter as much as possible by running
/// it very infrequently (once every 256 inserts per shard). This setting
/// should be used with caution, as it may allow the cache's event buffers
/// and memory usage to grow significantly between maintenance cycles.
pub const LOW_OVERHEAD: u32 = 256;
