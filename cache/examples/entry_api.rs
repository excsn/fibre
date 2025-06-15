use fibre_cache::{Cache, CacheBuilder, Entry};

// A function to demonstrate atomic get-or-insert logic.
fn get_or_create_user_session(cache: &Cache<String, u32>, user_id: &str) -> u32 {
  println!("\nAttempting to get or create session for '{}'...", user_id);

  // The .entry() method acquires a write lock on the shard for this key.
  // This ensures the entire operation is atomic.
  let entry = cache.entry(user_id.to_string());

  match entry {
    // The entry was occupied (the session already exists).
    Entry::Occupied(occupied) => {
      println!("Session found! ID: {}", *occupied.get());
      // We can just get the existing value.
      *occupied.get()
    }
    // The entry was vacant (no session for this user).
    Entry::Vacant(vacant) => {
      let new_session_id = rand::random::<u32>();
      println!(
        "No session found. Creating new session with ID: {}",
        new_session_id
      );
      // We can insert the new value and get a reference to it back.
      // The cost is 1.
      *vacant.insert(new_session_id, 1)
    }
  }
}

fn main() {
  let cache = CacheBuilder::default()
    .capacity(100)
    .build()
    .expect("Failed to build cache");

  let user_a = "user_a".to_string();
  let user_b = "user_b".to_string();

  // First call for user_a: should be vacant
  let session_a_1 = get_or_create_user_session(&cache, &user_a);

  // Second call for user_a: should be occupied
  let session_a_2 = get_or_create_user_session(&cache, &user_a);
  assert_eq!(
    session_a_1, session_a_2,
    "Session ID for user_a should be stable"
  );

  // First call for user_b: should be vacant
  let session_b = get_or_create_user_session(&cache, &user_b);
  assert_ne!(
    session_a_1, session_b,
    "Different users should have different sessions"
  );

  println!("\nFinal cache state:");
  println!("user_a -> session {}", cache.fetch(&user_a).unwrap());
  println!("user_b -> session {}", cache.fetch(&user_b).unwrap());
  println!("\nMetrics: {:#?}", cache.metrics());
}
