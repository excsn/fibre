use fibre_cache::CacheBuilder;
use tokio::time::{sleep, Duration};

const TINY_TTI: Duration = Duration::from_millis(200);
const JANITOR_TICK: Duration = Duration::from_millis(50);
// A sleep duration safely longer than the TTI
const SLEEP_DURATION: Duration = Duration::from_millis(400);

#[tokio::test]
async fn test_async_item_expires_after_tti() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_idle(TINY_TTI)
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  cache.insert("key", "value", 1).await;

  // Wait for TTI to expire
  sleep(SLEEP_DURATION).await;

  // The get call will see the item is expired and return None.
  assert!(
    cache.get(&"key").is_none(),
    "Item should expire after being idle"
  );

  // Wait a little longer to give the janitor time to run and update the eviction metric.
  sleep(JANITOR_TICK * 2).await;

  assert_eq!(
    cache.metrics().evicted_by_ttl,
    1,
    "TTI eviction metric should be updated by janitor"
  );
}

#[tokio::test]
async fn test_async_tti_is_reset_on_access() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_idle(TINY_TTI)
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  cache.insert("key", "value", 1).await;

  // Loop several times, each time sleeping for less than the TTI
  // and then accessing the key to reset the idle timer.
  for _ in 0..3 {
    sleep(TINY_TTI / 2).await;
    assert!(
      cache.get(&"key").is_some(),
      "Item should be present before TTI expires"
    );
  }

  // Now, wait for the full TTI duration without access
  sleep(SLEEP_DURATION).await;

  // The item should finally be expired.
  assert!(
    cache.get(&"key").is_none(),
    "Item should have expired after final idle period"
  );
}
