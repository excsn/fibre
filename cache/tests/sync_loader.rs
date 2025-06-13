use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc, Barrier,
};
use std::thread;

#[test]
fn test_sync_loader_basic() {
  // A counter to see how many times the loader is called.
  let load_count = Arc::new(AtomicUsize::new(0));

  let cache = CacheBuilder::new()
    .capacity(10)
    .loader({
      let load_count = load_count.clone();
      // The loader closure takes the key and returns (value, cost).
      move |key: i32| {
        load_count.fetch_add(1, Ordering::SeqCst);
        (key * 10, 1) // e.g., for key 5, loads value 50 with cost 1
      }
    })
    .build()
    .unwrap();

  // 1. First call to get_with on a missing key.
  // This should trigger the loader.
  let value = cache.get_with(&5);
  assert_eq!(*value, 50);
  assert_eq!(
    load_count.load(Ordering::SeqCst),
    1,
    "Loader should be called once"
  );
  assert_eq!(cache.metrics().misses, 1);
  assert_eq!(cache.metrics().inserts, 1);

  // 2. Second call to get_with on the same key.
  // This should be a cache hit and not call the loader.
  let value = cache.get_with(&5);
  assert_eq!(*value, 50);
  assert_eq!(
    load_count.load(Ordering::SeqCst),
    1,
    "Loader should NOT be called again"
  );
  assert_eq!(cache.metrics().hits, 1);
}

#[test]
fn test_sync_loader_thundering_herd() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let num_threads = 20;

  let cache = Arc::new(
    CacheBuilder::new()
      .capacity(10)
      .loader({
        let load_count = load_count.clone();
        move |key: i32| {
          // Simulate a slow database call or computation
          thread::sleep(std::time::Duration::from_millis(100));
          load_count.fetch_add(1, Ordering::SeqCst);
          (key * 10, 1)
        }
      })
      .build()
      .unwrap(),
  );

  let barrier = Arc::new(Barrier::new(num_threads));
  let mut handles = vec![];

  for _ in 0..num_threads {
    let cache_clone = cache.clone();
    let barrier_clone = barrier.clone();
    handles.push(thread::spawn(move || {
      // Wait for all threads to be ready
      barrier_clone.wait();
      // All threads request the same missing key at once
      let value = cache_clone.get_with(&99);
      assert_eq!(*value, 990);
    }));
  }

  for handle in handles {
    handle.join().unwrap();
  }

  // The core assertion: Despite 20 concurrent requests, the loader
  // was only executed ONCE.
  assert_eq!(
    load_count.load(Ordering::SeqCst),
    1,
    "Thundering herd protection failed: loader was called more than once"
  );
  assert_eq!(
    cache.metrics().misses,
    1,
    "There should be only one initial miss"
  );
  // Hits will be num_threads - 1 because the other threads hit the pending future
  assert_eq!(cache.metrics().hits, (num_threads - 1) as u64);
}
