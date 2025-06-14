use fibre_cache::{
  builder::CacheBuilder, policy::sieve::SievePolicy, EvictionListener, EvictionReason,
};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// A simple listener that just prints evicted entries.
struct MyListener;

impl EvictionListener<i32, String> for MyListener {
  fn on_evict(&self, key: i32, value: Arc<String>, reason: EvictionReason) {
    println!(
      "[Listener] Item evicted! Key: {}, Value: '{}', Reason: {}",
      key, value, reason
    );
  }
}

fn main() {
  println!("--- Cache with Custom Policy (Sieve) and Eviction Listener ---");

  let cache = CacheBuilder::default()
    .capacity(3) // A small capacity to easily trigger evictions
    .shards(1)
    .cache_policy(SievePolicy::new())
    .eviction_listener(MyListener)
    .janitor_tick_interval(Duration::from_millis(50))
    .build()
    .expect("Failed to build cache");

  println!("Cache created with SievePolicy and capacity 3.");

  // Insert 3 items. Cache is now full.
  cache.insert(1, "one".to_string(), 1);
  cache.insert(2, "two".to_string(), 1);
  cache.insert(3, "three".to_string(), 1);
  println!("\nInserted keys 1, 2, 3. Cache is full.");
  println!("Current cost: {}", cache.metrics().current_cost);

  // Access key 1. In Sieve, this sets its 'visited' bit, giving it a second chance.
  println!("\nAccessing key 1 to mark it as 'visited'...");
  cache.get(&1);

  // Insert a 4th item. This will push the cache over capacity.
  println!("\nInserting key 4. This will trigger an eviction.");
  cache.insert(4, "four".to_string(), 1);

  // Wait for the janitor and notifier to run.
  thread::sleep(Duration::from_millis(200));

  println!("\n--- Final State ---");
  // Sieve eviction starts from the oldest, unvisited item.
  // Order of insertion: 1, 2, 3. Oldest is 1.
  // But 1 was visited, so it is spared.
  // The next oldest is 2, which is unvisited. Key 2 should be the victim.
  assert!(
    cache.get(&1).is_some(),
    "Key 1 should be present (was visited)"
  );
  assert!(cache.get(&2).is_none(), "Key 2 should have been evicted");
  assert!(cache.get(&3).is_some());
  assert!(cache.get(&4).is_some());

  println!("Final cost: {}", cache.metrics().current_cost);
  println!("\nCache metrics: {:#?}", cache.metrics());
}
