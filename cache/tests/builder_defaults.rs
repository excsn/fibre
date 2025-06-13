use fibre_cache::CacheBuilder;
use std::time::Duration;

#[test]
fn test_unbounded_cache_with_ttl_does_not_panic() {
  // This test reproduces the overflow panic.
  // By NOT setting .capacity(), we get the default u64::MAX.
  // The builder should be smart enough to select NullPolicy instead of
  // TinyLfu, which would panic with such a large capacity.
  let cache = CacheBuilder::<i32, i32>::new()
    .time_to_live(Duration::from_secs(1))
    .build();

  // The test passes if the build() call above does not panic.
  // We can add a simple operation to be sure.
  assert!(cache.is_ok());
  let cache = cache.unwrap();
  cache.insert(1, 1, 1);
  assert!(cache.get(&1).is_some());
}

#[tokio::test]
async fn test_unbounded_async_cache_with_tti_does_not_panic() {
  let cache = CacheBuilder::<i32, i32>::new()
    .time_to_idle(Duration::from_secs(1))
    .build_async();

  assert!(cache.is_ok());
  let cache = cache.unwrap();
  cache.insert(1, 1, 1).await;
  assert!(cache.get(&1).is_some());
}

#[test]
fn test_bounded_cache_defaults_to_tinylfu() {
  // This test verifies that the default policy for a bounded cache is TinyLFU
  // by checking its core behavior: protecting frequently accessed items.

  // With a small capacity, the window/main cache interaction is triggered quickly.
  // The janitor needs a fast tick to clean up for the test assertions.
  let cache = CacheBuilder::<i32, i32>::new()
    .capacity(2)
    .janitor_tick_interval(std::time::Duration::from_millis(50))
    .build()
    .unwrap();

  // Insert two items and access them to make them "valuable" (frequency > 1)
  cache.insert(1, 1, 1);
  cache.get(&1);
  cache.insert(2, 2, 1);
  cache.get(&2);

  // At this point, cost is 2. The cache is full.
  assert_eq!(cache.metrics().current_cost, 2);

  // Insert a third item.
  // With TinyLfu (capacity 2 -> window_target=1, main_slru_target=1):
  // - Item 3 (cost 1) goes to window. Window: {(3,c1), (2,c1)}. Cost=2. Target=1. Overflow.
  // - Candidate from window: (2,c1). Freq(2)=2.
  // - Main SLRU has (1,c1). Freq(1)=2.
  // - Candidate 2 is admitted to main. Main SLRU becomes {(2,c1),(1,c1)}. Cost=2. Target=1. Overflow.
  // - Main SLRU evicts (1,c1). Victim for on_admit is [1].
  // Cache store: adds 3 (cost +1), removes 1 (cost -1). Net cost change 0.
  // current_cost was 2. After insert(3) and its victim processing, current_cost should still be 2.
  cache.insert(3, 3, 1);
  assert_eq!(
    cache.metrics().current_cost,
    2, // CHANGED FROM 3 to 2
    "Cost after insert(3) and its policy-driven eviction"
  );

  // Wait for the janitor to run and perform eviction.
  std::thread::sleep(std::time::Duration::from_millis(150));

  // The janitor should see current_cost (2) == capacity (2). No further eviction needed.
  assert_eq!(
    cache.metrics().current_cost,
    2,
    "Janitor should not change cost as it's at capacity"
  );
  // The eviction of item 1 happened due to policy decision during insert(3), not by janitor for general overflow.
  // Depending on how EvictionReason is tracked or if a new metric for policy-evictions is added, this might change.
  // For now, let's assume it still counts towards evicted_by_capacity by the shared.process_evicted_victims.
  assert_eq!(cache.metrics().evicted_by_capacity, 1);

  // *** VERIFY THE EVICTION ***
  // Store after insert(3) and its victims: original {1,2} -> add 3 -> remove 1 -> final {2,3}
  assert!(
    cache.get(&1).is_none(), // Item 1 was evicted by policy during insert(3)
    "Item 1 was policy-evicted"
  );
  assert!(
    cache.get(&2).is_some(),
    "Item 2 was admitted to main SLRU and should be present"
  );
  assert!(
    cache.get(&3).is_some(),
    "Item 3 is newest and should be in the window"
  );
}
