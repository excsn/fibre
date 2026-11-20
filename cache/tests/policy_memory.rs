use fibre_cache::CacheBuilder;
use fibre_cache::policy::tinylfu::TinyLfuPolicy;

use parking_lot::Mutex;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
  unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
    LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
    unsafe { System.alloc(layout) }
  }

  unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
    LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
    unsafe { System.alloc_zeroed(layout) }
  }

  unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
    LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    unsafe { System.dealloc(ptr, layout) }
  }

  unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
    unsafe { System.realloc(ptr, layout, new_size) }
  }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The counter is process-wide, so one test measures at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const ENTRY_COST: u64 = 1_024;
const CAPACITY: u64 = 1_024 * ENTRY_COST;
const ENTRIES: u64 = 256;
const BUDGET: isize = 1 << 20;

fn live() -> isize {
  LIVE.load(Ordering::Relaxed)
}

#[test]
fn tinylfu_memory_follows_the_entries_held() {
  let _one = ONE_AT_A_TIME.lock();
  let before = live();
  let cache = CacheBuilder::<u64, u64>::new().capacity(CAPACITY).shards(1).tinylfu().build().unwrap();
  let built = live() - before;
  for key in 0..ENTRIES {
    cache.insert(key, key, ENTRY_COST);
  }
  let held = live() - before;
  assert_eq!(cache.metrics().current_cost, ENTRIES * ENTRY_COST);
  assert!(built < BUDGET, "an empty cache took {built} bytes");
  assert!(held < BUDGET, "a cache of {ENTRIES} entries took {held} bytes");
}

#[test]
fn tinylfu_through_the_factory_memory_follows_the_entries_held() {
  let _one = ONE_AT_A_TIME.lock();
  let before = live();
  let cache = CacheBuilder::<u64, u64>::new()
    .capacity(CAPACITY)
    .shards(1)
    .cache_policy_factory(|| Box::new(TinyLfuPolicy::new(CAPACITY)))
    .build()
    .unwrap();
  let built = live() - before;
  for key in 0..ENTRIES {
    cache.insert(key, key, ENTRY_COST);
  }
  let held = live() - before;
  assert_eq!(cache.metrics().current_cost, ENTRIES * ENTRY_COST);
  assert!(built < BUDGET, "an empty cache took {built} bytes");
  assert!(held < BUDGET, "a cache of {ENTRIES} entries took {held} bytes");
}
