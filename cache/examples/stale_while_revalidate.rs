use fibre_cache::CacheBuilder;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
struct Data {
  version: usize,
  content: String,
}

fn main() {
  let load_counter = Arc::new(AtomicUsize::new(0));

  let cache = CacheBuilder::default()
    .time_to_live(Duration::from_secs(2))
    .stale_while_revalidate(Duration::from_secs(10)) // 10s grace period
    .loader({
      let counter = load_counter.clone();
      move |key: String| {
        let version = counter.fetch_add(1, Ordering::SeqCst) + 1;
        println!("[Loader] Loading version {} for key '{}'...", version, key);
        thread::sleep(Duration::from_millis(500)); // Simulate slow load
        (
          Arc::new(Data {
            version,
            content: format!("Content for {} - version {}", key, version),
          }),
          1, // cost
        )
      }
    })
    .build()
    .unwrap();

  let key = "my-data".to_string();

  println!("--- Step 1: Initial Load ---");
  let value1 = cache.fetch_with(&key);
  println!("Received: {:?}", *value1);
  assert_eq!(value1.version, 1);

  println!("\n--- Step 2: Cache Hit (Fresh) ---");
  let value2 = cache.fetch_with(&key);
  println!("Received: {:?}", *value2);
  assert_eq!(value2.version, 1);
  println!("Loader calls: {}", load_counter.load(Ordering::Relaxed));
  assert_eq!(load_counter.load(Ordering::Relaxed), 1);

  println!("\n--- Step 3: Wait for TTL to expire (3 seconds) ---");
  thread::sleep(Duration::from_secs(3));

  println!("\n--- Step 4: Stale Read ---");
  println!(
    "Requesting key '{}'. It's stale, but within the grace period.",
    key
  );
  let value3 = cache.fetch_with(&key);
  println!("IMMEDIATELY Received (stale): {:?}", *value3);
  assert_eq!(
    value3.version, 1,
    "Should return stale version 1 immediately"
  );

  println!("A background refresh has been triggered.");
  println!(
    "Loader calls (may still be 1): {}",
    load_counter.load(Ordering::Relaxed)
  );

  println!("\n--- Step 5: Wait for Background Refresh to Complete ---");
  thread::sleep(Duration::from_secs(1)); // Wait for loader's sleep

  println!("Loader calls: {}", load_counter.load(Ordering::Relaxed));
  assert_eq!(
    load_counter.load(Ordering::Relaxed),
    2,
    "Loader should have been called a second time"
  );

  println!("\n--- Step 6: Final Read (Fresh) ---");
  let value4 = cache.fetch_with(&key);
  println!("Received (refreshed): {:?}", *value4);
  assert_eq!(value4.version, 2, "Should now have the refreshed version 2");

  println!("\nCache metrics: {:#?}", cache.metrics());
}
