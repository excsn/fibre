use fibre_cache::{AsyncCache, builder::CacheBuilder, policy::lru::LruPolicy};
use std::{sync::Arc, time::Duration};

fn new_test_cache(capacity: u64) -> AsyncCache<i32, String> {
  CacheBuilder::new()
    .capacity(capacity)
    .shards(4) // Use multiple shards to test cross-shard logic
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(Duration::from_millis(50))
    .build_async()
    .unwrap()
}

#[tokio::test]
#[cfg(feature = "bulk")]
async fn test_async_multi_insert_and_multiget() {
  let cache = new_test_cache(100);
  let items: Vec<_> = (0..20).map(|i| (i, i.to_string(), 1)).collect();

  // 1. Insert 20 items in bulk.
  cache.multi_insert(items).await;

  // 2. Verify metrics and cost.
  let metrics = cache.metrics();
  assert_eq!(metrics.current_cost, 20);

  // 3. Get all 20 items back.
  let keys_to_get: Vec<_> = (0..20).collect();
  let found = cache.multiget(keys_to_get.clone()).await;

  assert_eq!(found.len(), 20, "Should find all inserted items");
  for i in 0..20 {
    assert_eq!(*found.get(&i).unwrap(), Arc::new(i.to_string()));
  }
  assert_eq!(cache.metrics().hits, 20);
  assert_eq!(cache.metrics().misses, 0);

  // 4. Get a mix of existing and non-existent keys.
  let mixed_keys: Vec<_> = (10..30).collect();
  let found_mixed = cache.multiget(mixed_keys).await;
  assert_eq!(
    found_mixed.len(),
    10,
    "Should only find the 10 existing keys"
  );
  assert_eq!(cache.metrics().hits, 30); // 20 previous + 10 new
  assert_eq!(cache.metrics().misses, 10); // 10 new misses
}

#[tokio::test]
#[cfg(feature = "bulk")]
async fn test_async_multi_invalidate() {
  let cache = new_test_cache(100);
  let items: Vec<_> = (0..20).map(|i| (i, i.to_string(), 1)).collect();
  cache.multi_insert(items).await;
  assert_eq!(cache.metrics().current_cost, 20);

  // 1. Invalidate a subset of keys.
  let keys_to_invalidate: Vec<_> = (5..15).collect();
  cache.multi_invalidate(keys_to_invalidate).await;

  // 2. Verify cost and metrics.
  let metrics = cache.metrics();
  assert_eq!(metrics.current_cost, 10, "Cost should be reduced by 10");
  assert_eq!(metrics.invalidations, 10);

  // 3. Verify items are gone.
  for i in 5..15 {
    assert!(cache.fetch(&i).await.is_none());
  }
  for i in (0..5).chain(15..20) {
    assert!(cache.fetch(&i).await.is_some());
  }
}

#[tokio::test]
#[cfg(feature = "bulk")]
async fn test_async_multi_insert_triggers_eviction() {
  use futures_util::StreamExt;
use tokio::time;

  let cache = new_test_cache(10);

  // Insert 15 items into a cache with capacity 10.
  let items: Vec<_> = (0..15).map(|i| (i, i.to_string(), 1)).collect();
  cache.multi_insert(items).await;

  // The cache is temporarily over capacity.
  assert_eq!(cache.metrics().current_cost, 15);

  let deadline = time::Instant::now() + Duration::from_secs(2);
  let mut final_cost = 0;

  loop {
    final_cost = cache.metrics().current_cost;
    if final_cost <= 10 {
      break; // Success! Cost is at or below capacity.
    }
    if time::Instant::now() > deadline {
      panic!(
        "Cache cost did not converge to capacity in time. Final cost: {}",
        final_cost
      );
    }
    time::sleep(Duration::from_millis(50)).await;
  }

  assert!(
    final_cost <= 10,
    "Janitor should bring cost to at or below capacity. Final cost: {}",
    final_cost
  );

  // The eviction count is harder to assert precisely, as it depends on the overshoot.
  // A better assertion is on the number of items remaining.
  assert!(
    cache.metrics().evicted_by_capacity >= 5,
    "Janitor should evict at least 5 items"
  );

  let mut total_keys = 0;
  let mut stream = cache.iter_stream();
  while let Some(_) = stream.next().await {
    total_keys += 1;
  }

  assert_eq!(
    total_keys as u64, final_cost,
    "The number of items in the cache should equal the final cost"
  );
}
