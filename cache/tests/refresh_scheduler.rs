use fibre_cache::{AsyncCache, Cache, CacheBuilder};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Loads {
  live: Arc<AtomicUsize>,
  peak: Arc<AtomicUsize>,
  calls: Arc<AtomicUsize>,
}

impl Loads {
  fn enter(&self) {
    let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
    self.peak.fetch_max(now, Ordering::SeqCst);
    self.calls.fetch_add(1, Ordering::SeqCst);
  }

  fn leave(&self) {
    self.live.fetch_sub(1, Ordering::SeqCst);
  }

  fn peak(&self) -> usize {
    self.peak.load(Ordering::SeqCst)
  }
}

fn sync_cache(loads: Loads, limit: Option<usize>, ttl: Duration, grace: Duration, load: Duration) -> Cache<u64, u64> {
  let mut builder = CacheBuilder::<u64, u64>::new()
    .time_to_live(ttl)
    .stale_while_revalidate(grace)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader(move |k| {
      loads.enter();
      std::thread::sleep(load);
      loads.leave();
      (k + 1_000_000, 1)
    });
  if let Some(limit) = limit {
    builder = builder.max_concurrent_refreshes(limit);
  }
  builder.build().unwrap()
}

fn async_cache(loads: Loads, limit: Option<usize>, ttl: Duration, grace: Duration, load: Duration) -> AsyncCache<u64, u64> {
  let mut builder = CacheBuilder::<u64, u64>::new()
    .time_to_live(ttl)
    .stale_while_revalidate(grace)
    .janitor_tick_interval(Duration::from_millis(10))
    .async_loader(move |k| {
      let loads = loads.clone();
      async move {
        loads.enter();
        tokio::time::sleep(load).await;
        loads.leave();
        (k + 1_000_000, 1)
      }
    });
  if let Some(limit) = limit {
    builder = builder.max_concurrent_refreshes(limit);
  }
  builder.build_async().unwrap()
}

/// A short first TTL makes every key stale at once. Refreshed values last for the builder's TTL.
const LONG_TTL: Duration = Duration::from_secs(600);
const GRACE: Duration = Duration::from_secs(600);

#[test]
fn test_sync_over_capacity_every_stale_key_is_refreshed_in_turn() {
  let loads = Loads::default();
  let ttl = Duration::from_millis(150);
  let cache = sync_cache(loads.clone(), None, ttl, GRACE, Duration::from_millis(50));
  for k in 0..400 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(ttl + Duration::from_millis(20));
  let start = Instant::now();
  let mut ever_refreshed = vec![false; 400];
  while start.elapsed() < Duration::from_secs(6) && ever_refreshed.iter().any(|seen| !seen) {
    for k in 0..400u64 {
      if *cache.fetch_with(&k) >= 1_000_000 {
        ever_refreshed[k as usize] = true;
      }
    }
    std::thread::sleep(Duration::from_millis(2));
  }
  assert_eq!(ever_refreshed.iter().filter(|seen| !**seen).count(), 0);
  assert!(loads.peak() <= 32, "peak {}", loads.peak());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_async_over_capacity_every_stale_key_is_refreshed_in_turn() {
  let loads = Loads::default();
  let ttl = Duration::from_millis(150);
  let cache = async_cache(loads.clone(), None, ttl, GRACE, Duration::from_millis(50));
  for k in 0..400 {
    cache.insert(k, k, 1).await;
  }
  tokio::time::sleep(ttl + Duration::from_millis(20)).await;
  let start = Instant::now();
  let mut ever_refreshed = vec![false; 400];
  while start.elapsed() < Duration::from_secs(6) && ever_refreshed.iter().any(|seen| !seen) {
    for k in 0..400u64 {
      if *cache.fetch_with(&k).await >= 1_000_000 {
        ever_refreshed[k as usize] = true;
      }
    }
    tokio::time::sleep(Duration::from_millis(2)).await;
  }
  assert_eq!(ever_refreshed.iter().filter(|seen| !**seen).count(), 0);
  assert!(loads.peak() <= 32, "peak {}", loads.peak());
}

#[test]
fn test_sync_limit_holds_and_the_queue_drains_without_further_reads() {
  let loads = Loads::default();
  let cache = sync_cache(loads.clone(), Some(2), LONG_TTL, GRACE, Duration::from_millis(20));
  for k in 0..40 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(50));
  }
  std::thread::sleep(Duration::from_millis(100));
  for k in 0..40u64 {
    assert_eq!(*cache.fetch_with(&k), k);
  }
  let start = Instant::now();
  while loads.calls.load(Ordering::SeqCst) < 40 && start.elapsed() < Duration::from_secs(4) {
    std::thread::sleep(Duration::from_millis(5));
  }
  assert_eq!(loads.calls.load(Ordering::SeqCst), 40);
  assert_eq!(loads.peak(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_async_limit_holds_and_the_queue_drains_without_further_reads() {
  let loads = Loads::default();
  let cache = async_cache(loads.clone(), Some(2), LONG_TTL, GRACE, Duration::from_millis(20));
  for k in 0..40 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(50)).await;
  }
  tokio::time::sleep(Duration::from_millis(100)).await;
  for k in 0..40u64 {
    assert_eq!(*cache.fetch_with(&k).await, k);
  }
  let start = Instant::now();
  while loads.calls.load(Ordering::SeqCst) < 40 && start.elapsed() < Duration::from_secs(4) {
    tokio::time::sleep(Duration::from_millis(5)).await;
  }
  assert_eq!(loads.calls.load(Ordering::SeqCst), 40);
  assert_eq!(loads.peak(), 2);
}

#[test]
fn test_a_key_stale_again_goes_behind_the_keys_already_waiting() {
  let loads = Loads::default();
  let cache = sync_cache(loads.clone(), Some(1), Duration::from_millis(30), GRACE, Duration::from_millis(30));
  for k in 0..10 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(Duration::from_millis(60));
  let start = Instant::now();
  let mut last = vec![0u64; 10];
  while start.elapsed() < Duration::from_secs(3) && last.iter().any(|v| *v < 1_000_000) {
    for k in 0..10u64 {
      last[k as usize] = *cache.fetch_with(&k);
    }
    std::thread::sleep(Duration::from_millis(1));
  }
  assert!(last.iter().all(|v| *v >= 1_000_000), "{last:?}");
}

#[test]
fn test_sync_miss_past_grace_takes_its_refresh_from_the_queue() {
  let loads = Loads::default();
  let cache = sync_cache(loads, Some(1), LONG_TTL, Duration::from_millis(100), Duration::from_millis(300));
  for k in 0..50 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(20));
  }
  std::thread::sleep(Duration::from_millis(40));
  for k in 0..50u64 {
    assert_eq!(*cache.fetch_with(&k), k);
  }
  std::thread::sleep(Duration::from_millis(150));
  let start = Instant::now();
  assert_eq!(*cache.fetch_with(&49), 1_000_049);
  assert!(start.elapsed() < Duration::from_secs(2), "waited {:?} behind the queue", start.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_async_miss_past_grace_takes_its_refresh_from_the_queue() {
  let loads = Loads::default();
  let cache = async_cache(loads, Some(1), LONG_TTL, Duration::from_millis(100), Duration::from_millis(300));
  for k in 0..50 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(20)).await;
  }
  tokio::time::sleep(Duration::from_millis(40)).await;
  for k in 0..50u64 {
    assert_eq!(*cache.fetch_with(&k).await, k);
  }
  tokio::time::sleep(Duration::from_millis(150)).await;
  let start = Instant::now();
  assert_eq!(*cache.fetch_with(&49).await, 1_000_049);
  assert!(start.elapsed() < Duration::from_secs(2), "waited {:?} behind the queue", start.elapsed());
}

#[test]
fn test_panicking_refreshes_keep_the_queue_moving() {
  let fail = Arc::new(AtomicBool::new(true));
  let flag = fail.clone();
  let cache = CacheBuilder::<u64, u64>::new()
    .time_to_live(LONG_TTL)
    .stale_while_revalidate(GRACE)
    .janitor_tick_interval(Duration::from_millis(10))
    .max_concurrent_refreshes(2)
    .loader(move |k| {
      if flag.load(Ordering::SeqCst) {
        panic!("loader failed");
      }
      (k + 1_000_000, 1)
    })
    .build()
    .unwrap();
  for k in 0..50 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(20));
  }
  std::thread::sleep(Duration::from_millis(40));
  for k in 0..50u64 {
    assert_eq!(*cache.fetch_with(&k), k);
  }
  std::thread::sleep(Duration::from_millis(100));
  fail.store(false, Ordering::SeqCst);
  let start = Instant::now();
  while *cache.fetch_with(&7) < 1_000_000 && start.elapsed() < Duration::from_secs(3) {
    std::thread::sleep(Duration::from_millis(5));
  }
  assert_eq!(*cache.fetch_with(&7), 1_000_007);
}

#[test]
#[should_panic(expected = "max_concurrent_refreshes")]
fn test_a_limit_of_zero_is_refused() {
  let _ = CacheBuilder::<u64, u64>::new().max_concurrent_refreshes(0);
}
