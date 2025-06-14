use std::{sync::Arc, time::Duration};

use fibre::mpsc;
use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy, EvictionReason};

struct TestListener {
  // The sender must be non-async, as on_evict is not async.
  sender: mpsc::BoundedSender<(i32, Arc<String>, EvictionReason)>,
}

impl fibre_cache::EvictionListener<i32, String> for TestListener {
  fn on_evict(&self, key: i32, value: Arc<String>, reason: EvictionReason) {
    // Use blocking_send from the sync context of the Notifier thread.
    self.sender.send((key, value, reason)).unwrap();
  }
}

#[tokio::test]
async fn test_async_listener_for_capacity() {
  let (tx, rx) = mpsc::bounded(10);
  let cache = CacheBuilder::default()
    .capacity(2)
    .shards(1) 
    .cache_policy(LruPolicy::new())
    .eviction_listener(TestListener { sender: tx })
    // Add a fast janitor tick to ensure eviction happens promptly for the test
    .janitor_tick_interval(Duration::from_millis(10))
    .build_async()
    .unwrap();

  cache.insert(1, "one".to_string(), 1).await;
  cache.insert(2, "two".to_string(), 1).await;
  // Access key 1 to make it the most-recently-used.
  // This makes key 2 the unambiguous least-recently-used item.
  cache.get(&1).await;
  // Inserting item 3 will cause the janitor to evict the LRU item (2).
  cache.insert(3, "three".to_string(), 1).await;

  // The test now correctly expects key 2 to be the victim.
  let (key, value, reason) = rx.to_async().recv().await.unwrap();
  assert_eq!(key, 2);
  assert_eq!(*value, "two");
  assert_eq!(reason, EvictionReason::Capacity);
}

#[tokio::test]
async fn test_async_listener_for_invalidation() {
  let (tx, rx) = mpsc::bounded(10);
  let cache = CacheBuilder::default()
    .eviction_listener(TestListener { sender: tx })
    .build_async()
    .unwrap();

  cache.insert(1, "one".to_string(), 1).await;
  assert!(cache.invalidate(&1).await);

  let (key, value, reason) = rx.to_async().recv().await.unwrap();
  assert_eq!(key, 1);
  assert_eq!(*value, "one");
  assert_eq!(reason, EvictionReason::Invalidated);
}

#[tokio::test]
async fn test_async_listener_for_ttl() {
  let (tx, rx) = mpsc::bounded(10);
  let cache = CacheBuilder::default()
    .time_to_live(Duration::from_millis(100))
    .janitor_tick_interval(Duration::from_millis(10))
    .eviction_listener(TestListener { sender: tx })
    .build_async()
    .unwrap();

  cache.insert(1, "one".to_string(), 1).await;
  tokio::time::sleep(Duration::from_millis(250)).await;

  let (key, value, reason) = rx.to_async().recv().await.unwrap();
  assert_eq!(key, 1);
  assert_eq!(*value, "one");
  assert_eq!(reason, EvictionReason::Expired);
}
