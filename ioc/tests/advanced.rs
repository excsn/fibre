use fibre_ioc::{global, resolve, Container};
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc,
};
use std::thread;

// --- Advanced Test Fixtures ---

struct AppConfig {
  database_url: String,
}

// A service that depends on AppConfig.
struct DatabaseConnection {
  url: String,
}

// A service that depends on DatabaseConnection.
struct UserService {
  db: Arc<DatabaseConnection>,
}

impl UserService {
  fn get_user(&self) -> String {
    format!("user from db at {}", self.db.url)
  }
}

// --- Advanced Tests ---

#[test]
fn test_multi_level_dependency_chaining() {
  // This test ensures that services can resolve other services in their factories.
  // Arrange
  // 1. Register the root dependency.
  global().add_instance_with_name(
    "test_config_chain", // Use a unique name to avoid test conflicts
    AppConfig {
      database_url: "postgres://user:pass@host:5432/db".to_string(),
    },
  );

  // 2. Register the mid-level dependency, which resolves the config.
  global().add_singleton_with_name("test_db_chain", || {
    let config = resolve!(AppConfig, "test_config_chain");
    DatabaseConnection {
      url: config.database_url.clone(),
    }
  });

  // 3. Register the top-level service, which resolves the database connection.
  global().add_singleton_with_name("test_user_service_chain", || UserService {
    db: resolve!(DatabaseConnection, "test_db_chain"),
  });

  // Act
  let user_service = resolve!(UserService, "test_user_service_chain");

  // Assert
  assert_eq!(
    user_service.get_user(),
    "user from db at postgres://user:pass@host:5432/db"
  );
}

#[test]
fn test_custom_container_is_isolated_from_global() {
  // This test proves that a user can create their own container instance
  // that does not interfere with the global one. This is crucial for testing.

  // Arrange
  let custom_container = Container::new();

  // Register a value ONLY in the global container.
  global().add_instance_with_name("global_string_isolated", String::from("I am global"));

  // Register a different value ONLY in the custom container using its explicit unnamed method.
  custom_container.add_instance(String::from("I am in a custom container"));

  // Act & Assert
  // 1. The global container can resolve its value.
  assert_eq!(*resolve!(String, "global_string_isolated"), "I am global");

  // 2. The global container CANNOT resolve the custom container's value.
  assert!(global().get::<String>(None).is_none());

  // 3. The custom container can resolve its value.
  let local_val = custom_container.get::<String>(None).unwrap();
  assert_eq!(*local_val, "I am in a custom container");

  // 4. The custom container CANNOT resolve the global container's value.
  assert!(custom_container
    .get::<String>(Some("global_string_isolated"))
    .is_none());
}

#[test]
fn test_singleton_factory_is_called_only_once_under_concurrency() {
  // This test is critical for verifying the thread-safety of lazy initialization.

  // An atomic counter to track how many times the factory is executed.
  static FACTORY_EXECUTION_COUNT: AtomicUsize = AtomicUsize::new(0);

  // A unique struct for this test to avoid conflicts.
  struct ConcurrentService;

  // Arrange: Use the correct explicit unnamed function.
  global().add_singleton(|| {
    // This block should only ever be entered once across all threads.
    FACTORY_EXECUTION_COUNT.fetch_add(1, Ordering::SeqCst);
    // Simulate some work to increase the chance of a race condition if not implemented correctly.
    thread::sleep(std::time::Duration::from_millis(50));
    ConcurrentService
  });

  // Act
  // Spawn multiple threads that all try to resolve the same service concurrently.
  thread::scope(|s| {
    for _ in 0..20 {
      s.spawn(|| {
        // Each thread resolves the service.
        let _service = resolve!(ConcurrentService);
      });
    }
  });

  // Assert
  // If the singleton is implemented correctly, the factory was only executed once.
  assert_eq!(FACTORY_EXECUTION_COUNT.load(Ordering::SeqCst), 1);
}

#[test]
#[should_panic(expected = "Circular dependency detected")]
fn test_circular_dependency_panics() {
  // This test ensures that a direct circular dependency causes a panic instead of
  // a stack overflow or a deadlock. `once_cell`'s sync primitives panic if
  // a thread tries to re-enter `get_or_init` for a cell it's already initializing.

  struct ServiceA {
    _b: Arc<ServiceB>,
  }
  struct ServiceB {
    _a: Arc<ServiceA>,
  }
  
  // Arrange: Create a circular dependency A -> B -> A
  global().add_singleton_with_name("circular_a", || ServiceA {
    _b: resolve!(ServiceB, "circular_b"),
  });
  global().add_singleton_with_name("circular_b", || ServiceB {
    _a: resolve!(ServiceA, "circular_a"),
  });

  // Act: Resolving either service should trigger the panic.
  resolve!(ServiceA, "circular_a");
}

#[test]
fn test_overwriting_registration_is_successful() {
  // This test documents the behavior that the last registration for a given key wins.
  
  // 1. Test with a concrete type
  global().add_instance_with_name("overwrite_test", "first value".to_string());
  let first = resolve!(String, "overwrite_test");
  assert_eq!(*first, "first value");

  // Overwrite the registration
  global().add_instance_with_name("overwrite_test", "second value".to_string());
  let second = resolve!(String, "overwrite_test");
  assert_eq!(*second, "second value"); // The new value should be resolved.

  // 2. Test with a trait
  trait OverwriteTrait: Send + Sync { fn name(&self) -> &str; }
  struct English; impl OverwriteTrait for English { fn name(&self) -> &str { "English" } }
  struct German; impl OverwriteTrait for German { fn name(&self) -> &str { "German" } }
  
  global().add_singleton_trait_with_name::<dyn OverwriteTrait>("overwrite_trait", || Arc::new(English));
  let first_trait = resolve!(trait OverwriteTrait, "overwrite_trait");
  assert_eq!(first_trait.name(), "English");

  // Overwrite the trait registration
  global().add_singleton_trait_with_name::<dyn OverwriteTrait>("overwrite_trait", || Arc::new(German));
  let second_trait = resolve!(trait OverwriteTrait, "overwrite_trait");
  assert_eq!(second_trait.name(), "German"); // The new implementation should be resolved.
}

#[test]
fn test_singleton_depending_on_transient() {
  // This test verifies the lifetime interaction: a singleton resolves its transient
  // dependencies only once, at the moment of its own creation.

  // A transient service with a unique ID per instance.
  struct TransientDependency {
    id: usize,
  }
  // A singleton that holds onto the transient dependency it was created with.
  struct SingletonHolder {
    dependency: Arc<TransientDependency>,
  }

  static TRANSIENT_COUNTER: AtomicUsize = AtomicUsize::new(0);
  
  // Arrange
  global().add_transient_with_name("test_transient", || {
    let id = TRANSIENT_COUNTER.fetch_add(1, Ordering::SeqCst);
    TransientDependency { id }
  });

  global().add_singleton_with_name("test_singleton_holder", || SingletonHolder {
    dependency: resolve!(TransientDependency, "test_transient"),
  });

  // Act
  let holder1 = resolve!(SingletonHolder, "test_singleton_holder");
  let holder2 = resolve!(SingletonHolder, "test_singleton_holder");
  let standalone_transient = resolve!(TransientDependency, "test_transient");

  // Assert
  // 1. Both resolutions of the holder are the same instance.
  assert!(Arc::ptr_eq(&holder1, &holder2));
  
  // 2. Both holders contain the *exact same* instance of the dependency.
  assert!(Arc::ptr_eq(&holder1.dependency, &holder2.dependency));
  
  // 3. The ID of the held dependency should be 0, as it was the first one created.
  assert_eq!(holder1.dependency.id, 0);

  // 4. A newly resolved transient has a different ID, proving the transient
  //    factory is working, but the singleton is holding onto its original instance.
  assert_eq!(standalone_transient.id, 1);
}

#[test]
fn test_concurrent_registration_and_resolution() {
  // A stress test to ensure registering new services while resolving others
  // does not cause deadlocks or race conditions.

  // Arrange: Pre-register a common service that all threads will resolve.
  global().add_singleton_with_name("common_service", || 42_i32);

  // Act: Spawn multiple threads to perform reads and writes concurrently.
  thread::scope(|s| {
    for i in 0..10 {
      s.spawn(move || {
        // Each thread registers its own unique service.
        global().add_instance_with_name(&format!("thread_service_{}", i), i);

        // Each thread also resolves the common service multiple times.
        for _ in 0..100 {
          let common = resolve!(i32, "common_service");
          assert_eq!(*common, 42);
        }

        // Each thread resolves its own service to ensure the write was successful.
        let my_service = resolve!(usize, &format!("thread_service_{}", i));
        assert_eq!(*my_service, i);
      });
    }
  });

  // Assert: After all threads finish, verify from the main thread that
  // a service registered by one of the threads is available.
  let final_check = resolve!(usize, "thread_service_5");
  assert_eq!(*final_check, 5);
}

#[test]
fn test_resolving_arc_directly() {
  // This test ensures that if a user explicitly registers an Arc<T>,
  // they can resolve it as Arc<T>. This is a meta-test of the container's
  // ability to handle nested generics like Arc<Arc<T>> correctly.

  struct ConfigArc(Arc<String>);
  
  // Arrange
  let shared_string = Arc::new("shared config data".to_string());
  global().add_instance_with_name("arc_instance", shared_string.clone());

  // Act
  let resolved_arc = resolve!(Arc<String>, "arc_instance");

  // Assert
  // 1. The resolved Arc points to the same data.
  assert_eq!(&**resolved_arc, "shared config data");
  // 2. The resolved Arc is the *exact same* Arc instance.
  assert!(Arc::ptr_eq(&shared_string, &resolved_arc));
}

#[test]
fn test_service_with_static_lifetime() {
  // This test ensures that services with 'static lifetimes, a common pattern
  // for configuration, are handled correctly.

  struct StaticConfig {
    name: &'static str,
  }

  // Arrange
  global().add_singleton_with_name("static_config", || StaticConfig { name: "app_v1" });

  // Act
  let config = resolve!(StaticConfig, "static_config");

  // Assert
  assert_eq!(config.name, "app_v1");
}


#[test]
fn test_resolving_generic_struct_with_trait_bound() {
  // This tests a more complex generic scenario where the service itself is generic.

  trait MessageSender: Send + Sync {
    fn send(&self, msg: &str);
  }
  struct EmailSender;
  impl MessageSender for EmailSender {
    fn send(&self, msg: &str) {
      // In a real app, this would send an email.
      println!("Emailing: {}", msg);
    }
  }

  // A generic notification service that can use any MessageSender.
  struct NotificationService<T: MessageSender> {
    sender: Arc<T>,
  }
  impl<T: MessageSender> NotificationService<T> {
    fn notify(&self, message: &str) {
      self.sender.send(message);
    }
  }

  // Arrange
  // 1. Register the concrete dependency.
  global().add_singleton_with_name("email_sender", || EmailSender);
  
  // 2. Register the generic service, specifying its concrete type parameter.
  global().add_singleton_with_name("notification_service", || NotificationService::<EmailSender> {
    sender: resolve!(EmailSender, "email_sender"),
  });

  // Act
  let notifier = resolve!(NotificationService<EmailSender>, "notification_service");
  
  // Assert (the test passes if it runs without panicking and we can call the method)
  notifier.notify("Your subscription is expiring soon.");
}

#[test]
fn test_drop_behavior_of_singletons() {
  // This test verifies that the Drop implementation of a singleton service is
  // called when the container that owns it is dropped. This is crucial for
  // resource cleanup (e.g., closing database connections).

  static DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

  // A service that increments a counter when it's dropped.
  struct ConnectionPool;
  impl Drop for ConnectionPool {
    fn drop(&mut self) {
      // This code should run exactly once when the container is dropped.
      DROP_COUNTER.fetch_add(1, Ordering::SeqCst);
    }
  }

  // We need to use a custom container for this test, as we can't drop the global one.
  let container = Container::new();
  
  // Arrange
  container.add_singleton(|| ConnectionPool);
  
  // Act
  // 1. Resolve the service to ensure the singleton is created.
  let pool = container.get::<ConnectionPool>(None).unwrap();
  // The drop counter should be 0 while the container is alive.
  assert_eq!(DROP_COUNTER.load(Ordering::SeqCst), 0);

  // 2. Drop the resolved Arc. This should NOT drop the underlying ConnectionPool,
  // as the container still holds a strong reference.
  drop(pool);
  assert_eq!(DROP_COUNTER.load(Ordering::SeqCst), 0);
  
  // 3. Drop the container itself. This should release the last strong reference
  // to the singleton, triggering its Drop implementation.
  drop(container);
  
  // Assert
  assert_eq!(DROP_COUNTER.load(Ordering::SeqCst), 1);
}