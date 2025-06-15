use fibre_cache::CacheBuilder;
use std::thread;
use std::time::Duration;

fn main() {
  // Create a cache with a 2-second TTI.
  let cache = CacheBuilder::default()
    .time_to_idle(Duration::from_secs(2))
    .janitor_tick_interval(Duration::from_millis(500))
    .build()
    .expect("Failed to build cache");

  println!("--- Time-to-Idle (TTI) Demonstration ---");
  cache.insert("my_key", "my_value", 1);
  println!("Inserted ('my_key', 'my_value'). It will expire in 2 seconds if not accessed.");

  println!("\n--- Part 1: Resetting the Idle Timer ---");
  for i in 1..=4 {
    // Wait for 1 second, which is less than the TTI.
    thread::sleep(Duration::from_secs(1));
    // Accessing the key resets its idle timer back to 2 seconds.
    assert!(cache.fetch(&"my_key").is_some());
    println!(
      "[Cycle {}] Accessed 'my_key'. Its 2-second idle timer has been reset.",
      i
    );
  }

  println!("\n--- Part 2: Letting the Item Expire ---");
  println!("Waiting for 3 seconds without accessing the key...");
  thread::sleep(Duration::from_secs(3));

  // The get call will find the item is expired because it has been idle for > 2s.
  assert!(
    cache.fetch(&"my_key").is_none(),
    "Item should be expired now."
  );
  println!("'my_key' has expired due to being idle for more than 2 seconds.");

  println!("\nFinal Metrics: {:#?}", cache.metrics());
  assert_eq!(cache.metrics().evicted_by_tti, 1);
}
