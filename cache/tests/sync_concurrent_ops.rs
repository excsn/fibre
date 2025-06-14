use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, Barrier,
};
use std::thread;
use std::time::Duration;

#[test]
fn test_sync_concurrent_load_and_invalidate() {
  let cache = Arc::new(
    CacheBuilder::default()
      .capacity(10)
      .loader(|key: i32| {
        thread::sleep(Duration::from_millis(50));
        (key * 10, 1)
      })
      .build()
      .unwrap(),
  );

  let num_loaders = 5;
  let barrier = Arc::new(Barrier::new(num_loaders + 1));
  let mut handles = vec![];

  // Spawn loader threads
  for _ in 0..num_loaders {
    let cache_clone = cache.clone();
    let barrier_clone = barrier.clone();
    handles.push(thread::spawn(move || {
      barrier_clone.wait();
      // All threads request the same key
      let value = cache_clone.get_with(&1);
      // The value could be 10 (if they ran before invalidate) or
      // potentially a re-loaded 10. The key is no deadlocks.
      assert_eq!(*value, 10);
    }));
  }

  // Spawn invalidator thread
  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();
  handles.push(thread::spawn(move || {
    barrier_clone.wait();
    // Invalidate the key while the others are potentially loading it.
    let _was_present = cache_clone.invalidate(&1);
    // It might be present or not depending on timing, so we don't assert this.
  }));

  for handle in handles {
    handle.join().unwrap(); // Test passes if it doesn't hang or panic
  }

  // Final check on metrics can give some insight
  let metrics = cache.metrics();
  assert!(metrics.inserts >= 1); // At least one load happened
  assert!(metrics.invalidations <= 1);
}

#[test]
fn test_sync_concurrent_insert_and_clear() {
  let cache = Arc::new(
    CacheBuilder::<i32, i32>::default()
      .capacity(1_000_000)
      .build()
      .unwrap(),
  );
  let stop_inserting = Arc::new(AtomicBool::new(false));

  let cache_clone = cache.clone();
  let stop_clone = stop_inserting.clone();
  let insert_handle = thread::spawn(move || {
    for i in 0.. {
      // Loop indefinitely until stopped
      if stop_clone.load(Ordering::Relaxed) {
        break;
      }
      cache_clone.insert(i, i, 1);
    }
  });

  let cache_clone_2 = cache.clone();
  let stop_clone_2 = stop_inserting.clone();
  let clear_handle = thread::spawn(move || {
    // Let the inserter run for a bit
    thread::sleep(Duration::from_millis(20));
    cache_clone_2.clear();
    // Signal the inserter to stop *after* clear() is done.
    stop_clone_2.store(true, Ordering::Relaxed);
  });

  insert_handle.join().unwrap();
  clear_handle.join().unwrap();

  // After clear() and stopping the insert thread, the number of items
  // should be very small. It can be slightly > 0 due to the race condition
  // where an insert call begins before the stop flag is checked.
  let final_cost = cache.metrics().current_cost;
  // A small threshold like 100 is more than enough to account for scheduling races.
  // The previous failure (34884) would still fail, but a cost of 1 will pass.
  assert!(
    final_cost < 100,
    "Final cost should be very low after clear, but was {}",
    final_cost
  );
}
