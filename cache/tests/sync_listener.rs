use fibre_cache::{builder::CacheBuilder, policy::lru::LruPolicy, EvictionReason};
use std::{
  sync::{mpsc, Arc},
  thread,
  time::Duration,
};

// Use std::sync::mpsc for synchronous tests. It's simpler and clearer.
struct TestListener {
  sender: mpsc::Sender<(i32, Arc<String>, EvictionReason)>,
}

impl fibre_cache::EvictionListener<i32, String> for TestListener {
  fn on_evict(&self, key: i32, value: Arc<String>, reason: EvictionReason) {
    // Use a blocking send. This will wait until the receiver is ready.
    // This is safe because the Notifier thread has no other locks.
    self.sender.send((key, value, reason)).unwrap();
  }
}

#[test]
fn test_sync_listener_for_capacity() {
  let (tx, rx) = mpsc::channel();
  let cache = CacheBuilder::default()
    .capacity(2)
    .shards(1)
    .cache_policy_factory(|| Box::new(LruPolicy::new()))
    .eviction_listener(TestListener { sender: tx })
    .build()
    .unwrap();

  cache.insert(1, "one".to_string(), 1);
  cache.insert(2, "two".to_string(), 1);
  cache.insert(3, "three".to_string(), 1);

  let (key, value, reason) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
  assert_eq!(key, 1);
  assert_eq!(*value, "one");
  assert_eq!(reason, EvictionReason::Capacity);
}

#[test]
fn test_sync_listener_for_invalidation() {
  let (tx, rx) = mpsc::channel();
  let cache = CacheBuilder::default()
    .eviction_listener(TestListener { sender: tx })
    .build()
    .unwrap();

  cache.insert(1, "one".to_string(), 1);
  assert!(cache.invalidate(&1));

  let (key, value, reason) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
  assert_eq!(key, 1);
  assert_eq!(*value, "one");
  assert_eq!(reason, EvictionReason::Invalidated);
}

#[test]
fn test_sync_listener_for_ttl() {
  let (tx, rx) = mpsc::channel();
  let cache = CacheBuilder::default()
    .time_to_live(Duration::from_millis(100))
    .janitor_tick_interval(Duration::from_millis(10))
    .eviction_listener(TestListener { sender: tx })
    .build()
    .unwrap();

  cache.insert(1, "one".to_string(), 1);
  thread::sleep(Duration::from_millis(250));

  // The janitor will trigger the on_evict, which blocks until we call recv.
  let (key, value, reason) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
  assert_eq!(key, 1);
  assert_eq!(*value, "one");
  assert_eq!(reason, EvictionReason::Expired);
}
