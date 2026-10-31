use fibre_cache::CacheBuilder;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_millis(20);
const KEYS: u64 = 1000;

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
  let start = Instant::now();
  while start.elapsed() < limit {
    if done() {
      return true;
    }
    std::thread::sleep(Duration::from_millis(10));
  }
  done()
}

#[test]
fn test_sync_insert_with_ttl_is_removed_from_an_unbounded_cache_with_no_builder_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build().unwrap();
  for k in 0..KEYS {
    cache.insert_with_ttl(k, k, 1, TTL);
  }
  assert!(wait_for(Duration::from_secs(4), || cache.metrics().evicted_by_ttl == KEYS), "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[test]
fn test_sync_insert_with_ttl_is_removed_from_a_bounded_cache_with_no_builder_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().capacity(100_000).janitor_tick_interval(TICK).build().unwrap();
  for k in 0..KEYS {
    cache.insert_with_ttl(k, k, 1, TTL);
  }
  assert!(wait_for(Duration::from_secs(4), || cache.metrics().evicted_by_ttl == KEYS), "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[tokio::test]
async fn test_async_insert_with_ttl_is_removed_with_no_builder_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build_async().unwrap();
  for k in 0..KEYS {
    cache.insert_with_ttl(k, k, 1, TTL).await;
  }
  let start = Instant::now();
  while start.elapsed() < Duration::from_secs(4) && cache.metrics().evicted_by_ttl != KEYS {
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  assert_eq!(cache.metrics().evicted_by_ttl, KEYS, "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[test]
fn test_plain_inserts_stay_beside_entries_with_their_own_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build().unwrap();
  for k in 0..100 {
    cache.insert(k, k, 1);
    cache.insert_with_ttl(k + 1000, k, 1, TTL);
  }
  assert!(wait_for(Duration::from_secs(4), || cache.metrics().evicted_by_ttl == 100), "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 100);
  for k in 0..100 {
    assert!(cache.fetch(&k).is_some());
  }
}

#[test]
fn test_replacing_with_a_plain_insert_drops_the_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build().unwrap();
  cache.insert_with_ttl(1, 1, 1, TTL);
  cache.insert(1, 2, 1);
  std::thread::sleep(Duration::from_millis(1500));
  assert_eq!(cache.fetch(&1).as_deref(), Some(&2));
  assert_eq!(cache.metrics().evicted_by_ttl, 0);
}

#[test]
fn test_entries_restored_with_a_ttl_are_removed_with_no_builder_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build().unwrap();
  for k in 0..100 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(500));
  }
  let snapshot = cache.to_snapshot();
  drop(cache);
  let restored = CacheBuilder::<u64, u64>::new().janitor_tick_interval(TICK).build_from_snapshot(snapshot).unwrap();
  assert!(wait_for(Duration::from_secs(4), || restored.metrics().evicted_by_ttl == 100), "{:?}", restored.metrics());
}

#[test]
fn test_dropping_the_cache_stops_a_janitor_started_by_insert_with_ttl() {
  for _ in 0..200 {
    let cache = CacheBuilder::<u64, u64>::new().janitor_tick_interval(Duration::from_millis(1)).build().unwrap();
    cache.insert_with_ttl(1, 1, 1, TTL);
  }
}
