use fibre_cache::{CacheBuilder, EvictionListener, EvictionReason};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TTI: Duration = Duration::from_millis(200);
const TICK: Duration = Duration::from_millis(20);
const KEYS: u64 = 2000;

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
fn test_sync_idle_entries_are_removed_at_the_default_shard_count() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_idle(TTI).janitor_tick_interval(TICK).build().unwrap();
  for k in 0..KEYS {
    cache.insert(k, k, 1);
  }
  assert!(wait_for(Duration::from_secs(3), || cache.metrics().evicted_by_tti == KEYS), "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[tokio::test]
async fn test_async_idle_entries_are_removed_at_the_default_shard_count() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_idle(TTI).janitor_tick_interval(TICK).build_async().unwrap();
  for k in 0..KEYS {
    cache.insert(k, k, 1).await;
  }
  let start = Instant::now();
  while start.elapsed() < Duration::from_secs(3) && cache.metrics().evicted_by_tti != KEYS {
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  assert_eq!(cache.metrics().evicted_by_tti, KEYS, "{:?}", cache.metrics());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[test]
fn test_read_entries_do_not_shield_idle_ones() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(TTI).janitor_tick_interval(TICK).build().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1);
  }
  let front: Vec<u64> = cache.iter().take(10).map(|(k, _)| k).collect();
  let start = Instant::now();
  while start.elapsed() < Duration::from_secs(2) {
    for k in &front {
      assert!(cache.fetch(k).is_some(), "a read key expired");
    }
    std::thread::sleep(Duration::from_millis(20));
  }
  let metrics = cache.metrics();
  assert_eq!(metrics.evicted_by_tti, 990, "{metrics:?}");
  assert_eq!(metrics.current_cost, 10);
}

#[test]
fn test_run_maintenance_removes_every_idle_entry() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(TTI).janitor_tick_interval(Duration::from_secs(3600)).build().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(TTI + TTI / 2 + Duration::from_millis(50));
  cache.run_maintenance();
  assert_eq!(cache.metrics().evicted_by_tti, 1000);
}

#[tokio::test]
async fn test_async_run_maintenance_removes_every_idle_entry() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(TTI).janitor_tick_interval(Duration::from_secs(3600)).build_async().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1).await;
  }
  tokio::time::sleep(TTI + TTI / 2 + Duration::from_millis(50)).await;
  cache.run_maintenance().await;
  assert_eq!(cache.metrics().evicted_by_tti, 1000);
}

#[test]
fn test_run_maintenance_removes_every_entry_past_its_ttl() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_live(Duration::from_millis(100)).janitor_tick_interval(Duration::from_secs(3600)).build().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(Duration::from_millis(150));
  cache.run_maintenance();
  assert_eq!(cache.metrics().evicted_by_ttl, 1000);
}

#[test]
fn test_a_read_key_stays_when_the_tti_is_shorter_than_the_tick() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(TTI).janitor_tick_interval(Duration::from_secs(1)).build().unwrap();
  cache.insert(1, 1, 1);
  let start = Instant::now();
  while start.elapsed() < Duration::from_millis(3500) {
    assert!(cache.fetch(&1).is_some(), "read key expired at {:?}", start.elapsed());
    std::thread::sleep(Duration::from_millis(5));
  }
  assert_eq!(cache.metrics().evicted_by_tti, 0);
}

#[test]
fn test_ttl_and_tti_each_count_as_their_own_removal() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_live(Duration::from_millis(600)).time_to_idle(TTI).janitor_tick_interval(TICK).build().unwrap();
  cache.insert(1, 1, 1);
  cache.insert(2, 2, 1);
  let start = Instant::now();
  while start.elapsed() < Duration::from_millis(1500) {
    let _ = cache.fetch(&2);
    std::thread::sleep(Duration::from_millis(10));
  }
  let metrics = cache.metrics();
  assert_eq!(metrics.evicted_by_tti, 1, "{metrics:?}");
  assert_eq!(metrics.evicted_by_ttl, 1, "{metrics:?}");
  assert_eq!(metrics.current_cost, 0);
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(u64, EvictionReason)>>>);

impl EvictionListener<u64, u64> for Recorder {
  fn on_evict(&self, key: u64, _value: Arc<u64>, reason: EvictionReason) {
    self.0.lock().unwrap().push((key, reason));
  }
}

#[test]
fn test_idle_removal_notifies_the_listener() {
  let recorder = Recorder::default();
  let cache = CacheBuilder::<u64, u64>::new().shards(8).time_to_idle(TTI).janitor_tick_interval(TICK).eviction_listener(recorder.clone()).build().unwrap();
  for k in 0..100 {
    cache.insert(k, k, 1);
  }
  assert!(wait_for(Duration::from_secs(3), || recorder.0.lock().unwrap().len() == 100));
  assert!(recorder.0.lock().unwrap().iter().all(|(_, reason)| *reason == EvictionReason::Expired));
}

#[test]
fn test_replacing_an_entry_restarts_its_idle_time() {
  let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(Duration::from_millis(400)).janitor_tick_interval(TICK).build().unwrap();
  cache.insert(1, 1, 1);
  std::thread::sleep(Duration::from_millis(300));
  cache.insert(1, 2, 1);
  std::thread::sleep(Duration::from_millis(300));
  assert_eq!(cache.fetch(&1).as_deref(), Some(&2));
  assert_eq!(cache.metrics().evicted_by_tti, 0);
}
