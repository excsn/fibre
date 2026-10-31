use fibre_cache::{Cache, CacheBuilder};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
  let (tx, rx) = mpsc::channel();
  std::thread::spawn(move || {
    let _ = tx.send(f());
  });
  rx.recv_timeout(limit).expect("hung past the deadline")
}

fn panics(f: impl FnOnce()) -> bool {
  catch_unwind(AssertUnwindSafe(f)).is_err()
}

#[test]
fn test_sync_fetch_with_without_a_loader_panics() {
  assert!(within(Duration::from_secs(5), || {
    let cache = CacheBuilder::<u64, u64>::new().build().unwrap();
    panics(|| { let _ = cache.fetch_with(&1); })
  }));
}

#[test]
fn test_async_fetch_with_without_a_loader_panics() {
  assert!(within(Duration::from_secs(5), || {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let handle = runtime.spawn(async {
      let cache = CacheBuilder::<u64, u64>::new().build_async().unwrap();
      let _ = cache.fetch_with(&1).await;
    });
    runtime.block_on(handle).is_err()
  }));
}

#[test]
fn test_sync_loader_panic_reaches_the_caller_and_the_key_recovers() {
  within(Duration::from_secs(5), || {
    let fail = Arc::new(AtomicBool::new(true));
    let flag = fail.clone();
    let cache = CacheBuilder::<u64, u64>::new()
      .loader(move |k| {
        if flag.load(Ordering::SeqCst) {
          panic!("loader failed");
        }
        (k, 1)
      })
      .build()
      .unwrap();
    assert!(panics(|| { let _ = cache.fetch_with(&1); }));
    fail.store(false, Ordering::SeqCst);
    assert_eq!(*cache.fetch_with(&1), 1);
  });
}

#[test]
fn test_sync_loader_panic_releases_every_waiter() {
  within(Duration::from_secs(5), || {
    let cache = CacheBuilder::<u64, u64>::new()
      .loader(|_| {
        std::thread::sleep(Duration::from_millis(100));
        panic!("loader failed")
      })
      .build()
      .unwrap();
    let results: Vec<bool> = std::thread::scope(|s| {
      let handles: Vec<_> = (0..4).map(|_| {
        let cache = cache.clone();
        s.spawn(move || panics(|| { let _ = cache.fetch_with(&1); }))
      }).collect();
      handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results, vec![true; 4]);
  });
}

#[test]
fn test_async_loader_panic_reaches_the_caller_and_the_key_recovers() {
  within(Duration::from_secs(5), || {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fail = Arc::new(AtomicBool::new(true));
    let flag = fail.clone();
    let cache = runtime.block_on(async move {
      CacheBuilder::<u64, u64>::new()
        .async_loader(move |k| {
          let failing = flag.load(Ordering::SeqCst);
          async move {
            if failing {
              panic!("loader failed");
            }
            (k, 1)
          }
        })
        .build_async()
        .unwrap()
    });
    let first = cache.clone();
    assert!(runtime.block_on(runtime.spawn(async move { let _ = first.fetch_with(&1).await; })).is_err());
    fail.store(false, Ordering::SeqCst);
    assert_eq!(*runtime.block_on(cache.fetch_with(&1)), 1);
  });
}

#[test]
fn test_sync_loader_on_an_async_cache_panic_reaches_the_caller() {
  within(Duration::from_secs(5), || {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let cache = CacheBuilder::<u64, u64>::new().loader(|_| panic!("loader failed")).build_async().unwrap();
    assert!(runtime.block_on(runtime.spawn(async move { let _ = cache.fetch_with(&1).await; })).is_err());
  });
}

#[test]
fn test_a_loader_may_fetch_another_key() {
  within(Duration::from_secs(5), || {
    let slot: Arc<OnceLock<Cache<u64, u64>>> = Arc::new(OnceLock::new());
    let inner = slot.clone();
    let cache = CacheBuilder::<u64, u64>::new()
      .loader(move |k| if k == 1 { (*inner.get().unwrap().fetch_with(&2), 1) } else { (k, 1) })
      .build()
      .unwrap();
    let _ = slot.set(cache.clone());
    assert_eq!(*cache.fetch_with(&1), 2);
  });
}

#[test]
fn test_a_loader_fetching_its_own_key_panics() {
  assert!(within(Duration::from_secs(5), || {
    let slot: Arc<OnceLock<Cache<u64, u64>>> = Arc::new(OnceLock::new());
    let inner = slot.clone();
    let cache = CacheBuilder::<u64, u64>::new()
      .loader(move |k| (*inner.get().unwrap().fetch_with(&k), 1))
      .build()
      .unwrap();
    let _ = slot.set(cache.clone());
    panics(|| { let _ = cache.fetch_with(&1); })
  }));
}

#[test]
fn test_one_key_loads_once_under_a_herd() {
  let calls = Arc::new(AtomicUsize::new(0));
  let counter = calls.clone();
  let cache = CacheBuilder::<u64, u64>::new()
    .loader(move |k| {
      counter.fetch_add(1, Ordering::SeqCst);
      std::thread::sleep(Duration::from_millis(50));
      (k, 1)
    })
    .build()
    .unwrap();
  std::thread::scope(|s| {
    for _ in 0..8 {
      let cache = cache.clone();
      s.spawn(move || assert_eq!(*cache.fetch_with(&1), 1));
    }
  });
  assert_eq!(calls.load(Ordering::SeqCst), 1);
}

fn stale_cache(live: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Cache<u64, u64> {
  CacheBuilder::<u64, u64>::new()
    .time_to_live(Duration::from_millis(100))
    .stale_while_revalidate(Duration::from_secs(60))
    .janitor_tick_interval(Duration::from_millis(20))
    .loader(move |k| {
      let now = live.fetch_add(1, Ordering::SeqCst) + 1;
      peak.fetch_max(now, Ordering::SeqCst);
      std::thread::sleep(Duration::from_millis(300));
      live.fetch_sub(1, Ordering::SeqCst);
      (k, 1)
    })
    .build()
    .unwrap()
}

#[test]
fn test_stale_refreshes_are_bounded() {
  let (live, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
  let cache = stale_cache(live, peak.clone());
  for k in 0..5000 {
    cache.insert(k, k, 1);
  }
  std::thread::sleep(Duration::from_millis(200));
  for k in 0..5000 {
    assert_eq!(*cache.fetch_with(&k), k);
  }
  assert!(peak.load(Ordering::SeqCst) <= 64, "peak {}", peak.load(Ordering::SeqCst));
}

#[test]
fn test_every_stale_key_is_refreshed_in_turn() {
  let calls = Arc::new(AtomicUsize::new(0));
  let counter = calls.clone();
  let cache = CacheBuilder::<u64, u64>::new()
    .time_to_live(Duration::from_secs(600))
    .stale_while_revalidate(Duration::from_secs(60))
    .janitor_tick_interval(Duration::from_millis(20))
    .loader(move |k| {
      counter.fetch_add(1, Ordering::SeqCst);
      std::thread::sleep(Duration::from_millis(100));
      (k + 1000, 1)
    })
    .build()
    .unwrap();
  for k in 0..200 {
    cache.insert_with_ttl(k, k, 1, Duration::from_millis(100));
  }
  std::thread::sleep(Duration::from_millis(200));
  let mut refreshed = 0;
  for _ in 0..30 {
    refreshed = (0..200).filter(|k| *cache.fetch_with(k) == k + 1000).count();
    if refreshed == 200 {
      break;
    }
    std::thread::sleep(Duration::from_millis(100));
  }
  assert_eq!(refreshed, 200);
}

#[test]
fn test_a_panicking_stale_refresh_leaves_the_stale_value_and_retries() {
  within(Duration::from_secs(10), || {
    let fail = Arc::new(AtomicBool::new(true));
    let flag = fail.clone();
    let cache = CacheBuilder::<u64, u64>::new()
      .time_to_live(Duration::from_millis(100))
      .stale_while_revalidate(Duration::from_secs(60))
      .janitor_tick_interval(Duration::from_millis(20))
      .loader(move |k| {
        if flag.load(Ordering::SeqCst) {
          panic!("loader failed");
        }
        (k + 1, 1)
      })
      .build()
      .unwrap();
    cache.insert(1, 1, 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(*cache.fetch_with(&1), 1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(*cache.fetch_with(&1), 1);
    fail.store(false, Ordering::SeqCst);
    let mut value = 0;
    for _ in 0..50 {
      value = *cache.fetch_with(&1);
      if value == 2 {
        break;
      }
      std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(value, 2);
  });
}
