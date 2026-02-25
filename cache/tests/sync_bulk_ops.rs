use fibre_cache::{Cache, builder::CacheBuilder, policy::lru::LruPolicy};
use std::{sync::Arc, thread, time::Duration};

fn new_test_cache(capacity: u64) -> Cache<i32, String> {
  CacheBuilder::new()
    .capacity(capacity)
    .shards(4) // Use multiple shards to test cross-shard logic
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(Duration::from_millis(50))
    .maintenance_chance(1)
    .build()
    .unwrap()
}

#[test]
#[cfg(feature = "bulk")]
fn test_sync_multi_insert_and_multiget() {
  let cache = new_test_cache(100);
  let items: Vec<_> = (0..20).map(|i| (i, i.to_string(), 1)).collect();

  // 1. Insert 20 items in bulk.
  cache.multi_insert(items);

  // 2. Verify metrics and cost.
  let metrics = cache.metrics();
  assert_eq!(metrics.current_cost, 20);

  // 3. Get all 20 items back.
  let keys_to_get: Vec<_> = (0..20).collect();
  let found = cache.multiget(keys_to_get.clone());

  assert_eq!(found.len(), 20, "Should find all inserted items");
  for i in 0..20 {
    assert_eq!(*found.get(&i).unwrap(), Arc::new(i.to_string()));
  }
  assert_eq!(cache.metrics().hits, 20);
  assert_eq!(cache.metrics().misses, 0);

  // 4. Get a mix of existing and non-existent keys.
  let mixed_keys: Vec<_> = (10..30).collect();
  let found_mixed = cache.multiget(mixed_keys);
  assert_eq!(
    found_mixed.len(),
    10,
    "Should only find the 10 existing keys"
  );
  assert_eq!(cache.metrics().hits, 30); // 20 previous + 10 new
  assert_eq!(cache.metrics().misses, 10); // 10 new misses
}

#[test]
#[cfg(feature = "bulk")]
fn test_sync_multi_invalidate() {
  let cache = new_test_cache(100);
  let items: Vec<_> = (0..20).map(|i| (i, i.to_string(), 1)).collect();
  cache.multi_insert(items);
  assert_eq!(cache.metrics().current_cost, 20);

  // 1. Invalidate a subset of keys.
  let keys_to_invalidate: Vec<_> = (5..15).collect();
  cache.multi_invalidate(keys_to_invalidate);

  // 2. Verify cost and metrics.
  let metrics = cache.metrics();
  assert_eq!(metrics.current_cost, 10, "Cost should be reduced by 10");
  assert_eq!(metrics.invalidations, 10);

  // 3. Verify items are gone.
  for i in 5..15 {
    assert!(cache.fetch(&i).is_none());
  }
  for i in (0..5).chain(15..20) {
    assert!(cache.fetch(&i).is_some());
  }
}

#[test]
#[cfg(feature = "bulk")]
fn test_sync_multi_insert_triggers_eviction() {
  let cache = new_test_cache(10);

  // Insert 15 items into a cache with capacity 10.
  let items: Vec<_> = (0..15).map(|i| (i, i.to_string(), 1)).collect();
  cache.multi_insert(items);

  // The cache is temporarily over capacity.
  assert_eq!(cache.metrics().current_cost, 15);

  // Use a polling loop to wait for the janitor to converge.
  let deadline = std::time::Instant::now() + Duration::from_secs(2);
  let mut final_cost = 0;
  loop {
    final_cost = cache.metrics().current_cost;
    if final_cost <= 10 {
      break; // Success! Cost is at or below capacity.
    }
    if std::time::Instant::now() > deadline {
      panic!(
        "Cache cost did not converge to capacity in time. Final cost: {}",
        final_cost
      );
    }
    thread::sleep(Duration::from_millis(50));
  }

  assert!(
    final_cost <= 10,
    "Janitor should bring cost to at or below capacity. Final cost: {}",
    final_cost
  );

  // Because proportional cross-shard eviction uses ceiling math,
  // it might occasionally evict slightly more than exactly 5 items.
  assert!(
    cache.metrics().evicted_by_capacity >= 5,
    "Janitor should evict at least 5 items"
  );

  let mut total_keys = 0;
  for i in 0..15 {
    if cache.fetch(&i).is_some() {
      total_keys += 1;
    }
  }

  assert_eq!(
    total_keys as u64, final_cost,
    "The number of items in the cache should equal the final cost"
  );
}
