use fibre_cache::{builder::CacheBuilder, policy::null::NullPolicy, Cache, Entry};
use std::sync::Arc;

// Helper to create a new cache with a NullPolicy for testing.
fn new_test_cache(capacity: u64) -> Cache<String, i32> {
  CacheBuilder::<String, i32>::new()
    .capacity(capacity)
    .cache_policy_factory(|| Box::new(NullPolicy))
    .build()
    .unwrap()
}

#[test]
fn test_sync_new_fetch_with_closure() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1);

  // Test get hit, extracts length of string representation
  let value_len = cache.get(&"key1".to_string(), |v| v.to_string().len());
  assert_eq!(value_len, Some(2)); // "10" has length 2

  // Test get miss
  let miss_result = cache.get(&"non-existent".to_string(), |v| v.to_string().len());
  assert!(miss_result.is_none());

  let metrics = cache.metrics();
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
}

#[test]
fn test_sync_insert_and_fetch() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1);

  // Test get hit
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(10)));

  // Test get miss
  assert!(cache.fetch(&"non-existent".to_string()).is_none());

  let metrics = cache.metrics();
  assert_eq!(metrics.inserts, 1);
  assert_eq!(metrics.hits, 1);
  assert_eq!(metrics.misses, 1);
  assert_eq!(metrics.current_cost, 1);
}

#[test]
fn test_sync_invalidate_and_clear() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1);
  cache.insert("key2".to_string(), 20, 2);

  // Test invalidate
  assert!(cache.invalidate(&"key1".to_string()));
  assert!(
    !cache.invalidate(&"key1".to_string()),
    "Double invalidate should fail"
  );
  assert!(cache.fetch(&"key1".to_string()).is_none());
  assert_eq!(cache.metrics().invalidations, 1);
  assert_eq!(
    cache.metrics().current_cost,
    2,
    "Cost of key2 should remain"
  );

  // Test clear
  cache.clear();
  assert!(cache.fetch(&"key2".to_string()).is_none());
  assert_eq!(cache.metrics().current_cost, 0);
}

#[test]
fn test_sync_replacement() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 10, 1);
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(10)));
  assert_eq!(cache.metrics().current_cost, 1);

  // Replace with new value and cost
  cache.insert("key1".to_string(), 20, 5);
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(20)));
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

#[test]
fn test_sync_entry_api() {
  let cache = new_test_cache(100);

  // Test Vacant entry
  if let Entry::Vacant(entry) = cache.entry("key1".to_string()) {
    entry.insert(100, 10);
  } else {
    panic!("Entry should be vacant");
  }
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(100)));
  assert_eq!(cache.metrics().inserts, 1);
  assert_eq!(cache.metrics().current_cost, 10);

  // Test Occupied entry
  if let Entry::Occupied(entry) = cache.entry("key1".to_string()) {
    assert_eq!(entry.key(), "key1");
    assert_eq!(*entry.get(), 100);
  } else {
    panic!("Entry should be occupied");
  }
  // Accessing via entry does not count as a 'get' hit/miss
  assert_eq!(cache.metrics().hits, 1);
}

#[test]
fn test_sync_compute() {
  let cache = new_test_cache(100);
  cache.insert("key1".to_string(), 50, 1);

  // Test successful compute
  let was_computed = cache.try_compute(&"key1".to_string(), |v| *v *= 2);
  assert_eq!(was_computed, Some(true));
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(100)));
  assert_eq!(cache.metrics().updates, 1);

  // Test failed compute (due to another Arc existing)
  let external_arc = cache.fetch(&"key1".to_string()).unwrap();
  let was_computed_again = cache.try_compute(&"key1".to_string(), |v| *v *= 2);
  assert_eq!(was_computed_again, Some(false));
  assert_eq!(cache.fetch(&"key1".to_string()), Some(Arc::new(100))); // Value is unchanged
  assert_eq!(cache.metrics().updates, 1); // Metric is unchanged
  drop(external_arc);

  // Test compute on non-existent key
  let was_computed_miss = cache.try_compute(&"non-existent".to_string(), |v| *v *= 2);
  assert_eq!(was_computed_miss, None);
}

#[test]
fn test_sync_handle_conversion() {
  let cache = new_test_cache(100);
  cache.insert("shared_key".to_string(), 999, 1);

  // Convert to async and check if the state is shared
  let async_handle = cache.to_async();

  // We must now block on the `async` get method to get its result.
  let value = futures_executor::block_on(async_handle.fetch(&"shared_key".to_string()));
  assert_eq!(value, Some(Arc::new(999)));

  // Invalidate from the async handle (must block to await the future)
  futures_executor::block_on(async_handle.invalidate(&"shared_key".to_string()));

  // Check from the original sync handle that it's gone
  assert!(cache.fetch(&"shared_key".to_string()).is_none());
}

#[test]
fn test_sync_compute_waits_and_succeeds() {
  let cache = Arc::new(new_test_cache(100));
  cache.insert("key1".to_string(), 50, 1);

  // Hold an Arc to the value, which would make try_compute fail.
  let external_arc = cache.fetch(&"key1".to_string()).unwrap();
  assert_eq!(*external_arc, 50);

  let cache_clone = cache.clone();
  let compute_handle = std::thread::spawn(move || {
    // This will loop, yielding until the external_arc is dropped.
    cache_clone.compute(&"key1".to_string(), |v| *v += 1);
  });

  // Give the compute thread a moment to start and begin looping.
  std::thread::sleep(std::time::Duration::from_millis(10));

  // Now, drop our external handle. This should unblock the compute thread.
  drop(external_arc);

  // Wait for the compute thread to finish. If it deadlocks, this will hang.
  compute_handle.join().unwrap();

  // Verify the value was updated.
  let final_value = cache.fetch(&"key1".to_string()).unwrap();
  assert_eq!(*final_value, 51);
}
