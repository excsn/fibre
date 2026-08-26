use fibre_cache::{CacheBuilder, builder::TimerWheelMode};
use std::{thread, time::Duration};

const TINY_TTL: Duration = Duration::from_millis(150);
const JANITOR_TICK: Duration = Duration::from_millis(10);
const SLEEP_MARGIN: Duration = Duration::from_millis(150);

#[test]
fn test_sync_item_expires_after_ttl() {
  let cache = CacheBuilder::<&str, &str>::new()
    .shards(1)
    .time_to_live(TINY_TTL)
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  cache.insert("key", "value", 1);
  assert!(cache.fetch(&"key").is_some());
  thread::sleep(TINY_TTL + SLEEP_MARGIN);
  assert!(cache.fetch(&"key").is_none(), "Item should have expired");

  let metrics = cache.metrics();
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
  assert_eq!(metrics.evicted_by_ttl, 1);
  assert_eq!(metrics.current_cost, 0);
}

/// Physical removal must not depend on the janitor keeping pace with the wheel's tick
/// duration. With the default maintenance chance a shard's TTL sweep runs rarely; the
/// wheel has to catch up on elapsed wall time when it does, or expired values stay
/// resident until the cache drops (they are only masked from reads).
#[test]
fn test_sync_ttl_frees_memory_under_default_maintenance_chance() {
  let cache = CacheBuilder::<&str, String>::new()
    .shards(4)
    .time_to_live(Duration::from_millis(500))
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  cache.insert("key", "value".to_string(), 1);
  let weak = std::sync::Arc::downgrade(&cache.fetch(&"key").unwrap());

  let deadline = std::time::Instant::now() + Duration::from_secs(3);
  while weak.upgrade().is_some() {
    assert!(
      std::time::Instant::now() < deadline,
      "TTL-expired value still resident 3s after a 500ms TTL"
    );
    thread::sleep(Duration::from_millis(50));
  }
  assert_eq!(cache.metrics().evicted_by_ttl, 1);
}

/// A TTL far longer than one wheel revolution exercises the lap arithmetic of the
/// catch-up path: sparse sweeps must burn the correct number of laps, no more.
#[test]
fn test_sync_ttl_expiry_across_wheel_laps() {
  let cache = CacheBuilder::<&str, String>::new()
    .shards(1)
    .time_to_live(Duration::from_millis(200))
    .timer_tick_duration(Duration::from_millis(5))
    .timer_wheel_size(8)
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  cache.insert("short", "value".to_string(), 1);
  let weak = std::sync::Arc::downgrade(&cache.fetch(&"short").unwrap());

  thread::sleep(Duration::from_millis(100));
  assert!(weak.upgrade().is_some(), "evicted before its TTL elapsed");

  let deadline = std::time::Instant::now() + Duration::from_secs(2);
  while weak.upgrade().is_some() {
    assert!(
      std::time::Instant::now() < deadline,
      "multi-lap timer never expired"
    );
    thread::sleep(Duration::from_millis(20));
  }
}

#[test]
fn test_sync_ttl_is_not_reset_on_access() {
  let cache = CacheBuilder::<&str, &str>::new()
    .shards(1)
    .time_to_live(TINY_TTL)
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build()
    .unwrap();

  cache.insert("key", "value", 1);
  thread::sleep(TINY_TTL / 2);
  assert!(cache.fetch(&"key").is_some());
  thread::sleep(TINY_TTL / 2 + SLEEP_MARGIN);
  assert!(
    cache.fetch(&"key").is_none(),
    "Item should have expired despite access"
  );
}
