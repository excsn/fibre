use fibre_cache::{CacheBuilder, builder::TimerWheelMode};
use tokio::time::{Duration, sleep};

const TINY_TTL: Duration = Duration::from_millis(150);
const JANITOR_TICK: Duration = Duration::from_millis(10);
const SLEEP_MARGIN: Duration = Duration::from_millis(150);

#[tokio::test]
async fn test_async_item_expires_after_ttl() {
  let cache = CacheBuilder::<&str, &str>::new()
    .shards(1)
    .time_to_live(TINY_TTL)
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK)
    .maintenance_chance(1)
    .build_async()
    .unwrap();

  cache.insert("key", "value", 1).await;

  // 1. Immediately after insert, the value should be present.
  assert!(cache.fetch(&"key").await.is_some());

  // 2. Wait for a duration longer than the TTL.
  sleep(TINY_TTL + SLEEP_MARGIN).await;

  // 3. The janitor should have run and removed the expired item.
  assert!(
    cache.fetch(&"key").await.is_none(),
    "Item should have expired"
  );

  // 4. Verify metrics
  let metrics = cache.metrics();
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
  assert_eq!(metrics.evicted_by_ttl, 1);
  assert_eq!(metrics.current_cost, 0);
}

#[tokio::test]
async fn test_async_ttl_is_not_reset_on_access() {
  let cache = CacheBuilder::<&str, &str>::new()
    .time_to_live(TINY_TTL)
    // --- FIX: Add high-precision timer mode ---
    .timer_mode(TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(JANITOR_TICK)
    .build_async()
    .unwrap();

  cache.insert("key", "value", 1).await;

  // Wait for half the TTL
  sleep(TINY_TTL / 2).await;

  // Access the key. This should NOT reset the TTL.
  assert!(cache.fetch(&"key").await.is_some());

  // Wait for the second half of the TTL
  sleep(TINY_TTL / 2 + SLEEP_MARGIN).await;

  // The item should now be expired, as the total time since insert > TTL.
  assert!(
    cache.fetch(&"key").await.is_none(),
    "Item should have expired despite access"
  );
}
