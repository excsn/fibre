use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use tokio::time::{sleep, Duration};

// A simulated database or slow external service.
async fn fetch_from_database(key: i32, load_count: Arc<AtomicUsize>) -> (String, u64) {
  println!(
    "--- Database: Received request for key {}. Simulating slow query...",
    key
  );
  load_count.fetch_add(1, Ordering::SeqCst);
  sleep(Duration::from_millis(500)).await;
  let value = format!("value_for_{}", key);
  println!("--- Database: Responding with '{}' for key {}.", value, key);
  (value, 1) // Return the value and its cost
}

#[tokio::main]
async fn main() {
  let load_counter = Arc::new(AtomicUsize::new(0));

  // Create an async cache with an async loader function.
  let cache = Arc::new(CacheBuilder::default()
    .capacity(10)
    .async_loader({
      let counter = load_counter.clone();
      move |key: i32| {
        // The closure must return a pinned, boxed future.
        let fut = fetch_from_database(key, counter.clone());
        Box::pin(fut)
      }
    })
    .build_async()
    .expect("Failed to build async cache"));

  println!("--- Thundering Herd Demonstration ---");
  println!("Spawning 10 tasks to request the same key '42' at once.\n");

  let mut tasks = Vec::new();
  for i in 0..10 {
    let cache_clone = cache.clone();
    tasks.push(tokio::spawn(async move {
      println!("[Task {}] Requesting key 42...", i);
      let value = cache_clone.get_with(&42).await;
      println!("[Task {}] Received value: {}", i, value);
      assert_eq!(*value, "value_for_42");
    }));
  }

  // Wait for all tasks to complete.
  for task in tasks {
    task.await.unwrap();
  }

  println!("\n--- Verification ---");
  println!("All 10 tasks completed.");
  println!(
    "Database function was called {} time(s).",
    load_counter.load(Ordering::SeqCst)
  );
  assert_eq!(load_counter.load(Ordering::SeqCst), 1);
  println!("Thundering herd protection worked successfully!");

  println!("\n--- Second Request ---");
  println!("Requesting key 42 again. This should now be a cache hit.");
  let value = cache.get_with(&42).await;
  println!("Received value: {}", value);
  println!(
    "Database function was called {} time(s) in total.",
    load_counter.load(Ordering::SeqCst)
  );
  assert_eq!(load_counter.load(Ordering::SeqCst), 1);

  println!("\nCache metrics: {:#?}", cache.metrics());
}
