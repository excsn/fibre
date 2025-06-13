use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy};
use tokio::time::{sleep, Duration};

const JANITOR_TICK: Duration = Duration::from_millis(50);
const JANITOR_WAIT_MULTIPLIER: u32 = 4; // How many tick intervals to wait

#[tokio::test]
async fn test_async_janitor_evicts_on_capacity() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .shards(1) 
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  // 1. Fill the cache up to its capacity + 1.
  for i in 0..11 {
    cache.insert(i, i, 1).await;
  }
  assert_eq!(cache.metrics().current_cost, 11);

  // 2. Wait for the janitor to run.
  sleep(JANITOR_TICK * 4).await;

  // 3. Assert the cache is back to its target capacity.
  assert_eq!(cache.metrics().current_cost, 10);
  assert!(
    cache.get(&0).is_none(),
    "Key 0 should have been evicted by the janitor"
  );
  assert!(cache.get(&10).is_some());
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_insert_is_non_blocking_and_janitor_cleans_up() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(5)
    .shards(1)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  // 1. Fill the cache up to its capacity.
  cache.insert(1, 1, 5).await;
  assert_eq!(cache.metrics().current_cost, 5);

  // 2. This insert needs 10 cost. It should complete immediately
  // and push the cache over capacity.
  cache.insert(2, 2, 10).await;

  // 3. Assert that the cache is now temporarily over its capacity.
  assert_eq!(
    cache.metrics().current_cost,
    15,
    "Cache should be temporarily over capacity"
  );
  assert!(cache.get(&1).is_some(), "Item 1 should still be present");
  assert!(
    cache.get(&2).is_some(),
    "Item 2 should have been inserted immediately"
  );

  // 4. Wait for the janitor to run and evict both items.
  sleep(JANITOR_TICK * 4).await;

  // 5. Assert the final state.
  assert_eq!(
    cache.metrics().current_cost,
    0,
    "Janitor should have evicted both items to get under capacity"
  );
  assert!(cache.get(&1).is_none(), "Item 1 should have been evicted");
  assert!(cache.get(&2).is_none(), "Item 2 should have been evicted");
  assert_eq!(cache.metrics().evicted_by_capacity, 2);
}

// --- NEW TESTS ADDED FOR PARITY WITH SYNC ---

#[tokio::test]
async fn test_async_janitor_evicts_on_capacity_with_lru() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .shards(1)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  for i in 0..=10 {
    cache.insert(i, i, 1).await;
  }
  assert_eq!(
    cache.metrics().current_cost,
    11,
    "Cost should be 11 after inserting 11 items"
  );

  sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER).await;

  assert_eq!(
    cache.metrics().current_cost,
    10,
    "Cost should be 10 (capacity) after janitor"
  );
  assert!(cache.get(&0).is_none(), "Key 0 should have been evicted");
  for i in 1..=10 {
    assert!(cache.get(&i).is_some(), "Key {} should still be present", i);
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
    .build_async()
    .unwrap();

  cache.insert(1, 10, 1).await;
  cache.insert(2, 20, 1).await;
  cache.insert(3, 30, 1).await;
  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Cache should be at capacity before overflow"
  );

  cache.get(&2);
  cache.get(&3);

  cache.insert(1, 11, 2).await;

  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity + 1,
    "Cache should be over capacity for janitor"
  );
  let evictions_before_janitor = cache.metrics().evicted_by_capacity;

  sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER).await;

  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Janitor should bring cost back to capacity"
  );

  let evictions_after_janitor = cache.metrics().evicted_by_capacity;
  assert_eq!(
    evictions_after_janitor - evictions_before_janitor,
    1,
    "Janitor should evict exactly one item"
  );

  let item1_present = cache.get(&1).is_some();
  let item2_present = cache.get(&2).is_some();
  let item3_present = cache.get(&3).is_some();

  assert!(item1_present, "Item 1 (high cost) should remain");
  assert_ne!(
    item2_present, item3_present,
    "Exactly one of item 2 or 3 should have been evicted"
  );
}

#[tokio::test]
async fn test_async_no_eviction_if_at_capacity() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  for i in 0..cache_capacity {
    cache.insert(i as i32, i as i32, 1).await;
  }
  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Cost should be at capacity"
  );
  let initial_evictions = cache.metrics().evicted_by_capacity;

  sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER).await;

  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Cost should remain at capacity"
  );
  assert_eq!(
    cache.metrics().evicted_by_capacity,
    initial_evictions,
    "No new evictions should occur"
  );
  for i in 0..cache_capacity {
    assert!(
      cache.get(&(i as i32)).is_some(),
      "Key {} should still be present",
      i
    );
  }
}

#[tokio::test]
async fn test_async_janitor_cleans_up_large_overflow() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .shards(1)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  cache.insert(1, 1, 1).await;
  assert_eq!(cache.metrics().current_cost, 1);

  cache.insert(2, 2, 10).await;

  assert_eq!(
    cache.metrics().current_cost,
    11,
    "Cache should be temporarily over capacity"
  );
  assert!(cache.get(&1).is_some());
  assert!(cache.get(&2).is_some());
  let evictions_before_janitor = cache.metrics().evicted_by_capacity;

  sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER).await;

  assert_eq!(
    cache.metrics().current_cost,
    0,
    "Janitor should have evicted both items"
  );
  assert!(cache.get(&1).is_none(), "Item 1 should have been evicted");
  assert!(cache.get(&2).is_none(), "Item 2 should have been evicted");
  let evictions_after_janitor = cache.metrics().evicted_by_capacity;
  assert_eq!(
    evictions_after_janitor - evictions_before_janitor,
    2,
    "Two items should be evicted by janitor"
  );
}
