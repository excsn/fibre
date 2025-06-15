use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use tokio::sync::Barrier;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_async_loader_basic() {
  let load_count = Arc::new(AtomicUsize::new(0));

  let cache = CacheBuilder::default()
    .capacity(10)
    .async_loader({
      let load_count = load_count.clone();
      move |key: i32| {
        let load_count = load_count.clone();
        async move {
          load_count.fetch_add(1, Ordering::SeqCst);
          (key * 10, 1)
        }
      }
    })
    .build_async()
    .unwrap();

  // 1. First call to fetch_with_async on a missing key.
  let value = cache.fetch_with(&5).await;
  assert_eq!(*value, 50);
  assert_eq!(load_count.load(Ordering::SeqCst), 1);
  assert_eq!(cache.metrics().misses, 1);
  assert_eq!(cache.metrics().inserts, 1);

  // 2. Second call on the same key.
  let value = cache.fetch_with(&5).await;
  assert_eq!(*value, 50);
  assert_eq!(
    load_count.load(Ordering::SeqCst),
    1,
    "Loader should not be called again"
  );
  assert_eq!(cache.metrics().hits, 1);
}

#[tokio::test]
async fn test_async_loader_thundering_herd() {
  let load_count = Arc::new(AtomicUsize::new(0));
  let num_tasks = 20;

  let cache = Arc::new(
    CacheBuilder::default()
      .capacity(10)
      .async_loader({
        let load_count = load_count.clone();
        move |key: i32| {
          let load_count = load_count.clone();
          async move {
            // Simulate a slow database call or computation
            sleep(Duration::from_millis(100)).await;
            load_count.fetch_add(1, Ordering::SeqCst);
            (key * 10, 1)
          }
        }
      })
      .build_async()
      .unwrap(),
  );

  let barrier = Arc::new(Barrier::new(num_tasks));
  let mut tasks = vec![];

  for _ in 0..num_tasks {
    let cache_clone = cache.clone();
    let barrier_clone = barrier.clone();
    tasks.push(tokio::spawn(async move {
      // Wait for all tasks to be ready
      barrier_clone.wait().await;
      // All tasks request the same missing key at once
      let value = cache_clone.fetch_with(&99).await;
      assert_eq!(*value, 990);
    }));
  }

  // Wait for all tasks to complete
  for task in tasks {
    task.await.unwrap();
  }

  assert_eq!(
    load_count.load(Ordering::SeqCst),
    1,
    "Thundering herd protection failed: loader was called more than once"
  );
  assert_eq!(
    cache.metrics().misses,
    1,
    "There should be only one initial miss"
  );
  assert_eq!(cache.metrics().hits, (num_tasks - 1) as u64);
}
