use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy};
use std::time::Duration;

const JANITOR_TICK: Duration = Duration::from_millis(50);

#[test]
fn test_sync_insert_evicts_on_capacity() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  for i in 0..11 {
    cache.insert(i, i, 1);
  }
  assert_eq!(cache.metrics().current_cost, 10, "the eleventh insert evicts before it returns");
  assert!(
    cache.fetch(&0).is_none(),
    "Key 0 should have been evicted"
  );
  assert!(cache.fetch(&10).is_some());
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[test]
fn test_sync_insert_evicts_what_it_cannot_fit() {
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

  // 2. An item of cost 10 cannot fit in a cache of capacity 5. LRU evicts
  // item 1 (cost 5) and then the new item itself (cost 10) before returning.
  cache.insert(2, 2, 10);

  assert_eq!(cache.metrics().current_cost, 0);
  assert!(cache.fetch(&1).is_none());
  assert!(cache.fetch(&2).is_none());
  assert_eq!(cache.metrics().evicted_by_capacity, 2);
}

#[test]
fn test_sync_insert_evicts_on_capacity_with_lru() {
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
    10,
    "Cost should be 10 (capacity) after inserting 11 items"
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
fn test_sync_insert_evicts_on_capacity_with_tinylfu() {
  let cache_capacity = 3;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .shards(1)
    .tinylfu()
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

  // 2. Replacing item 1 with a cost-2 version takes the cache to 4, so the
  // insert evicts one cost-1 item before returning.
  let evictions_before = cache.metrics().evicted_by_capacity;
  cache.insert(1, 11, 2);

  assert_eq!(
    cache.metrics().current_cost,
    cache_capacity,
    "the insert should bring cost back to capacity"
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
fn test_sync_insert_evicts_a_large_overflow() {
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

  // 2. An item of cost 10 overflows a capacity of 5 even alone, so the insert
  // evicts item 1 and then the new item before returning.
  let evictions_before = cache.metrics().evicted_by_capacity;
  cache.insert(2, 2, 10);

  let final_cost = cache.metrics().current_cost;
  assert!(
    final_cost <= cache_capacity,
    "the insert should leave cost at or below capacity. Final cost: {}",
    final_cost
  );

  let evictions_after = cache.metrics().evicted_by_capacity;
  assert!(
    evictions_after > evictions_before,
    "At least one item should have been evicted"
  );
}

#[test]
fn default_shards_hold_small_capacity_exactly() {
  let cache = CacheBuilder::<u64, u64>::new().capacity(100).build().unwrap();
  for i in 0..250 {
    cache.insert(i, i, 1);
  }
  cache.run_maintenance();
  assert_eq!(cache.iter().count(), 100);
  assert_eq!(cache.metrics().current_cost, 100);
}
