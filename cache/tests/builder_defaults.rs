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
  assert!(cache.fetch(&1).is_some());
}

#[tokio::test]
async fn test_unbounded_async_cache_with_tti_does_not_panic() {
  let cache = CacheBuilder::<i32, i32>::new()
    .shards(1)
    .time_to_idle(Duration::from_secs(1))
    .build_async();

  assert!(cache.is_ok());
  let cache = cache.unwrap();
  cache.insert(1, 1, 1).await;
  assert!(cache.fetch(&1).await.is_some());
}

#[test]
fn test_bounded_cache_defaults_to_tinylfu() {
  let cache = CacheBuilder::<i32, i32>::new()
    .shards(1)
    .capacity(2)
    .janitor_tick_interval(std::time::Duration::from_millis(50))
    .maintenance_chance(1)
    .build()
    .unwrap();

  // Insert two items.
  cache.insert(1, 1, 1);
  cache.insert(2, 2, 1);

  // Access item 1 repeatedly to make it "hot". Item 2 remains "cold".
  for _ in 0..10 {
    cache.fetch(&1);
  }

  // At this point, cost is 2. The cache is full.
  assert_eq!(cache.metrics().current_cost, 2);

  // By performing a dummy insert on the "hot" key, we trigger the now-100%-guaranteed
  // opportunistic maintenance. This maintenance task will drain the batcher of
  // read events, making the policy aware that item 1 is "hot" BEFORE we
  // proceed to the next step.
  cache.insert(1, 1, 1); // This is effectively a "flush" operation now.

  // Insert a third item. This will go into the policy's write buffer.
  cache.insert(3, 3, 1);

  // Wait for the janitor to run. It will process the write for item 3.
  // The TinyLFU policy should decide to evict the "cold" item 2, not the "hot" item 1.
  std::thread::sleep(std::time::Duration::from_millis(150));

  // Assert that the cost is back to capacity and the correct item was evicted.
  assert_eq!(
    cache.metrics().current_cost,
    2,
    "Janitor should bring cost back to capacity"
  );
  assert!(cache.fetch(&1).is_some(), "Hot item 1 should be protected");
  assert!(cache.fetch(&2).is_none(), "Cold item 2 should be evicted");
  assert!(cache.fetch(&3).is_some(), "New item 3 should be present");
}
