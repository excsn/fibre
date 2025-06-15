use fibre_cache::{builder::TimerWheelMode, CacheBuilder};
use std::{thread, time::Duration};

const TINY_TTL: Duration = Duration::from_millis(150);
const JANITOR_TICK: Duration = Duration::from_millis(10);
const SLEEP_MARGIN: Duration = Duration::from_millis(150);

#[test]
fn test_sync_item_expires_after_ttl() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_live(TINY_TTL)
    .timer_mode(TimerWheelMode::HighPrecisionShortLived) // Add this line
    .janitor_tick_interval(JANITOR_TICK) // Set fast tick
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

#[test]
fn test_sync_ttl_is_not_reset_on_access() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_live(TINY_TTL)
    // --- FIX: Add high-precision timer mode ---
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK) // Set fast tick
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