use fibre_cache::{CacheBuilder, builder::TimerWheelMode};
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use std::thread;
use std::time::Duration;

#[test]
fn test_sync_stale_while_revalidate() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(Duration::from_millis(500))
    .loader({
      let load_count = load_count.clone();
      move |key: i32| {
        let count = load_count.fetch_add(1, Ordering::SeqCst);
        // Return a different value for each load to check for refresh
        (key + (count as i32 * 100), 1)
      }
    })
    .build()
    .unwrap();

  // 1. First call, triggers initial load.
  // Load count becomes 1, value is 5 + 0*100 = 5
  let value1 = cache.fetch_with(&5);
  assert_eq!(*value1, 5);
  assert_eq!(load_count.load(Ordering::Relaxed), 1);

  // 2. Wait for TTL to expire, but stay within the stale grace period.
  thread::sleep(Duration::from_millis(150));

  // 3. Second call. Should return the STALE value (5) immediately.
  // A background refresh is triggered.
  let value2 = cache.fetch_with(&5);
  assert_eq!(*value2, 5, "Should return stale value immediately");
  // Loader has been *triggered* but may not have completed, so count might still be 1.
  // This is hard to test deterministically without more complex sync.

  // 4. Wait for the background load to complete.
  thread::sleep(Duration::from_millis(50));
  assert_eq!(
    load_count.load(Ordering::Relaxed),
    2,
    "Background refresh should have run"
  );

  // 5. Third call. Should now return the new, refreshed value.
  // Load count is 2, so value should be 5 + 1*100 = 105
  let value3 = cache.fetch_with(&5);
  assert_eq!(*value3, 105);
  assert_eq!(
    load_count.load(Ordering::Relaxed),
    2,
    "Loader should not be called again"
  );
}

#[test]
fn test_sync_stale_value_survives_janitor_sweeps_during_grace() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(Duration::from_secs(5))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader({
      let load_count = load_count.clone();
      move |key: i32| (key + load_count.fetch_add(1, Ordering::SeqCst) as i32 * 100, 1)
    })
    .build()
    .unwrap();

  assert_eq!(*cache.fetch_with(&5), 5);
  thread::sleep(Duration::from_millis(500));

  assert_eq!(
    *cache.fetch_with(&5),
    5,
    "a read inside the grace period must return the stale value"
  );
}

#[test]
fn test_sync_stale_value_is_removed_after_grace() {
  let grace = Duration::from_millis(300);
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(grace)
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader(|key: i32| (key, 1))
    .build()
    .unwrap();

  let weak = Arc::downgrade(&cache.fetch_with(&5));
  let deadline = std::time::Instant::now() + Duration::from_secs(2);
  while weak.upgrade().is_some() {
    assert!(
      std::time::Instant::now() < deadline,
      "stale value still resident 2 s after insert with a 400 ms TTL plus grace"
    );
    thread::sleep(Duration::from_millis(20));
  }
  assert_eq!(cache.metrics().evicted_by_ttl, 1);
}

#[test]
fn test_sync_grace_defers_ttl_removal_accounting() {
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(Duration::from_secs(5))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader(|key: i32| (key, 1))
    .build()
    .unwrap();

  cache.fetch_with(&5);
  thread::sleep(Duration::from_millis(500));

  let metrics = cache.metrics();
  assert_eq!(metrics.evicted_by_ttl, 0);
  assert_eq!(metrics.current_cost, 1);
}

#[test]
fn test_sync_refresh_during_grace_keeps_the_new_entry() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(400))
    .stale_while_revalidate(Duration::from_secs(5))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader({
      let load_count = load_count.clone();
      move |key: i32| (key + load_count.fetch_add(1, Ordering::SeqCst) as i32 * 100, 1)
    })
    .build()
    .unwrap();

  assert_eq!(*cache.fetch_with(&5), 5);
  thread::sleep(Duration::from_millis(500));
  assert_eq!(*cache.fetch_with(&5), 5);
  thread::sleep(Duration::from_millis(100));

  assert_eq!(*cache.fetch_with(&5), 105);
  assert_eq!(load_count.load(Ordering::SeqCst), 2);
  assert_eq!(cache.metrics().evicted_by_ttl, 0);
}

#[test]
fn test_sync_insert_with_ttl_keeps_the_grace_period() {
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_secs(3600))
    .stale_while_revalidate(Duration::from_secs(5))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .loader(|_key: i32| (999, 1))
    .build()
    .unwrap();

  cache.insert_with_ttl(5, 5, 1, Duration::from_millis(100));
  thread::sleep(Duration::from_millis(500));

  assert_eq!(*cache.fetch_with(&5), 5);
}
