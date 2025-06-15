use std::fmt;

/// Errors that can occur when building a cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
  /// The cache was configured with a capacity of zero, which is not allowed
  /// for a bounded cache. Use `unbounded()` for an unbounded cache.
  ZeroCapacity,
  /// The cache was configured with zero shards, which is not allowed.
  ZeroShards,
  /// An `async_loader` was provided, but no `TaskSpawner` was configured
  /// and the default `tokio` feature is not enabled.
  SpawnerRequired,
}

impl fmt::Display for BuildError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      BuildError::ZeroCapacity => write!(f, "bounded cache capacity cannot be zero"),
      BuildError::ZeroShards => write!(f, "shard count cannot be zero"),
      BuildError::SpawnerRequired => write!(
        f,
        "an async loader requires a task spawner or the 'tokio' feature"
      ),
    }
  }
}

impl std::error::Error for BuildError {}

pub enum ComputeResult<R> {
  Ok(R),
  Fail,
  NotFound,
}