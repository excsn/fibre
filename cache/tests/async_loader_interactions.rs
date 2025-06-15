use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_async_stale_while_revalidate() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let cache = CacheBuilder::default()
    .capacity(10)
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(Duration::from_millis(500))
    .async_loader({
      let load_count = load_count.clone();
      move |key: i32| {
        let load_count = load_count.clone();
        async move {
          let count = load_count.fetch_add(1, Ordering::SeqCst);
          (key + (count as i32 * 100), 1)
        }
      }
    })
    .build_async()
    .unwrap();

  // 1. First call, triggers initial load.
  let value1 = cache.fetch_with(&5).await;
  assert_eq!(*value1, 5);
  assert_eq!(load_count.load(Ordering::Relaxed), 1);

  // 2. Wait for TTL to expire, but stay within the stale grace period.
  sleep(Duration::from_millis(150)).await;

  // 3. Second call. Should return the STALE value (5) immediately.
  let value2 = cache.fetch_with(&5).await;
  assert_eq!(*value2, 5, "Should return stale value immediately");

  // 4. Wait for the background load to complete.
  sleep(Duration::from_millis(50)).await;
  assert_eq!(
    load_count.load(Ordering::Relaxed),
    2,
    "Background refresh should have run"
  );

  // 5. Third call. Should now return the new, refreshed value.
  let value3 = cache.fetch_with(&5).await;
  assert_eq!(*value3, 105);
  assert_eq!(
    load_count.load(Ordering::Relaxed),
    2,
    "Loader should not be called again"
  );
}
