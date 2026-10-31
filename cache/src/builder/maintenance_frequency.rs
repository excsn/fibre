
/// Recommended for most general-purpose workloads.
///
/// A janitor tick applies a chosen shard's pending reads, on average, once in 16 tries.
pub const RESPONSIVE: u32 = 16;

/// Recommended for write-heavy workloads where insert throughput is critical.
///
/// A janitor tick applies a chosen shard's pending reads, on average, once in 64 tries.
pub const THROUGHPUT: u32 = 64;

/// Recommended for specialized, extremely high-throughput systems.
///
/// A janitor tick applies a chosen shard's pending reads, on average, once in 256 tries.
pub const LOW_OVERHEAD: u32 = 256;
