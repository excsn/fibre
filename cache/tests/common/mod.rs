use std::hash::{BuildHasher, Hasher};

use fibre_cache::{AsyncCache, Cache, CacheBuilder};

// A custom hasher that allows us to control which shard a key is assigned to.
// It simply uses the integer value of the key as its hash.
// For a 4-shard cache:
// - key 0 -> shard 0 (0 % 4 = 0)
// - key 1 -> shard 1 (1 % 4 = 1)
// - key 2 -> shard 2 (2 % 4 = 2)
// - key 3 -> shard 3 (3 % 4 = 3)
// - key 4 -> shard 0 (4 % 4 = 0)
#[derive(Clone, Default)]
pub struct ShardControllingHasher;
impl BuildHasher for ShardControllingHasher {
  type Hasher = TestHasher;
  fn build_hasher(&self) -> Self::Hasher {
    TestHasher(0)
  }
}
pub struct TestHasher(u64);
impl Hasher for TestHasher {
  fn finish(&self) -> u64 {
    self.0
  }
  fn write(&mut self, _: &[u8]) {
    unimplemented!()
  }
  fn write_i32(&mut self, i: i32) {
    self.0 = i as u64;
  }
}

pub fn build_test_cache(shards: usize) -> Cache<i32, String, ShardControllingHasher> {
  CacheBuilder::new()
    .shards(shards)
    .hasher(ShardControllingHasher)
    .build()
    .unwrap()
}

// Helper to build a cache for testing purposes.
pub fn build_test_cache_with_cap(shards: usize, capacity: u64) -> Cache<i32, String, ShardControllingHasher> {
  CacheBuilder::new()
    .shards(shards)
    .capacity(capacity)
    .hasher(ShardControllingHasher)
    .build()
    .unwrap()
}

pub fn build_test_async_cache(shards: usize) -> AsyncCache<i32, String, ShardControllingHasher> {
  CacheBuilder::new()
    .shards(shards)
    .hasher(ShardControllingHasher)
    .build_async()
    .unwrap()
}