use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy};
use std::{thread, time::Duration};

const JANITOR_TICK: Duration = Duration::from_millis(50);
const JANITOR_WAIT_MULTIPLIER: u32 = 4; // How many tick intervals to wait

#[test]
fn test_sync_janitor_evicts_on_capacity() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  // 1. Fill the cache up to its capacity + 1.
  // The insert calls will succeed and the cache will be temporarily over capacity.
  for i in 0..11 {
    cache.insert(i, i, 1);
  }
  assert_eq!(cache.metrics().current_cost, 11);

  // 2. Wait for the janitor to run and perform eviction.
  thread::sleep(JANITOR_TICK * 3);

  // 3. Assert the cache is back to its target capacity.
  assert_eq!(cache.metrics().current_cost, 10);
  assert!(
    cache.get(&0).is_none(),
    "Key 0 should have been evicted by the janitor"
  );
  assert!(cache.get(&10).is_some());
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[test]
fn test_sync_insert_is_non_blocking_and_janitor_cleans_up() {
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(5)
    .cache_policy(LruPolicy::new()) // Use a simple policy for predictable eviction
    .janitor_tick_interval(JANITOR_TICK)
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
  assert!(cache.get(&1).is_some());
  assert!(cache.get(&2).is_some());

  // 4. Wait for the janitor to run.
  // With LRU policy, it will evict item 1 (cost 5), then item 2 (cost 10)
  // to get the cost (0) back under the capacity (5).
  thread::sleep(JANITOR_TICK * 3);

  // 5. Assert the final state.
  assert_eq!(
    cache.metrics().current_cost,
    0,
    "Janitor should have evicted both items"
  );
  assert!(cache.get(&1).is_none(), "Item 1 should have been evicted");
  assert!(cache.get(&2).is_none(), "Item 2 should have been evicted");
  assert_eq!(cache.metrics().evicted_by_capacity, 2);
}

#[test]
fn test_sync_janitor_evicts_on_capacity_with_lru() { // Renamed slightly for clarity if we add more LRU tests
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(10)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  // 1. Fill the cache up to its capacity + 1.
  for i in 0..=10 { // Insert 0 through 10 (11 items)
    cache.insert(i, i, 1);
  }
  assert_eq!(cache.metrics().current_cost, 11, "Cost should be 11 after inserting 11 items");

  // 2. Wait for the janitor to run and perform eviction.
  thread::sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER);

  // 3. Assert the cache is back to its target capacity.
  assert_eq!(cache.metrics().current_cost, 10, "Cost should be 10 (capacity) after janitor");
  assert!(
    cache.get(&0).is_none(), // With LRU, item 0 (first in) should be evicted
    "Key 0 should have been evicted by the janitor"
  );
  for i in 1..=10 { // Items 1 through 10 should remain
      assert!(cache.get(&i).is_some(), "Key {} should still be present", i);
  }
  assert_eq!(cache.metrics().evicted_by_capacity, 1);
}

#[test]
fn test_sync_janitor_evicts_on_capacity_with_default_tinylfu() {
  let cache_capacity = 3;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    // No explicit policy, should default to TinyLfu
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  // 1. Insert items to fill the cache exactly to capacity.
  cache.insert(1, 10, 1);
  cache.insert(2, 20, 1);
  cache.insert(3, 30, 1);
  assert_eq!(cache.metrics().current_cost, cache_capacity, "Cache should be at capacity before overflow");

  // To make eviction predictable, let's access items 2 and 3 to make them "hotter"
  // than item 1 in the eviction policy's view.
  cache.get(&2);
  cache.get(&3);

  // 2. Cause an overflow by replacing an item with a higher-cost one.
  // This is a more reliable way to create an overflow state for the janitor with TinyLFU.
  // current_cost will become 3 (initial) - 1 (old cost of item 1) + 2 (new cost of item 1) = 4.
  cache.insert(1, 11, 2);
  
  // 3. Assert the cache is now over capacity. This should now pass.
  assert_eq!(cache.metrics().current_cost, cache_capacity + 1, "Cache should be over capacity for janitor");
  let evictions_before_janitor = cache.metrics().evicted_by_capacity;

  // 4. Wait for janitor to run.
  // Janitor sees cost 4 > capacity 3. It needs to free at least 1 cost.
  // The policy will evict the least valuable item. In our setup, item 2 is a likely candidate
  // as it was inserted before 3 and has a low cost.
  thread::sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER);

  // 5. Assert current_cost is back to capacity. This is the assertion that was failing.
  // The janitor should have run, evicted one item of cost 1, bringing the total cost from 4 down to 3.
  assert_eq!(cache.metrics().current_cost, cache_capacity, "Janitor should bring cost back to capacity");
  
  let evictions_after_janitor = cache.metrics().evicted_by_capacity;
  assert_eq!(evictions_after_janitor - evictions_before_janitor, 1, "Janitor should evict exactly one item");
  
  // 6. Assert item consistency (which specific item is evicted depends on fine-grained policy details,
  // but we can confirm that *one* of the cost-1 items is gone).
  let item1_present = cache.get(&1).is_some(); // cost 2
  let item2_present = cache.get(&2).is_some(); // cost 1
  let item3_present = cache.get(&3).is_some(); // cost 1
  
  assert!(item1_present, "Item 1 (high cost) should remain");
  // One of item 2 or 3 should be evicted.
  assert_ne!(item2_present, item3_present, "Exactly one of item 2 or 3 should have been evicted");
}


#[test]
fn test_sync_no_eviction_if_at_capacity() {
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .cache_policy(LruPolicy::new())
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  // 1. Fill the cache exactly to its capacity.
  for i in 0..cache_capacity {
    cache.insert(i as i32, i as i32, 1);
  }
  assert_eq!(cache.metrics().current_cost, cache_capacity, "Cost should be at capacity");
  let initial_evictions = cache.metrics().evicted_by_capacity;

  // 2. Wait for the janitor to run.
  thread::sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER);

  // 3. Assert that the cost is still at capacity and no new evictions occurred.
  assert_eq!(cache.metrics().current_cost, cache_capacity, "Cost should remain at capacity");
  assert_eq!(cache.metrics().evicted_by_capacity, initial_evictions, "No new evictions should occur");
  for i in 0..cache_capacity { // All items should still be present
      assert!(cache.get(&(i as i32)).is_some(), "Key {} should still be present", i);
  }
}


// This is the existing test, kept for reference for its scenario
#[test]
fn test_sync_insert_is_non_blocking_and_janitor_cleans_up_large_overflow() { // Renamed slightly
  let cache_capacity = 5;
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(cache_capacity)
    .cache_policy(LruPolicy::new()) // Use a simple policy for predictable eviction
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  // 1. Insert one item.
  cache.insert(1, 1, 1); // Item 1, cost 1. Current cost 1.
  assert_eq!(cache.metrics().current_cost, 1);

  // 2. Insert a large item that significantly overflows the cache.
  // Capacity=5. CurrentCost=1. Room=4.
  // New item cost = 10. Overflow by 10-4 = 6.
  // Total cost will be 1 (item1) + 10 (item2) = 11.
  cache.insert(2, 2, 10);

  // 3. Assert that the cache is now temporarily over its capacity.
  assert_eq!(
    cache.metrics().current_cost,
    11, // 1 (for key 1) + 10 (for key 2)
    "Cache should be temporarily over capacity"
  );
  assert!(cache.get(&1).is_some());
  assert!(cache.get(&2).is_some());
  let evictions_before_janitor = cache.metrics().evicted_by_capacity;


  // 4. Wait for the janitor to run.
  // current_cost = 11, capacity = 5. cost_to_free = 6.
  // LRU policy will be asked to evict 6.
  // It will evict item 1 (cost 1). Remaining to free: 5.
  // It will then evict item 2 (cost 10). Total freed 11.
  // Cache will be empty.
  thread::sleep(JANITOR_TICK * JANITOR_WAIT_MULTIPLIER);

  // 5. Assert the final state.
  assert_eq!(
    cache.metrics().current_cost,
    0, // Both items evicted to get under capacity 5.
    "Janitor should have evicted both items"
  );
  assert!(cache.get(&1).is_none(), "Item 1 should have been evicted");
  assert!(cache.get(&2).is_none(), "Item 2 should have been evicted");
  let evictions_after_janitor = cache.metrics().evicted_by_capacity;
  assert_eq!(evictions_after_janitor - evictions_before_janitor, 2, "Two items should be evicted by janitor");
}