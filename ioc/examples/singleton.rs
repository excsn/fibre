use fibre_ioc::{global, resolve};
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};

// A simple service that gets a unique ID upon creation.
struct RequestTracker {
  id: usize,
}

// A global, thread-safe counter to generate unique IDs.
static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn main() {
  // --- Singleton Registration ---
  // This factory will only be called ONCE.
  global().add_singleton_with_name("singleton_tracker", || {
    println!("Creating SINGLETON RequestTracker...");
    RequestTracker {
      id: ID_COUNTER.fetch_add(1, Ordering::SeqCst),
    }
  });

  // --- Transient Registration ---
  // This factory will be called EVERY time the service is resolved.
  global().add_transient_with_name("transient_tracker", || {
    println!("Creating TRANSIENT RequestTracker...");
    RequestTracker {
      id: ID_COUNTER.fetch_add(1, Ordering::SeqCst),
    }
  });

  println!("--- Resolving Singletons ---");
  let s1 = resolve!(RequestTracker, "singleton_tracker");
  let s2 = resolve!(RequestTracker, "singleton_tracker");
  println!("Singleton 1 ID: {}, Singleton 2 ID: {}", s1.id, s2.id);
  assert_eq!(s1.id, 0);
  assert_eq!(s2.id, 0);
  assert!(
    Arc::ptr_eq(&s1, &s2),
    "Singleton instances should be identical"
  );
  println!("Singleton instances are the same pointer, as expected.\n");

  println!("--- Resolving Transients ---");
  let t1 = resolve!(RequestTracker, "transient_tracker");
  let t2 = resolve!(RequestTracker, "transient_tracker");
  println!("Transient 1 ID: {}, Transient 2 ID: {}", t1.id, t2.id);
  assert_eq!(t1.id, 1);
  assert_eq!(t2.id, 2);
  assert!(
    !Arc::ptr_eq(&t1, &t2),
    "Transient instances should be different"
  );
  println!("Transient instances are different pointers, as expected.");
}
