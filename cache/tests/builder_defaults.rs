use fibre_cache::CacheBuilder;
use std::time::Duration;

#[test]
fn test_unbounded_cache_with_ttl_does_not_panic() {
  // This test reproduces the overflow panic.
  let cache = CacheBuilder::<i32, i32>::new()
    .time_to_live(Duration::from_secs(1))
    .build();

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
    // Set a long tick so the background thread doesn't race with
    // our foreground opportunistic maintenance over-evicting items.
    .janitor_tick_interval(std::time::Duration::from_secs(10))
    .maintenance_chance(1)
    .build()
    .unwrap();

  cache.insert(1, 1, 1);
  cache.insert(2, 2, 1);

  for _ in 0..10 {
    cache.fetch(&1);
  }
  assert_eq!(cache.metrics().current_cost, 2);

  // Allow time for the read batcher to be ready for processing
  std::thread::sleep(std::time::Duration::from_millis(10));

  // Trigger opportunistic maintenance to flush the read batcher
  cache.insert(1, 1, 1);

  // Insert a third item. The opportunistic maintenance will run immediately,
  // reject item 2, and remove it natively without the Janitor's help.
  cache.insert(3, 3, 1);

  // Give the channels a brief moment to settle
  std::thread::sleep(std::time::Duration::from_millis(10));

  assert_eq!(
    cache.metrics().current_cost,
    2,
    "Opportunistic maintenance should hold cost at capacity"
  );
  assert!(cache.fetch(&1).is_some(), "Hot item 1 should be protected");
  assert!(cache.fetch(&2).is_none(), "Cold item 2 should be evicted");
  assert!(cache.fetch(&3).is_some(), "New item 3 should be present");
}
