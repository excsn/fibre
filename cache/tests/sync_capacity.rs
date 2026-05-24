use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy};
use std::time::Duration;

const JANITOR_TICK: Duration = Duration::from_millis(50);

#[test]
fn test_sync_janitor_evicts_on_capacity() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Fill the cache up to its capacity + 1.
  // The insert calls will succeed and the cache will be temporarily over capacity.
  for i in 0..11 {
    cache.insert(i, i, 1);
  }
  assert_eq!(cache.metrics().current_cost, 11);

  // 2. Force a maintenance pass to perform eviction.
  cache.run_maintenance();

  // 3. Assert the cache is back to its target capacity.
  assert_eq!(cache.metrics().current_cost, 10);
  assert!(
    cache.fetch(&0).is_none(),
    "Key 0 should have been evicted"
  );
  assert!(cache.fetch(&10).is_some());
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[test]
fn test_sync_insert_is_non_blocking_and_janitor_cleans_up() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(5)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new())) // Use a simple policy for predictable eviction
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Fill the cache up to its capacity.
  cache.insert(1, 1, 5);
  assert_eq!(cache.metrics().current_cost, 5);

  // 2. This insert should complete immediately and push the cache over capacity.
  // There is no spawned thread needed as insert is non-blocking.
  cache.insert(2, 2, 10);

  // 3. Assert that the cache is now temporarily over its capacity.
  assert_eq!(
    cache.metrics().current_cost,
    15,
    "Cache should be temporarily over capacity"
  );
  assert!(cache.fetch(&1).is_some());
  assert!(cache.fetch(&2).is_some());

  // 4. Force maintenance to evict. LRU will evict item 1 (cost 5) then item 2 (cost 10).
  cache.run_maintenance();

  // 5. Assert the final state.
  let final_cost = cache.metrics().current_cost;
  assert!(
    final_cost <= 5,
    "run_maintenance should bring cost at or below capacity. Final cost: {}",
    final_cost
  );

  let final_evictions = cache.metrics().evicted_by_capacity;
  assert!(
    final_evictions > 0,
    "At least one item should have been evicted"
  );
}

#[test]
fn test_sync_janitor_evicts_on_capacity_with_lru() {
  // Renamed slightly for clarity if we add more LRU tests
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Fill the cache up to its capacity + 1.
  for i in 0..=10 {
    // Insert 0 through 10 (11 items)
    cache.insert(i, i, 1);
  }
  assert_eq!(
    cache.metrics().current_cost,
    11,
    "Cost should be 11 after inserting 11 items"
  );

  // 2. Force a maintenance pass to perform eviction.
  cache.run_maintenance();

  // 3. Assert the cache is back to its target capacity.
  assert_eq!(
    cache.metrics().current_cost,
    10,
    "Cost should be 10 (capacity) after maintenance"
  );
  assert!(
    cache.fetch(&0).is_none(), // With LRU, item 0 (first in) should be evicted
    "Key 0 should have been evicted"
  );
  for i in 1..=10 {
    // Items 1 through 10 should remain
    assert!(
      cache.fetch(&i).is_some(),
      "Key {} should still be present",
      i
    );
  }
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[test]
fn test_sync_janitor_evicts_on_capacity_with_default_tinylfu() {
  let cache_capacity = 3;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .shards(1)
    // No explicit policy, should default to TinyLfu
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Insert items to fill the cache exactly to capacity.
  cache.insert(1, 10, 1);
  cache.insert(2, 20, 1);
  cache.insert(3, 30, 1);
  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Cache should be at capacity before overflow"
  );

  // To make eviction predictable, let's access items 2 and 3 to make them "hotter"
  // than item 1 in the eviction policy's view.
  cache.fetch(&2);
  cache.fetch(&3);

  // 2. Cause an overflow by replacing an item with a higher-cost one.
  // This is a more reliable way to create an overflow state for the janitor with TinyLFU.
  // current_cost will become 3 (initial) - 1 (old cost of item 1) + 2 (new cost of item 1) = 4.
  cache.insert(1, 11, 2);

  let evictions_before = cache.metrics().evicted_by_capacity;
  cache.run_maintenance();

  // 4. Assert current_cost is back to capacity.
  // run_maintenance evicted one item of cost 1, bringing the total cost from 4 down to 3.
  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "run_maintenance should bring cost back to capacity"
  );

  let evictions_after = cache.metrics().evicted_by_capacity;
  assert_eq!(
    evictions_after - evictions_before,
    1,
    "Exactly one item should have been evicted"
  );

  // 5. Assert item consistency (which specific item is evicted depends on fine-grained policy details,
  // but we can confirm that *one* of the cost-1 items is gone).
  let item1_present = cache.fetch(&1).is_some(); // cost 2
  let item2_present = cache.fetch(&2).is_some(); // cost 1
  let item3_present = cache.fetch(&3).is_some(); // cost 1

  assert!(item1_present, "Item 1 (high cost) should remain");
  // One of item 2 or 3 should be evicted.
  assert_ne!(
    item2_present, item3_present,
    "Exactly one of item 2 or 3 should have been evicted"
  );
}

#[test]
fn test_sync_no_eviction_if_at_capacity() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .shards(1)
    .capacity(cache_capacity)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Fill the cache exactly to its capacity.
  for i in 0..cache_capacity {
    cache.insert(i as i32, i as i32, 1);
  }
  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "Cost should be at capacity"
  );
  let initial_evictions = cache.metrics().evicted_by_capacity;

  // 2. Force a maintenance pass (should find nothing to evict).
  cache.run_maintenance();

  // 3. Assert that the cost is still at capacity and no new evictions occurred.
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
    // All items should still be present
    assert!(
      cache.fetch(&(i as i32)).is_some(),
      "Key {} should still be present",
      i
    );
  }
}

#[test]
fn test_sync_insert_is_non_blocking_and_janitor_cleans_up_large_overflow() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new())) // Use a simple policy for predictable eviction
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  // 1. Insert one item.
  cache.insert(1, 1, 1); // Policy now knows about item 1.
  assert_eq!(cache.metrics().current_cost, 1);

  // 2. Insert a large item that significantly overflows the cache.
  // Because maintenance_chance is 1, this call also ensures the policy
  // is updated with item 2.
  cache.insert(2, 2, 10);

  // 3. Assert that the cache is now temporarily over its capacity.
  assert_eq!(
    cache.metrics().current_cost,
    11, // 1 (for key 1) + 10 (for key 2)
    "Cache should be temporarily over capacity"
  );
  assert!(cache.fetch(&1).is_some());
  assert!(cache.fetch(&2).is_some());
  let evictions_before = cache.metrics().evicted_by_capacity;

  // 4. Force a maintenance pass to perform eviction.
  cache.run_maintenance();

  // 5. Assert the final state.
  let final_cost = cache.metrics().current_cost;
  assert!(
    final_cost <= cache_capacity,
    "run_maintenance should bring cost at or below capacity. Final cost: {}",
    final_cost
  );

  let evictions_after = cache.metrics().evicted_by_capacity;
  assert!(
    evictions_after > evictions_before,
    "At least one item should have been evicted"
  );
}
