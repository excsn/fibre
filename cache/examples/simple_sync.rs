use fibre_cache::CacheBuilder;
use std::thread;
use std::time::Duration;

fn main() {
  // Create a cache with a capacity of 100 items, a 5-second TTL,
  // and a 1-second janitor tick interval.
  let cache = CacheBuilder::default()
    .capacity(100)
    .time_to_live(Duration::from_secs(5))
    .janitor_tick_interval(Duration::from_secs(1))
    .build()
    .expect("Failed to build cache");

  println!("Inserting ('key1', 100) with cost 1 into the cache.");
  cache.insert("key1".to_string(), 100, 1);

  // Get the value.
  match cache.get(&"key1".to_string()) {
    Some(value) => println!("Found value for key1: {}", value),
    None => println!("Value for key1 not found."),
  }

  println!("\nCache metrics: {:#?}", cache.metrics());

  println!("\nWaiting for 6 seconds for the item to expire...");
  thread::sleep(Duration::from_secs(6));

  // The janitor runs every second and will have removed the expired item.
  match cache.get(&"key1".to_string()) {
    Some(value) => println!("Found value for key1: {}", value),
    None => println!("Value for key1 not found (as expected after TTL)."),
  }

  println!("\nCache metrics after expiration: {:#?}", cache.metrics());
}
