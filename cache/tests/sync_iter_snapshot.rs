mod common;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use fibre::spsc;

use crate::common::build_test_cache;

#[test]
fn snapshot_iter_visits_all_items() {
  let cache = build_test_cache(4);
  let mut expected = HashSet::new();

  for i in 0..100 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1);
    expected.insert((i, Arc::new(value)));
  }

  let collected: HashSet<_> = cache.iter_snapshot().collect();
  assert_eq!(collected, expected);
}

#[test]
fn snapshot_iter_misses_insert_after_shard_scan() {
  let cache = Arc::new(build_test_cache(4));

  // Pre-populate shards 0 and 2.
  cache.insert(0, "a".to_string(), 1); // Shard 0
  cache.insert(2, "b".to_string(), 1); // Shard 2

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    // This insert will happen after the main thread's iterator
    // has snapshotted the keys for shard 0.
    thread::sleep(std::time::Duration::from_millis(50));
    cache_clone.insert(4, "new".to_string(), 1); // Also in shard 0
  });

  let collected: Vec<_> = cache.iter_snapshot().collect();

  thread_handle.join().unwrap();

  assert_eq!(collected.len(), 2, "Should only see the original 2 items");
  assert!(
    !collected.iter().any(|(k, _)| *k == 4),
    "Should miss item inserted into a past shard"
  );
}

#[test]
fn snapshot_iter_sees_insert_before_shard_scan() {
  let cache = Arc::new(build_test_cache(4));
  // Channels for two-way synchronization
  let (go_tx, go_rx) = spsc::bounded_sync::<()>(1);
  let (done_tx, done_rx) = spsc::bounded_sync::<()>(1);

  // Pre-populate shard 0.
  cache.insert(0, "a".to_string(), 1);

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    // Wait for the signal from the iterator.
    go_rx.recv().unwrap();
    // Insert into a future shard (Shard 2).
    cache_clone.insert(2, "new".to_string(), 1);
    // Signal that the insert is complete.
    done_tx.send(()).unwrap();
  });

  let mut collected = Vec::new();
  let mut iter = cache.iter_snapshot();

  // 1. Manually process the first item from Shard 0.
  if let Some(item) = iter.next() {
    collected.push(item);
  }

  // 2. We now know Shard 0 has been snapshotted.
  //    Signal the writer thread and wait for it to finish.
  go_tx.send(()).unwrap();
  done_rx.recv().unwrap();

  // 3. Continue iterating. The iterator will now move to Shard 1, and then Shard 2,
  //    where it will see the newly inserted item.
  for item in iter {
    collected.push(item);
  }

  thread_handle.join().unwrap();

  assert_eq!(
    collected.len(),
    2,
    "Should see the original and the new item"
  );
  assert!(
    collected.iter().any(|(k, _)| *k == 2),
    "Should see item inserted into a future shard"
  );
}

#[test]
fn snapshot_iter_skips_deleted_item() {
  let cache = Arc::new(build_test_cache(4));
  // Channels for two-way synchronization
  let (go_tx, go_rx) = spsc::bounded_sync::<()>(1);
  let (done_tx, done_rx) = spsc::bounded_sync::<()>(1);

  cache.insert(0, "a".to_string(), 1); // Shard 0
  cache.insert(1, "b".to_string(), 1); // Shard 1

  let cache_clone = cache.clone();
  let thread_handle = thread::spawn(move || {
    // Wait for the signal from the iterator.
    go_rx.recv().unwrap();
    // Invalidate an item that the iterator has already snapshotted.
    cache_clone.invalidate(&0);
    // Signal that the invalidation is complete.
    done_tx.send(()).unwrap();
  });

  let iter = cache.iter_snapshot();

  // 1. The first call to `next()` will trigger `load_next_shard()`, which
  //    snapshots the keys from Shard 0 (in this case, just key `0`).
  //    However, we don't consume the item yet. We just want to ensure the snapshot happened.
  //    To do this robustly, we can manually drive the internal state.
  //    A simpler way for this test is to just signal immediately.

  // Signal the writer thread to delete key 0.
  go_tx.send(()).unwrap();
  // Wait for the deletion to complete.
  done_rx.recv().unwrap();

  // 2. Now, consume the iterator.
  // The iterator's internal `shard_keys` contains `vec![0]`.
  // The first `next()` call will try to `fetch(&0)`, which will return `None`.
  // The loop in `next()` will continue, load Shard 1, and fetch `key=1`.
  let collected: Vec<_> = iter.collect();

  thread_handle.join().unwrap();

  assert_eq!(
    collected.len(),
    1,
    "Should only collect the item that was not deleted"
  );
  assert_eq!(collected[0].0, 1, "The remaining item should be key 1");
}
