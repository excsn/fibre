use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy};
use tokio::time::{Duration, Instant, sleep};

const JANITOR_TICK: Duration = Duration::from_millis(50);
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(2);

// Helper function to wait for the cache cost to be at or below a target.
async fn wait_for_cost_convergence(cache: &fibre_cache::AsyncCache<i32, i32>, target_cost: u64) {
  let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
  loop {
    let current_cost = cache.metrics().current_cost;
    if current_cost <= target_cost {
      return; // Success
    }
    if Instant::now() > deadline {
      panic!(
        "Cache cost did not converge. Current: {}, Target: {}",
        current_cost, target_cost
      );
    }
    sleep(JANITOR_TICK).await;
  }
}

#[tokio::test]
async fn test_async_janitor_evicts_on_capacity() {
  let capacity = 10;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(capacity)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  for i in 0..11 {
    cache.insert(i, i, 1).await;
  }
  assert_eq!(cache.metrics().current_cost, 11);

  // Wait for the janitor to bring the cost down to capacity.
  wait_for_cost_convergence(&cache, capacity).await;

  // Assert the final state.
  assert_eq!(cache.metrics().current_cost, capacity);
  assert!(
    cache.fetch(&0).await.is_none(),
    "Key 0 should have been evicted"
  );
  assert!(cache.fetch(&10).await.is_some());
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_insert_is_non_blocking_and_janitor_cleans_up() {
  let capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(capacity)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  cache.insert(1, 1, 5).await;
  cache.insert(2, 2, 10).await;
  assert_eq!(cache.metrics().current_cost, 15);

  // Wait for the janitor. Since cost_to_free is 10 and the largest item is 10,
  // it will likely evict both to get under capacity. The final cost should be 0.
  wait_for_cost_convergence(&cache, 0).await;

  // Assert the final state.
  assert_eq!(cache.metrics().current_cost, 0);
  assert!(
    cache.fetch(&1).await.is_none(),
    "Item 1 should have been evicted"
  );
  assert!(
    cache.fetch(&2).await.is_none(),
    "Item 2 should have been evicted"
  );
  assert_eq!(cache.metrics().evicted_by_capacity, 2);
}

#[tokio::test]
async fn test_async_janitor_evicts_on_capacity_with_lru() {
  let capacity = 10;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(capacity)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  for i in 0..=10 {
    cache.insert(i, i, 1).await;
  }
  assert_eq!(cache.metrics().current_cost, 11);

  wait_for_cost_convergence(&cache, capacity).await;

  assert_eq!(cache.metrics().current_cost, capacity);
  assert!(
    cache.fetch(&0).await.is_none(),
    "Key 0 should have been evicted"
  );
  for i in 1..=10 {
    assert!(
      cache.fetch(&i).await.is_some(),
      "Key {} should still be present",
      i
    );
  }
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[tokio::test]
async fn test_async_janitor_evicts_on_capacity_with_default_tinylfu() {
  let cache_capacity = 3;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .shards(1)
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  cache.insert(1, 10, 1).await;
  cache.insert(2, 20, 1).await;
  cache.insert(3, 30, 1).await;

  // Give the janitor a moment to process initial writes.
  sleep(JANITOR_TICK * 2).await;
  assert_eq!(cache.metrics().current_cost, cache_capacity);

  // Make items 2 and 3 "hotter" than item 1.
  cache.fetch(&2).await;
  cache.fetch(&3).await;

  // Replace item 1 with a higher-cost version, overflowing the cache.
  cache.insert(1, 11, 2).await; // Total cost is now 2 (item 1) + 1 (item 2) + 1 (item 3) = 4.
  assert_eq!(cache.metrics().current_cost, cache_capacity + 1);

  // Give the janitor one more tick to process the latest write event
  // before we start checking for convergence on cost.
  sleep(JANITOR_TICK * 2).await;

  // Wait for the janitor to bring the cost down. It needs to free 1.
  // The policy should evict the least valuable item. Based on access patterns,
  // the coldest item is 1. But based on SLRU mechanics, the victim from protected might be 2.
  // Let's not assume which one and just check the final state.
  wait_for_cost_convergence(&cache, cache_capacity).await;

  let final_cost = cache.metrics().current_cost;
  assert!(
    final_cost <= cache_capacity,
    "Final cost should be at or below capacity"
  );

  let snapshot_cache = cache.to_snapshot().await;

  println!("snapshot {:?}", snapshot_cache);
  let item1 = cache.fetch(&1).await;
  let item2 = cache.fetch(&2).await;
  let item3 = cache.fetch(&3).await;

  let mut calculated_cost = 0;
  if item1.is_some() {
    calculated_cost += 2;
  } // Item 1 has cost 2
  if item2.is_some() {
    calculated_cost += 1;
  } // Item 2 has cost 1
  if item3.is_some() {
    calculated_cost += 1;
  } // Item 3 has cost 1

  assert_eq!(
    final_cost, calculated_cost,
    "The reported final_cost metric should match the sum of costs of items actually in the cache"
  );

  // Based on our trace, we expect item 2 to be evicted.
  assert!(
    item1.is_some(),
    "Item 1 (high cost, recently inserted) should remain"
  );
  assert!(item3.is_some(), "Item 3 (hot) should remain");
  assert!(
    item2.is_none(),
    "Item 2 (LRU of protected) should be the victim"
  );
  assert_eq!(final_cost, 3);
}

#[tokio::test]
async fn test_async_no_eviction_if_at_capacity() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  for i in 0..cache_capacity {
    cache.insert(i as i32, i as i32, 1).await;
  }
  let initial_evictions = cache.metrics().evicted_by_capacity;

  // Wait for a few janitor ticks.
  sleep(JANITOR_TICK * 4).await;

  // No convergence loop needed, as we expect no change.
  assert_eq!(cache.metrics().current_cost, cache_capacity);
  assert_eq!(cache.metrics().evicted_by_capacity, initial_evictions);
  for i in 0..cache_capacity {
    assert!(cache.fetch(&(i as i32)).await.is_some());
  }
}

#[tokio::test]
async fn test_async_janitor_cleans_up_large_overflow() {
  let capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(capacity)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1) 
    .build_async()
    .unwrap();

  cache.insert(1, 1, 1).await;
  cache.insert(2, 2, 10).await;
  assert_eq!(cache.metrics().current_cost, 11);

  let evictions_before_janitor = cache.metrics().evicted_by_capacity;

  wait_for_cost_convergence(&cache, capacity).await;

  let evictions_after_janitor = cache.metrics().evicted_by_capacity;
  assert_eq!(evictions_after_janitor - evictions_before_janitor, 2);
  assert!(cache.metrics().current_cost <= capacity);
  assert!(cache.fetch(&1).await.is_none());
  assert!(cache.fetch(&2).await.is_none());
}
