use fibre_cache::{Cache, CacheBuilder, policy::*};
use std::{
  hash::{BuildHasher, Hash},
  time::{Duration, Instant},
};

const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(2);

// Helper function to wait for the cache cost to be at or below a target.
fn wait_for_cost_convergence<K, V, H>(cache: &Cache<K, V, H>, target_cost: u64)
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
  H: BuildHasher + Clone + Send + Sync,
{
  let deadline = Instant::now() + CONVERGENCE_TIMEOUT;
  loop {
    let current_cost = cache.metrics().current_cost;
    if current_cost <= target_cost {
      return; // Success
    }
    if Instant::now() > deadline {
      panic!(
        "Cache cost did not converge. Current: {}, Target: {}",
        current_cost, target_cost
      );
    }
    std::thread::sleep(Duration::from_millis(50));
  }
}

// --- LRU Policy Tests ---
mod lru {
  use std::time::Duration;

  use super::*;
  use fibre_cache::policy::lru::LruPolicy;
  #[test]
  fn test_lru_eviction_logic() {
    let cache = CacheBuilder::default()
      .capacity(3)
      .shards(1)
      .cache_policy_factory(|| Box::new(LruPolicy::new()))
      .janitor_tick_interval(Duration::from_millis(10))
      .maintenance_chance(1)
      .build()
      .unwrap();

    cache.insert(1, "a", 1);
    cache.insert(2, "b", 1);
    cache.insert(3, "c", 1);
    assert_eq!(cache.metrics().current_cost, 3);

    cache.fetch(&1);

    cache.insert(4, "d", 1);

    wait_for_cost_convergence(&cache, 3);

    assert_eq!(cache.metrics().current_cost, 3);
    assert!(cache.fetch(&2).is_none(), "Key 2 should have been evicted");
    assert!(cache.fetch(&1).is_some());
    assert!(cache.fetch(&3).is_some());
    assert!(cache.fetch(&4).is_some());
  }
}

// --- FIFO Policy Tests ---
mod fifo {
  use std::time::Duration;

  use super::*;
  use fibre_cache::policy::fifo::Fifo;
  #[test]
  fn test_fifo_eviction_logic() {
    let cache = CacheBuilder::default()
      .capacity(3)
      .shards(1)
      .cache_policy_factory(|| Box::new(Fifo::new()))
      .janitor_tick_interval(Duration::from_millis(10))
      .maintenance_chance(1)
      .build()
      .unwrap();

    cache.insert(1, "a", 1);
    cache.insert(2, "b", 1);
    cache.insert(3, "c", 1);

    cache.fetch(&1);

    cache.insert(4, "d", 1);
    std::thread::sleep(Duration::from_millis(50)); // <-- WAIT FOR JANITOR

    assert_eq!(cache.metrics().current_cost, 3);
    assert!(cache.fetch(&1).is_none(), "Key 1 should have been evicted");
    assert!(cache.fetch(&2).is_some());
    assert!(cache.fetch(&3).is_some());
    assert!(cache.fetch(&4).is_some());
  }
}

// --- SIEVE Policy Tests ---
mod sieve {
  use std::time::Duration;

  use super::*;
  use fibre_cache::policy::sieve::SievePolicy;

  #[test]
  fn test_sieve_eviction_logic() {
    let cache = CacheBuilder::default()
      .capacity(3)
      .shards(1)
      .cache_policy_factory(|| Box::new(SievePolicy::new()))
      .janitor_tick_interval(Duration::from_millis(10))
      .build()
      .unwrap();

    cache.insert(1, "a", 1);
    cache.insert(2, "b", 1);
    cache.insert(3, "c", 1);

    cache.fetch(&2);

    cache.insert(4, "d", 1);

    wait_for_cost_convergence(&cache, 3);

    assert_eq!(cache.metrics().current_cost, 3);
    assert!(cache.fetch(&1).is_none(), "Key 1 should be evicted");
    assert!(cache.fetch(&2).is_some(), "Key 2 should be retained");
    assert!(cache.fetch(&3).is_some());
    assert!(cache.fetch(&4).is_some());
  }
}

// --- TinyLFU Policy Tests ---
mod tinylfu {
  use std::time::Duration;

  use super::*;
  use fibre_cache::policy::tinylfu::TinyLfuPolicy;

  // Helper to build a test cache with a fast janitor, which is now
  // essential for testing capacity-based eviction.
  fn build_test_cache(capacity: u64) -> fibre_cache::Cache<i32, String> {
    CacheBuilder::default()
      .capacity(capacity)
      .shards(1)
      // The default policy for a bounded cache is TinyLfu, but we are explicit for clarity.
      .cache_policy_factory(move || Box::new(TinyLfuPolicy::new(capacity)))
      .janitor_tick_interval(Duration::from_millis(10))
      .maintenance_chance(1)
      .build()
      .unwrap()
  }

  #[test]
  fn store_and_retrieve_items() {
    let cache = build_test_cache(10);
    cache.insert(1, "one".to_string(), 1);
    cache.insert(2, "two".to_string(), 1);
    assert_eq!(*cache.fetch(&1).unwrap(), "one");
    assert_eq!(*cache.fetch(&2).unwrap(), "two");
  }

  #[test]
  fn store_retrieve_and_invalidate_items() {
    let cache = build_test_cache(10);
    cache.insert(1, "one".to_string(), 1);
    cache.insert(2, "two".to_string(), 1);
    assert_eq!(*cache.fetch(&1).unwrap(), "one");
    assert_eq!(*cache.fetch(&2).unwrap(), "two");

    cache.invalidate(&1);
    assert_eq!(cache.fetch(&1), None);
    assert_eq!(*cache.fetch(&2).unwrap(), "two");
  }

  #[test]
  fn check_if_cap_and_cost_are_correct() {
    let cache = build_test_cache(3);
    cache.insert(1, "one".to_string(), 1);
    cache.insert(2, "two".to_string(), 1);
    assert_eq!(cache.metrics().current_cost, 2);

    cache.fetch(&1);
    cache.fetch(&2);
    assert_eq!(cache.metrics().current_cost, 2);

    cache.insert(3, "three".to_string(), 1);
    assert_eq!(cache.metrics().current_cost, 3);

    // Inserting a 4th item will push the cache over capacity.
    // The insert itself is non-blocking.
    cache.insert(4, "four".to_string(), 1);

    // Wait for the janitor to run. The janitor will process the write event for key 4,
    // which updates the policy, and then run cleanup_capacity, which calls policy.evict().
    std::thread::sleep(std::time::Duration::from_millis(50));

    assert_eq!(
      cache.metrics().current_cost,
      3,
      "Janitor should bring cost back down to capacity"
    );
  }

  #[test]
  fn clear_cache() {
    let cache = build_test_cache(10);
    cache.insert(1, "one".to_string(), 1);
    cache.insert(2, "two".to_string(), 1);
    assert_eq!(cache.metrics().current_cost, 2);

    cache.clear();
    assert_eq!(cache.fetch(&1), None);
    assert_eq!(cache.fetch(&2), None);
    assert_eq!(cache.metrics().current_cost, 0);
  }

  #[test]
  fn test_admission_logic_rejects_infrequent_item() {
    let cache = build_test_cache(10);

    for i in 1..=10 {
      cache.insert(i, i.to_string(), 1);
    }
    for i in 1..=9 {
      cache.fetch(&i);
    }
    assert_eq!(cache.metrics().current_cost, 10);

    // Wait for the background Janitor to process the read batcher
    // so the CMS knows items 1-9 are actually "hot"
    std::thread::sleep(std::time::Duration::from_millis(50));

    cache.insert(100, "trigger".to_string(), 1);

    wait_for_cost_convergence(&cache, 10);

    let final_cost = cache.metrics().current_cost;
    assert!(
      final_cost <= 10,
      "Janitor should bring cost to at or below capacity. Final cost: {}",
      final_cost
    );
    assert!(
      cache.fetch(&10).is_none(),
      "Infrequent item (10) should be evicted"
    );
    assert!(
      cache.fetch(&1).is_some(),
      "Hot item (1) should have been protected"
    );
    assert!(
      cache.fetch(&100).is_some(),
      "New trigger item (100) should now be in the cache"
    );
  }

  #[test]
  fn test_admission_logic_admits_frequent_item() {
    let cache = build_test_cache(10);

    for i in 1..=10 {
      cache.insert(i, i.to_string(), 1);
    }
    assert_eq!(cache.metrics().current_cost, 10);

    for _ in 0..10 {
      cache.fetch(&5);
    }

    // Wait for the batcher to drain so item 5 becomes "hot" in the sketch
    std::thread::sleep(std::time::Duration::from_millis(50));

    cache.insert(11, "new".to_string(), 1);

    wait_for_cost_convergence(&cache, 10);

    assert_eq!(
      cache.metrics().current_cost,
      10,
      "Janitor should bring cost back to capacity"
    );
    assert!(
      cache.fetch(&5).is_some(),
      "Hot item 5 should have been protected"
    );
    assert!(
      cache.fetch(&1).is_none(),
      "Weak, cold victim (1) should be evicted by policy"
    );
    assert!(
      cache.fetch(&11).is_some(),
      "New item 11 should be in the cache"
    );
  }
}

// --- SLRU Policy Tests ---
mod slru {
  use std::time::Duration;

  use super::*;
  use fibre_cache::policy::slru::SlruPolicy;

  #[test]
  fn test_slru_promotion_and_eviction() {
    // Capacity 4: The SLRU policy internally splits this.
    let cache = CacheBuilder::default()
      .capacity(4)
      .shards(1)
      .cache_policy_factory(|| Box::new(SlruPolicy::new(4)))
      .janitor_tick_interval(Duration::from_millis(10))
      .build()
      .unwrap();

    // 1. Insert 4 items. They all go into the policy's probationary tracking.
    // Policy prob: [4, 3, 2, 1]
    cache.insert(1, "a".to_string(), 1);
    cache.insert(2, "b".to_string(), 1);
    cache.insert(3, "c".to_string(), 1);
    cache.insert(4, "d".to_string(), 1);

    assert_eq!(cache.metrics().current_cost, 4);

    // 2. Access item 2. The policy should promote it to the protected segment.
    // Policy prob: [4, 3, 1], Policy prot: [2]
    cache.fetch(&2);

    // 3. Insert item 5. This will exceed total capacity.
    // The insert itself is non-blocking and pushes the cost to 5.
    cache.insert(5, "e".to_string(), 1);
    assert_eq!(
      cache.metrics().current_cost,
      5,
      "Cache should be temporarily over capacity"
    );

    // Wait for the janitor to run and perform eviction.
    wait_for_cost_convergence(&cache, 4);

    // The Janitor will have called policy.evict(1).
    // The SLRU policy evicts from the LRU end of the probationary segment, which is item 1.
    assert_eq!(
      cache.metrics().current_cost,
      4,
      "Cache cost should be back to capacity after janitor runs"
    );

    assert!(
      cache.fetch(&1).is_none(),
      "LRU of probationary (1) should be evicted"
    );
    assert!(
      cache.fetch(&2).is_some(),
      "Promoted item (2) should be safe"
    );
    assert!(cache.fetch(&3).is_some());
    assert!(cache.fetch(&4).is_some());
    assert!(cache.fetch(&5).is_some(), "New item (5) should be present");
  }
}
