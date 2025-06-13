use fibre_cache::{builder::CacheBuilder, policy::lru::Lru, EvictionReason};
// Use a channel that can be sent from a sync thread and received on an async task.
use tokio::sync::mpsc;
use std::{sync::Arc, time::Duration};

struct TestListener {
    // The sender must be non-async, as on_evict is not async.
    sender: mpsc::Sender<(i32, Arc<String>, EvictionReason)>,
}

impl fibre_cache::EvictionListener<i32, String> for TestListener {
    fn on_evict(&self, key: i32, value: Arc<String>, reason: EvictionReason) {
        // Use blocking_send from the sync context of the Notifier thread.
        self.sender.blocking_send((key, value, reason)).unwrap();
    }
}

#[tokio::test]
async fn test_async_listener_for_capacity() {
    let (tx, mut rx) = mpsc::channel(10);
    let cache = CacheBuilder::default()
        .capacity(2)
        .cache_policy(Lru::new())
        .eviction_listener(TestListener { sender: tx })
        .build_async()
        .unwrap();

    cache.insert(1, "one".to_string(), 1).await;
    cache.insert(2, "two".to_string(), 1).await;
    cache.insert(3, "three".to_string(), 1).await;

    let (key, value, reason) = rx.recv().await.unwrap();
    assert_eq!(key, 1);
    assert_eq!(*value, "one");
    assert_eq!(reason, EvictionReason::Capacity);
}

#[tokio::test]
async fn test_async_listener_for_invalidation() {
    let (tx, mut rx) = mpsc::channel(10);
    let cache = CacheBuilder::default()
        .eviction_listener(TestListener { sender: tx })
        .build_async()
        .unwrap();

    cache.insert(1, "one".to_string(), 1).await;
    assert!(cache.invalidate(&1).await);

    let (key, value, reason) = rx.recv().await.unwrap();
    assert_eq!(key, 1);
    assert_eq!(*value, "one");
    assert_eq!(reason, EvictionReason::Invalidated);
}

#[tokio::test]
async fn test_async_listener_for_ttl() {
    let (tx, mut rx) = mpsc::channel(10);
    let cache = CacheBuilder::default()
        .time_to_live(Duration::from_millis(100))
        .janitor_tick_interval(Duration::from_millis(10))
        .eviction_listener(TestListener { sender: tx })
        .build_async()
        .unwrap();

    cache.insert(1, "one".to_string(), 1).await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let (key, value, reason) = rx.recv().await.unwrap();
    assert_eq!(key, 1);
    assert_eq!(*value, "one");
    assert_eq!(reason, EvictionReason::Expired);
}