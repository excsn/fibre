use fibre_ioc::LocalContainer;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[test]
fn test_local_singleton() {
  let mut container = LocalContainer::new();
  container.add_singleton(|| "hello".to_string());

  let r1 = container.get::<String>(None).unwrap();
  let r2 = container.get::<String>(None).unwrap();

  assert_eq!(*r1, "hello");
  // Ensure it's a singleton by checking pointer equality.
  assert!(Rc::ptr_eq(&r1, &r2));
}

#[test]
fn test_local_transient() {
  let mut container = LocalContainer::new();
  // Use a Cell to show that we get new instances
  container.add_transient(|| Cell::new(10));

  let r1 = container.get::<Cell<i32>>(None).unwrap();
  let r2 = container.get::<Cell<i32>>(None).unwrap();

  r1.set(20);

  assert_eq!(r1.get(), 20);
  assert_eq!(r2.get(), 10); // r2 is a different instance
  assert!(!Rc::ptr_eq(&r1, &r2));
}

#[test]
fn test_local_trait_resolution() {
  trait Greeter {
    fn greet(&self) -> String;
  }
  struct English;
  impl Greeter for English {
    fn greet(&self) -> String {
      "Hello".to_string()
    }
  }

  let mut container = LocalContainer::new();
  container.add_singleton_trait::<dyn Greeter>(|| Rc::new(English));

  let greeter = container.get::<dyn Greeter>(None).unwrap();
  assert_eq!(greeter.greet(), "Hello");
}

#[test]
#[should_panic(expected = "Circular dependency detected")]
fn test_local_circular_dependency_panics() {
  // This test demonstrates that the circular dependency guard works correctly
  // even with the more complex ownership model of LocalContainer.
  struct ServiceA {
    _b: Rc<ServiceB>,
  }
  struct ServiceB {
    _a: Rc<ServiceA>,
  }

  // To allow closures to resolve from the container they are being registered into,
  // we need shared ownership and interior mutability.
  let container = Rc::new(RefCell::new(LocalContainer::new()));

  // Register ServiceA, its factory will try to resolve ServiceB
  let c_for_a = Rc::clone(&container);
  container
    .borrow_mut()
    .add_singleton_with_name("a", move || {
      let container_ref = c_for_a.borrow();
      ServiceA {
        _b: container_ref.get::<ServiceB>(Some("b")).unwrap(),
      }
    });

  // Register ServiceB, its factory will try to resolve ServiceA
  let c_for_b = Rc::clone(&container);
  container
    .borrow_mut()
    .add_singleton_with_name("b", move || {
      let container_ref = c_for_b.borrow();
      ServiceB {
        _a: container_ref.get::<ServiceA>(Some("a")).unwrap(),
      }
    });

  // Act: Resolving either service should now trigger the panic as intended.
  // Resolution path: get(A) -> factory(A) -> get(B) -> factory(B) -> get(A) -> PANIC!
  container.borrow().get::<ServiceA>(Some("a"));
}

#[test]
fn test_local_container_handles_not_send_sync_types() {
  // `Rc<i32>` is neither `Send` nor `Sync`.
  // This is impossible with the thread-safe `Container`.
  struct NotSendSyncService {
    data: Rc<i32>,
  }

  let mut container = LocalContainer::new();
  let shared_data = Rc::new(42);

  // The factory closure must be `Fn`, so we clone the Rc inside it.
  container.add_singleton(move || NotSendSyncService {
    data: Rc::clone(&shared_data),
  });

  let service = container.get::<NotSendSyncService>(None).unwrap();
  assert_eq!(*service.data, 42);

  // Resolve twice and check that the singleton contains the same inner Rc.
  let service2 = container.get::<NotSendSyncService>(None).unwrap();
  assert!(Rc::ptr_eq(&service.data, &service2.data));

  println!("Successfully stored and retrieved a !Send + !Sync type!");
}
