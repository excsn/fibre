use fibre_cache::CacheBuilder;
use std::{thread, time::Duration};

const TINY_TTI: Duration = Duration::from_millis(200);
const JANITOR_TICK: Duration = Duration::from_millis(50);
// A sleep duration safely longer than the TTI
const SLEEP_DURATION: Duration = Duration::from_millis(400);

#[test]
fn test_sync_item_expires_after_tti() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_idle(TINY_TTI)
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  cache.insert("key", "value", 1);

  // Wait for TTI to expire
  thread::sleep(SLEEP_DURATION);

  // The get call will see the item is expired and return None.
  // At this point, the item might still be in the cache map, waiting for the janitor.
  assert!(
    cache.get(&"key").is_none(),
    "Item should be identified as expired on get"
  );

  // Wait a little longer to give the janitor time to run and update the eviction metric.
  thread::sleep(JANITOR_TICK * 2);

  assert_eq!(
    cache.metrics().evicted_by_tti,
    1,
    "TTI eviction metric should be updated by janitor"
  );
}

#[test]
fn test_sync_tti_is_reset_on_access() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_idle(TINY_TTI)
    .janitor_tick_interval(JANITOR_TICK)
    .build()
    .unwrap();

  cache.insert("key", "value", 1);

  // Loop several times, each time sleeping for less than the TTI
  // and then accessing the key to reset the idle timer.
  for _ in 0..3 {
    thread::sleep(TINY_TTI / 2);
    assert!(
      cache.get(&"key").is_some(),
      "Item should be present before TTI expires"
    );
  }

  // Now, wait for the full TTI duration without access
  thread::sleep(SLEEP_DURATION);

  // The item should finally be expired.
  assert!(
    cache.get(&"key").is_none(),
    "Item should have expired after final idle period"
  );
}
