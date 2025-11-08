mod common;

use std::collections::HashSet;
use std::sync::Arc;

use fibre::spsc;
use futures_util::StreamExt;

use crate::common::build_test_async_cache;

// ================================================================================================
// === Tests for Weak Iterator: `iter_stream`
// ================================================================================================

#[tokio::test]
async fn weak_iter_on_empty_cache() {
  let cache = build_test_async_cache(4);
  assert_eq!(cache.iter_stream().count().await, 0);
}

#[tokio::test]
async fn weak_iter_visits_all_items_multiple_batches() {
  let cache = build_test_async_cache(4);
  let mut expected = HashSet::new();
  for i in 0..100 {
    let value = i.to_string();
    cache.insert(i, value.clone(), 1).await;
    expected.insert((i, Arc::new(value)));
  }
  let collected: HashSet<_> = cache.iter_stream_with_batch_size(10).collect().await;
  assert_eq!(collected, expected);
}

#[tokio::test]
async fn weak_iter_misses_insert_after_shard_scan() {
  let cache = Arc::new(build_test_async_cache(4));
  let (go_tx, go_rx) = spsc::bounded_async(1);

  for i in (0..10).filter(|x| x % 4 == 0) {
    cache.insert(i, i.to_string(), 1).await;
  } // 3 in shard 0
  cache.insert(1, "shard 1".to_string(), 1).await; // 1 in shard 1

  let cache_clone = cache.clone();
  let task_handle = tokio::spawn(async move {
    go_rx.recv().await.unwrap();
    cache_clone.insert(100, "new".to_string(), 1).await; // insert into shard 0
  });

  let mut collected = HashSet::new();
  let mut stream = cache.iter_stream_with_batch_size(3);
  let mut items_seen = 0;
  let mut signaled = false;

  while let Some((key, value)) = stream.next().await {
    items_seen += 1;
    collected.insert((key, value));
    if items_seen >= 3 && !signaled {
      go_tx.send(()).await.unwrap();
      signaled = true;
    }
  }

  // Wait for the spawned task to complete before the test function ends.
  task_handle.await.unwrap();

  // By the time we get here, the insert is guaranteed to have happened.
  assert!(!collected.iter().any(|(k, _)| *k == 100));
  // The collected count should be 4 (3 from shard 0, 1 from shard 1)
  assert_eq!(collected.len(), 4);
}
