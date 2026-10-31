use fibre_cache::{AsyncCache, CacheBuilder, TaskSpawner};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

const LOAD: Duration = Duration::from_secs(30);
const DEADLINE: Duration = Duration::from_secs(5);

fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
  let (tx, rx) = mpsc::channel();
  std::thread::spawn(move || {
    let _ = tx.send(f());
  });
  rx.recv_timeout(DEADLINE).expect("hung past the deadline")
}

fn slow_cache(runtime: &tokio::runtime::Runtime) -> AsyncCache<u64, u64> {
  let _context = runtime.enter();
  CacheBuilder::<u64, u64>::new()
    .async_loader(|k| async move {
      tokio::time::sleep(LOAD).await;
      (k, 1)
    })
    .build_async()
    .unwrap()
}

fn start_runtime() -> tokio::runtime::Runtime {
  tokio::runtime::Runtime::new().unwrap()
}

#[test]
fn test_async_waiter_on_another_runtime_is_released_when_the_load_is_dropped() {
  assert!(within(|| {
    let (loading, waiting) = (start_runtime(), start_runtime());
    let cache = slow_cache(&loading);
    let waiter = waiting.spawn(async move { *cache.fetch_with(&1).await });
    std::thread::sleep(Duration::from_millis(100));
    loading.shutdown_background();
    waiting.block_on(waiter).is_err()
  }));
}

#[test]
fn test_sync_waiter_is_released_when_the_load_is_dropped() {
  assert!(within(|| {
    let loading = start_runtime();
    let cache = slow_cache(&loading).to_sync();
    let waiter = std::thread::spawn(move || *cache.fetch_with(&1));
    std::thread::sleep(Duration::from_millis(100));
    loading.shutdown_background();
    waiter.join().is_err()
  }));
}

#[test]
fn test_every_waiter_on_the_key_is_released_when_the_load_is_dropped() {
  assert_eq!(
    within(|| {
      let (loading, waiting) = (start_runtime(), start_runtime());
      let cache = slow_cache(&loading);
      let waiters: Vec<_> = (0..8)
        .map(|_| {
          let cache = cache.clone();
          waiting.spawn(async move { *cache.fetch_with(&1).await })
        })
        .collect();
      std::thread::sleep(Duration::from_millis(100));
      loading.shutdown_background();
      waiters.into_iter().map(|w| waiting.block_on(w).is_err()).filter(|failed| *failed).count()
    }),
    8
  );
}

#[test]
fn test_a_miss_after_the_runtime_is_gone_panics_for_any_key() {
  assert_eq!(
    within(|| {
      let (loading, waiting) = (start_runtime(), start_runtime());
      let cache = slow_cache(&loading);
      loading.shutdown_background();
      (0..3)
        .map(|k| {
          let cache = cache.clone();
          waiting.block_on(waiting.spawn(async move { *cache.fetch_with(&k).await })).is_err()
        })
        .collect::<Vec<_>>()
    }),
    vec![true; 3]
  );
}

#[test]
fn test_a_hit_still_works_after_the_runtime_is_gone() {
  assert_eq!(
    within(|| {
      let (loading, waiting) = (start_runtime(), start_runtime());
      let cache = slow_cache(&loading);
      waiting.block_on(cache.insert(1, 10, 1));
      loading.shutdown_background();
      *waiting.block_on(cache.fetch_with(&1))
    }),
    10
  );
}

/// Drops every spawned future unpolled while `dropping` is set, and otherwise hands it to a runtime.
struct SwitchSpawner {
  dropping: Arc<AtomicBool>,
  dropped: Arc<AtomicUsize>,
  handle: tokio::runtime::Handle,
}

impl TaskSpawner for SwitchSpawner {
  fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
    if self.dropping.load(Ordering::SeqCst) {
      self.dropped.fetch_add(1, Ordering::SeqCst);
      drop(future);
    } else {
      self.handle.spawn(future);
    }
  }
}

fn switch_cache(runtime: &tokio::runtime::Runtime, dropping: Arc<AtomicBool>, dropped: Arc<AtomicUsize>, ttl: Option<Duration>) -> AsyncCache<u64, u64> {
  let mut builder = CacheBuilder::<u64, u64>::new()
    .spawner(Arc::new(SwitchSpawner { dropping, dropped, handle: runtime.handle().clone() }))
    .janitor_tick_interval(Duration::from_millis(20))
    .async_loader(|k| async move { (k + 1000, 1) });
  if let Some(ttl) = ttl {
    builder = builder.time_to_live(ttl).stale_while_revalidate(Duration::from_secs(60));
  }
  builder.build_async().unwrap()
}

#[test]
fn test_a_key_loads_again_once_the_spawner_runs_tasks() {
  assert_eq!(
    within(|| {
      let runtime = start_runtime();
      let (dropping, dropped) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
      let cache = switch_cache(&runtime, dropping.clone(), dropped, None);
      let first = cache.clone();
      assert!(runtime.block_on(runtime.spawn(async move { *first.fetch_with(&1).await })).is_err());
      dropping.store(false, Ordering::SeqCst);
      *runtime.block_on(cache.fetch_with(&1))
    }),
    1001
  );
}

#[test]
fn test_dropped_stale_refreshes_keep_serving_and_give_back_their_slots() {
  assert_eq!(
    within(|| {
      let runtime = start_runtime();
      let (dropping, dropped) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
      let cache = switch_cache(&runtime, dropping.clone(), dropped.clone(), Some(Duration::from_secs(600)));
      for k in 0..100 {
        runtime.block_on(cache.insert_with_ttl(k, k, 1, Duration::from_millis(50)));
      }
      std::thread::sleep(Duration::from_millis(100));
      for k in 0..100 {
        assert_eq!(*runtime.block_on(cache.fetch_with(&k)), k);
      }
      assert_eq!(dropped.load(Ordering::SeqCst), 100, "a refresh slot was not given back");
      dropping.store(false, Ordering::SeqCst);
      for _ in 0..200 {
        let _ = runtime.block_on(cache.fetch_with(&7));
        if *runtime.block_on(cache.fetch_with(&7)) == 1007 {
          break;
        }
        std::thread::sleep(Duration::from_millis(10));
      }
      *runtime.block_on(cache.fetch_with(&7))
    }),
    1007
  );
}
