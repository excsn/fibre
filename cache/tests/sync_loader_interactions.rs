use fibre_cache::CacheBuilder;
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
  let value1 = cache.get_with(&5);
  assert_eq!(*value1, 5);
  assert_eq!(load_count.load(Ordering::Relaxed), 1);

  // 2. Wait for TTL to expire, but stay within the stale grace period.
  thread::sleep(Duration::from_millis(150));

  // 3. Second call. Should return the STALE value (5) immediately.
  // A background refresh is triggered.
  let value2 = cache.get_with(&5);
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
  let value3 = cache.get_with(&5);
  assert_eq!(*value3, 105);
  assert_eq!(
    load_count.load(Ordering::Relaxed),
    2,
    "Loader should not be called again"
  );
}
