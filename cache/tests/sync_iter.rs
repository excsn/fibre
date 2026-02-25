mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use fibre::spsc;
use fibre_cache::CacheBuilder;

use crate::common::{ShardControllingHasher, build_test_cache_with_cap};

// ================================================================================================
// === Category A: Basic Functionality Tests
// ================================================================================================

#[test]
fn iter_on_empty_cache() {
  let cache = build_test_cache_with_cap(4, 100);
  assert_eq!(
    cache.iter().count(),
    0,
    "Iterator on empty cache should yield no items"
  );
}

#[test]
fn iter_visits_all_items_single_batch() {
  let cache = build_test_cache_with_cap(4, 100);
  let mut expected = HashSet::new();

  for i in 0..20 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter().collect();
  assert_eq!(collected, expected);
}

#[test]
fn iter_visits_all_items_multiple_batches() {
  let cache = build_test_cache_with_cap(4, 100);
  let mut expected = HashSet::new();

  for i in 0..100 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter_with_batch_size(10).collect();
  assert_eq!(collected, expected);
}

#[test]
fn iter_with_batch_size_of_one() {
  let cache = build_test_cache_with_cap(4, 100);
  let mut expected = HashSet::new();

  for i in 0..10 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter_with_batch_size(1).collect();
  assert_eq!(collected, expected);
}

#[test]
fn iter_handles_partially_filled_shards() {
  let cache = build_test_cache_with_cap(4, 100);
  let mut expected = HashSet::new();

  // Insert 20 items, but only into shards 0 and 2. Shards 1 and 3 remain empty.
  for i in (0..20).filter(|x| x % 4 == 0 || x % 4 == 2) {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter().collect();
  assert_eq!(collected, expected);
  assert_eq!(collected.len(), 10);
}

// ================================================================================================
// === Category B: Correctness and Expiration Tests
// ================================================================================================

#[test]
fn iter_skips_ttl_expired_items() {
  let cache = CacheBuilder::<i32, String>::new()
    .shards(1)
    .time_to_live(Duration::from_millis(50))
    .janitor_tick_interval(Duration::from_millis(10)) // Fast janitor for cleanup
    .build()
    .unwrap();

  let mut expected = HashSet::new();

  // Insert items that will expire
  for i in 0..5 {
    cache.insert(i, i.to_string(), 1);
  }

  thread::sleep(Duration::from_millis(100));

  // Insert fresh items
  for i in 5..10 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter().collect();
  assert_eq!(collected, expected, "Iterator should skip expired items");
}

#[test]
fn iter_skips_items_expiring_mid_iteration() {
  let cache = CacheBuilder::new()
    .shards(4) // Use multiple shards
    .hasher(ShardControllingHasher)
    .time_to_live(Duration::from_millis(100))
    .janitor_tick_interval(Duration::from_millis(10))
    .build()
    .unwrap();

  // Insert items into shard 0 and shard 2
  for i in [0, 4, 8] {
    cache.insert(i, i.to_string(), 1);
  } // Shard 0
  for i in [2, 6, 10] {
    cache.insert(i, i.to_string(), 1);
  } // Shard 2

  let mut iter = cache.iter_with_batch_size(3);

  // 1. Get the first batch (from shard 0). These items are valid at this point.
  let mut collected = vec![];
  collected.push(iter.next().unwrap());
  collected.push(iter.next().unwrap());
  collected.push(iter.next().unwrap());

  // 2. Wait for all items in the cache to expire.
  thread::sleep(Duration::from_millis(150));

  // 3. Continue iterating. The next `refill_buffer` call will scan shard 2,
  //    find that its items are expired, and skip them.
  for item in iter {
    collected.push(item);
  }

  // 4. Assert that we only collected the initial batch from shard 0.
  assert_eq!(
    collected.len(),
    3,
    "Only the first valid batch should be collected"
  );
}

// ================================================================================================
// === Category C: Concurrency and Weak Consistency Tests
// ================================================================================================

#[test]
fn iter_misses_insert_after_shard_scan() {
  // This test confirms that an item inserted into a shard that has already been
  // scanned by the iterator will be missed.
  let cache = Arc::new(build_test_cache_with_cap(4, 100));
  let (go_tx, go_rx) = spsc::bounded_sync(1);
  let (done_tx, done_rx) = spsc::bounded_sync(1);

  // Populate Shard 0 with 2 items.
  cache.insert(0, "a".to_string(), 1);
  cache.insert(4, "b".to_string(), 1);

  // *** Insert at least one item into a subsequent shard. ***
  // This is crucial. It ensures that when the iterator fills its first batch,
  // it will completely drain Shard 0 and advance its cursor to Shard 1,
  // thereby "finalizing" its scan of Shard 0.
  cache.insert(1, "c".to_string(), 1); // Belongs to Shard 1.

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    // Wait for the main thread's iterator to signal.
    go_rx.recv().unwrap();
    // Insert a new item into Shard 0, which has already been fully scanned.
    cache_clone.insert(100, "new".to_string(), 1); // 100 % 4 = 0
    done_tx.send(()).unwrap();
  });

  let mut collected = HashSet::new();
  let mut items_seen = 0;
  let mut signaled = false;

  // Use a batch size that forces the iterator to cross the shard boundary.
  // Batch size 3 will read all 2 items from Shard 0 and 1 item from Shard 1.
  for (key, value) in cache.iter_with_batch_size(3) {
    items_seen += 1;
    collected.insert((key, value));

    // After seeing 2 items, we know the iterator *might* be done with shard 0.
    // After 3 items, we know it *is* done, because it had to move to shard 1.
    if items_seen >= 3 && !signaled {
      go_tx.send(()).unwrap();
      done_rx.recv().unwrap(); // Wait for the concurrent insert to complete.
      signaled = true;
    }
  }

  thread_handle.join().unwrap();

  // The core assertion remains the same.
  assert!(
    collected.iter().all(|(k, _)| *k != 100),
    "Item inserted after its shard was scanned should be missed"
  );
  assert_eq!(
    collected.len(),
    3,
    "Should have collected all original items"
  );
}

#[test]
fn iter_sees_insert_before_shard_scan() {
  // This test confirms that an item inserted into a shard before the iterator
  // has scanned it will be included in the results.
  let cache = Arc::new(build_test_cache_with_cap(4, 100));
  let (go_tx, go_rx) = spsc::bounded_sync(1);
  let (done_tx, done_rx) = spsc::bounded_sync(1);

  for i in (0..10).filter(|x| x % 4 == 0) {
    cache.insert(i, i.to_string(), 1);
  } // 3 items in shard 0

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    go_rx.recv().unwrap();
    cache_clone.insert(102, "new".to_string(), 1); // 102 % 4 = 2
    done_tx.send(()).unwrap();
  });

  let mut collected = HashSet::new();
  let mut items_seen = 0;
  let mut signaled = false;

  for (key, value) in cache.iter_with_batch_size(3) {
    items_seen += 1;
    collected.insert((key, value));
    if items_seen >= 3 && !signaled {
      go_tx.send(()).unwrap();
      done_rx.recv().unwrap(); // Wait for the insert to complete
      signaled = true;
    }
  }

  thread_handle.join().unwrap();

  assert!(
    collected.iter().any(|(k, _)| *k == 102),
    "Item inserted before its shard was scanned should be seen"
  );
  assert_eq!(
    collected.len(),
    4,
    "Should have collected original items plus the new one"
  );
}

#[test]
fn iter_sees_updated_value_before_scan() {
  // This test confirms that if an item is updated before its shard is scanned,
  // the iterator sees the new value.
  let cache = Arc::new(build_test_cache_with_cap(4, 100));
  let (go_tx, go_rx) = spsc::bounded_sync(1);
  let (done_tx, done_rx) = spsc::bounded_sync(1);

  cache.insert(0, "zero".to_string(), 1); // In shard 0
  cache.insert(2, "old_two".to_string(), 1); // In shard 2

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    go_rx.recv().unwrap();
    cache_clone.insert(2, "new_two".to_string(), 1);
    done_tx.send(()).unwrap();
  });

  let mut collected = HashMap::new();
  let mut signaled = false;

  // Use a small batch size to prevent greedy buffering of the whole cache.
  for (key, value) in cache.iter_with_batch_size(1) {
    collected.insert(key, value);
    if !signaled {
      go_tx.send(()).unwrap();
      done_rx.recv().unwrap(); // Wait for the update to complete
      signaled = true;
    }
  }

  thread_handle.join().unwrap();

  assert_eq!(**collected.get(&2).unwrap(), "new_two");
}

#[test]
fn iter_does_not_block_writers_on_other_shards() {
  // This test shows that an iterator holding a read lock on one shard does not
  // prevent write operations on other shards.
  let cache = Arc::new(build_test_cache_with_cap(4, 100));
  let barrier = Arc::new(Barrier::new(2));

  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();
  let writer_thread = thread::spawn(move || {
    // Wait for the iterator to start and grab its lock
    barrier_clone.wait();

    // These inserts should complete immediately because they are on different shards
    // from the iterator, which will be on shard 0.
    let start = std::time::Instant::now();
    cache_clone.insert(1, "shard 1".to_string(), 1);
    cache_clone.insert(2, "shard 2".to_string(), 1);
    cache_clone.insert(3, "shard 3".to_string(), 1);

    // Assert that the writes were not blocked for a long time
    assert!(start.elapsed() < Duration::from_millis(50));
  });

  let mut iter = cache.iter();
  // This `next()` call will lock shard 0 and fill the first buffer.
  iter.next();

  // Signal the writer thread that we have the lock
  barrier.wait();

  // Hold the "lock" by doing nothing, giving the writer time to run
  thread::sleep(Duration::from_millis(100));

  // Finish iterating
  for _ in iter {}

  writer_thread.join().unwrap();
}
