mod common;

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Barrier;

use crate::common::build_test_async_cache;

// ================================================================================================
// === Tests for Shard-Snapshot Iterator: `iter_snapshot_async`
// ================================================================================================

#[tokio::test]
async fn snapshot_iter_visits_all_items() {
  let cache = build_test_async_cache(4);
  let mut expected = HashSet::new();
  for i in 0..100 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1).await;
    expected.insert((i, Arc::new(value)));
  }
  let mut collected = HashSet::new();
  let mut iter = cache.iter_snapshot_async();
  while let Some(item) = iter.next().await {
    collected.insert(item);
  }
  assert_eq!(collected, expected);
}

#[tokio::test]
async fn snapshot_iter_misses_insert_after_shard_scan() {
  let cache = Arc::new(build_test_async_cache(4));
  cache.insert(0, "a".to_string(), 1).await; // Shard 0
  cache.insert(2, "b".to_string(), 1).await; // Shard 2

  let barrier = Arc::new(Barrier::new(2));
  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();

  let task_handle = tokio::spawn(async move {
    barrier_clone.wait().await;
    // This insert happens after the main task's iterator has snapshotted shard 0.
    cache_clone.insert(4, "new".to_string(), 1).await; // Also in shard 0
  });

  let mut collected = Vec::new();
  let mut iter = cache.iter_snapshot_async();

  // Process first item, ensuring shard 0 is snapshotted
  if let Some(item) = iter.next().await {
    collected.push(item);
  }
  // Sync with the writer task to let it perform the insert.
  barrier.wait().await;

  // Continue iterating
  while let Some(item) = iter.next().await {
    collected.push(item);
  }

  task_handle.await.unwrap();
  assert_eq!(collected.len(), 2, "Should only see the original 2 items");
  assert!(!collected.iter().any(|(k, _)| *k == 4));
}

#[tokio::test]
async fn snapshot_iter_sees_insert_before_shard_scan() {
  let cache = Arc::new(build_test_async_cache(4));
  cache.insert(0, "a".to_string(), 1).await;

  let barrier = Arc::new(Barrier::new(2));
  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();

  let task_handle = tokio::spawn(async move {
    barrier_clone.wait().await;
    // This insert happens before the main iterator gets to shard 2.
    cache_clone.insert(2, "new".to_string(), 1).await;
  });

  let mut collected = Vec::new();
  let mut iter = cache.iter_snapshot_async();

  // Process first item from shard 0.
  if let Some(item) = iter.next().await {
    collected.push(item);
  }
  // Sync with writer, allowing it to insert into shard 2.
  barrier.wait().await;

  // Continue. The iterator will now scan shard 1 (empty) then shard 2.
  while let Some(item) = iter.next().await {
    collected.push(item);
  }

  task_handle.await.unwrap();
  assert_eq!(
    collected.len(),
    2,
    "Should see the original and the new item"
  );
  assert!(collected.iter().any(|(k, _)| *k == 2));
}

#[tokio::test]
async fn snapshot_iter_skips_deleted_item() {
  let cache = Arc::new(build_test_async_cache(4));
  cache.insert(0, "a".to_string(), 1).await;
  cache.insert(1, "b".to_string(), 1).await;

  let barrier = Arc::new(Barrier::new(2));
  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();

  let task_handle = tokio::spawn(async move {
    barrier_clone.wait().await;
    // Delete an item the iterator has already snapshotted.
    cache_clone.invalidate(&0).await;
  });

  let mut collected = Vec::new();
  let mut iter = cache.iter_snapshot_async();

  // Calling next() snapshots shard 0. We don't consume the result yet.
  // To be precise, we need to know the implementation of next(), which
  // snapshots and then fetches. Let's signal *before* the first call.
  barrier.wait().await;

  // The iterator will snapshot key 0, but by the time it calls fetch(), it will be gone.
  while let Some(item) = iter.next().await {
    collected.push(item);
  }

  task_handle.await.unwrap();
  assert_eq!(
    collected.len(),
    1,
    "Should only collect the item that was not deleted"
  );
  assert_eq!(collected[0].0, 1, "The remaining item should be key 1");
}
