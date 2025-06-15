use fibre_cache::{builder::CacheBuilder, policy::null::NullPolicy, AsyncCache, AsyncEntry};
use std::sync::Arc;

// Helper to create a new async cache with a NullPolicy for testing.
fn new_test_cache(capacity: u64) -> AsyncCache<String, i32> {
  CacheBuilder::<String, i32>::new()
    .capacity(capacity)
    .cache_policy_factory(|| Box::new(NullPolicy))
    .build_async()
    .unwrap()
}

#[tokio::test]
async fn test_async_new_fetch_with_closure() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1).await;

  // Test get hit, extracts length of string representation
  let value_len = cache
    .get(&"key1".to_string(), |v| v.to_string().len())
    .await;
  assert_eq!(value_len, Some(2)); // "10" has length 2

  // Test get miss
  let miss_result = cache
    .get(&"non-existent".to_string(), |v| v.to_string().len())
    .await;
  assert!(miss_result.is_none());

  let metrics = cache.metrics();
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
}

#[tokio::test]
async fn test_async_insert_and_fetch() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1).await;

  // Test get hit
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(10)));

  // Test get miss
  assert!(cache.fetch(&"non-existent".to_string()).await.is_none());

  let metrics = cache.metrics();
  assert_eq!(metrics.inserts, 1);
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
  assert_eq!(metrics.current_cost, 1);
}

#[tokio::test]
async fn test_async_invalidate_and_clear() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1).await;
  cache.insert("key2".to_string(), 20, 2).await;

  // Test invalidate
  assert!(cache.invalidate(&"key1".to_string()).await);
  assert!(
    !cache.invalidate(&"key1".to_string()).await,
    "Double invalidate should fail"
  );
  assert!(cache.fetch(&"key1".to_string()).await.is_none());
  assert_eq!(cache.metrics().invalidations, 1);
  assert_eq!(
    cache.metrics().current_cost,
    2,
    "Cost of key2 should remain"
  );

  // Test clear
  cache.clear().await;
  assert!(cache.fetch(&"key2".to_string()).await.is_none());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[tokio::test]
async fn test_async_replacement() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1).await;
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(10)));
  assert_eq!(cache.metrics().current_cost, 1);

  // Replace with new value and cost
  cache.insert("key1".to_string(), 20, 5).await;
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(20)));
  assert_eq!(
    cache.metrics().current_cost,
    5,
    "Cost should be updated to 5"
  );
  assert_eq!(
    cache.metrics().inserts,
    2,
    "Replacement counts as a second insert"
  );
}

#[tokio::test]
async fn test_async_entry_api() {
  let cache = new_test_cache(100);

  // Test Vacant entry
  if let AsyncEntry::Vacant(entry) = cache.entry("key1".to_string()).await {
    // insert is sync as it already holds the lock
    entry.insert(100, 10);
  } else {
    panic!("Entry should be vacant");
  }
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(100)));
  assert_eq!(cache.metrics().inserts, 1);
  assert_eq!(cache.metrics().current_cost, 10);

  // Test Occupied entry
  if let AsyncEntry::Occupied(entry) = cache.entry("key1".to_string()).await {
    assert_eq!(entry.key(), "key1");
    assert_eq!(*entry.get(), 100);
  } else {
    panic!("Entry should be occupied");
  }
  // Accessing via entry does not count as a 'get' hit/miss
  assert_eq!(cache.metrics().hits, 1);
}

#[tokio::test]
async fn test_async_compute() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 50, 1).await;

  // Test successful compute
  let was_computed = cache.try_compute(&"key1".to_string(), |v| *v *= 2).await;
  assert!(was_computed);
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(100)));
  assert_eq!(cache.metrics().updates, 1);

  // Test failed compute (due to another Arc existing)
  let external_arc = cache.fetch(&"key1".to_string()).await.unwrap();
  let was_computed_again = cache.try_compute(&"key1".to_string(), |v| *v *= 2).await;
  assert!(!was_computed_again);
  assert_eq!(cache.fetch(&"key1".to_string()).await, Some(Arc::new(100))); // Value is unchanged
  assert_eq!(cache.metrics().updates, 1); // Metric is unchanged
  drop(external_arc);

  // Test compute on non-existent key
  let was_computed_miss = cache
    .try_compute(&"non-existent".to_string(), |v| *v *= 2)
    .await;
  assert!(!was_computed_miss);
}

#[tokio::test]
async fn test_async_handle_conversion() {
  let cache = new_test_cache(100);
  cache.insert("shared_key".to_string(), 999, 1).await;

  // Convert to sync and check if the state is shared
  let sync_handle = cache.to_sync();
  assert_eq!(
    sync_handle.fetch(&"shared_key".to_string()),
    Some(Arc::new(999))
  );

  // Invalidate from the sync handle
  sync_handle.invalidate(&"shared_key".to_string());

  // Check from the original async handle that it's gone
  assert!(sync_handle.fetch(&"shared_key".to_string()).is_none());
}

#[tokio::test]
async fn test_async_compute_waits_and_succeeds() {
  let cache = Arc::new(new_test_cache(100));
  cache.insert("key1".to_string(), 50, 1).await;

  // Hold an Arc, which would make try_compute fail.
  let external_arc = cache.fetch(&"key1".to_string()).await.unwrap();

  let cache_clone = cache.clone();
  let compute_task = tokio::spawn(async move {
    // This will loop, yielding until the external_arc is dropped.
    cache_clone.compute(&"key1".to_string(), |v| *v += 1).await;
  });

  // Yield to let the compute task start and loop at least once.
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;

  // Drop our external handle, unblocking the compute task.
  drop(external_arc);

  // Wait for the task to complete.
  compute_task.await.unwrap();

  // Verify the final value.
  let final_value = cache.fetch(&"key1".to_string()).await.unwrap();
  assert_eq!(*final_value, 51);
}
