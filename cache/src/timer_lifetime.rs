//! A TTL timer exists exactly while its entry is in the cache, whichever path
//! put the entry in or took it out.

use crate::{Cache, CacheBuilder};
use std::time::Duration;

const TTL: Duration = Duration::from_secs(3600);

fn timers(cache: &Cache<u64, u64>) -> usize {
  cache.shared.store.shards.iter().map(|s| s.map.read().timer_count()).sum()
}

fn entries(cache: &Cache<u64, u64>) -> usize {
  cache.shared.store.shards.iter().map(|s| s.map.read().len()).sum()
}

#[test]
fn capacity_eviction_drops_the_evicted_entries_timers() {
  let cache = CacheBuilder::<u64, u64>::new().capacity(1000).time_to_live(TTL).build().unwrap();
  for k in 0..20_000 {
    cache.insert(k, k, 1);
  }
  assert_eq!(timers(&cache), entries(&cache));
}

#[test]
fn clear_drops_every_timer() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_live(TTL).build().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1);
  }
  cache.clear();
  assert_eq!(timers(&cache), 0);
}

#[test]
fn entry_api_inserts_are_armed() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_live(TTL).build().unwrap();
  for k in 0..100 {
    cache.entry(k).or_insert(k, 1);
  }
  assert_eq!(entries(&cache), 100);
  assert_eq!(timers(&cache), 100);
}

#[test]
fn loader_inserts_are_armed() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_live(TTL).loader(|k| (k, 1)).build().unwrap();
  for k in 0..100 {
    cache.fetch_with(&k);
  }
  assert_eq!(entries(&cache), 100);
  assert_eq!(timers(&cache), 100);
}

#[test]
fn replacing_and_invalidating_keep_one_timer_per_entry() {
  let cache = CacheBuilder::<u64, u64>::new().time_to_live(TTL).loader(|k| (k, 1)).build().unwrap();
  for k in 0..100 {
    cache.insert(k, k, 1);
    cache.insert(k, k + 1, 1);
    cache.insert_with_ttl(k, k + 2, 1, TTL);
  }
  for k in 0..50 {
    cache.invalidate(&k);
  }
  assert_eq!(entries(&cache), 50);
  assert_eq!(timers(&cache), 50);
}

#[test]
fn an_entry_in_its_grace_period_stays_with_its_timer() {
  let cache = CacheBuilder::<u64, u64>::new()
    .time_to_live(Duration::from_millis(50))
    .stale_while_revalidate(TTL)
    .timer_mode(crate::builder::TimerWheelMode::HighPrecisionShortLived)
    .janitor_tick_interval(Duration::from_millis(10))
    .build()
    .unwrap();
  for k in 0..100 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(Duration::from_millis(300));
  assert_eq!(entries(&cache), 100);
  assert_eq!(timers(&cache), 100);
}


#[test]
fn a_cache_with_no_ttl_starts_no_janitor_until_an_entry_needs_one() {
  use std::sync::atomic::Ordering;
  let cache = CacheBuilder::<u64, u64>::new().build().unwrap();
  for k in 0..1000 {
    cache.insert(k, k, 1);
  }
  assert!(!cache.shared.janitor_running.load(Ordering::Relaxed));
  assert_eq!(timers(&cache), 0);
  cache.insert_with_ttl(1, 1, 1, TTL);
  assert!(cache.shared.janitor_running.load(Ordering::Relaxed));
  assert_eq!(timers(&cache), 1);
}
