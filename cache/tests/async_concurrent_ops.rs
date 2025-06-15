use fibre_cache::CacheBuilder;
use std::sync::Arc;
use tokio::sync::Barrier;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_async_concurrent_load_and_invalidate() {
  let cache = Arc::new(
    CacheBuilder::default()
      .capacity(10)
      .async_loader(|key: i32| async move {
        sleep(Duration::from_millis(50)).await;
        (key * 10, 1)
      })
      .build_async()
      .unwrap(),
  );

  let num_loaders = 5;
  let barrier = Arc::new(Barrier::new(num_loaders + 1));
  let mut tasks = vec![];

  // Spawn loader tasks
  for _ in 0..num_loaders {
    let cache_clone = cache.clone();
    let barrier_clone = barrier.clone();
    tasks.push(tokio::spawn(async move {
      barrier_clone.wait().await;
      let value = cache_clone.fetch_with(&1).await;
      assert_eq!(*value, 10);
    }));
  }

  // Spawn invalidator task
  let cache_clone = cache.clone();
  let barrier_clone = barrier.clone();
  tasks.push(tokio::spawn(async move {
    barrier_clone.wait().await;
    cache_clone.invalidate(&1).await;
  }));

  for task in tasks {
    task.await.unwrap();
  }

  let metrics = cache.metrics();
  assert!(metrics.inserts >= 1);
  assert!(metrics.invalidations <= 1);
}

#[tokio::test]
async fn test_async_concurrent_insert_and_clear() {
  let cache = Arc::new(
    CacheBuilder::<i32, i32>::new()
      .capacity(1_000_000)
      .build_async()
      .unwrap(),
  );

  let cache_clone = cache.clone();
  let insert_task = tokio::spawn(async move {
    for i in 0..50_000 {
      cache_clone.insert(i, i, 1).await;
    }
  });

  let cache_clone_2 = cache.clone();
  let clear_task = tokio::spawn(async move {
    sleep(Duration::from_millis(10)).await;
    cache_clone_2.clear().await;
  });

  let _ = tokio::join!(insert_task, clear_task);

  let final_cost = cache.metrics().current_cost;
  assert!(
    final_cost < 1000,
    "Final cost should be very low after clear, but was {}",
    final_cost
  );
}
