//! Core, non-public data structures for the IoC container.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

thread_local! {
  // This thread-local variable holds the set of services currently being resolved
  // on this specific thread. This is the key to detecting circular dependencies.
  static RESOLVING_STACK: RefCell<HashSet<InjectionKey>> = RefCell::new(HashSet::new());
}

/// An RAII guard to detect and prevent circular dependencies.
///
/// When created, it adds a service key to the thread-local resolution stack.
/// If the key is already present, it means we have a circular dependency, and it panics.
/// When the guard is dropped, it removes the key from the stack.
pub(crate) struct ResolutionGuard {
  key: InjectionKey,
}

impl ResolutionGuard {
  pub(crate) fn new(key: InjectionKey) -> Self {
    RESOLVING_STACK.with(|stack| {
      let mut stack = stack.borrow_mut();
      // `insert` returns `false` if the value was already present.
      if !stack.insert(key.clone()) {
        panic!(
          "Circular dependency detected while resolving service: {:?}",
          key
        );
      }
    });
    Self { key }
  }
}

impl Drop for ResolutionGuard {
  fn drop(&mut self) {
    RESOLVING_STACK.with(|stack| {
      stack.borrow_mut().remove(&self.key);
    });
  }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct InjectionKey {
  pub(crate) type_id: TypeId,
  pub(crate) name: Option<String>,
}

impl InjectionKey {
  pub(crate) fn new<T: ?Sized + Any>() -> Self {
    Self {
      type_id: TypeId::of::<T>(),
      name: None,
    }
  }
  pub(crate) fn new_with_name<T: ?Sized + Any>(name: &str) -> Self {
    Self {
      type_id: TypeId::of::<T>(),
      name: Some(name.to_owned()),
    }
  }
}

impl fmt::Debug for InjectionKey {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match &self.name {
      Some(name) => write!(f, "Key(TypeId({:?}), Name({}))", self.type_id, name),
      None => write!(f, "Key(TypeId({:?}))", self.type_id),
    }
  }
}

pub(crate) enum Provider {
  Singleton {
    cell: once_cell::sync::OnceCell<Box<dyn Any + Send + Sync>>,
    factory: Box<dyn Fn() -> Box<dyn Any + Send + Sync> + Send + Sync>,
  },
  Transient {
    factory: Box<dyn Fn() -> Box<dyn Any + Send + Sync> + Send + Sync>,
  },
}

impl Provider {
  pub(crate) fn get_singleton_ref(&self) -> Option<&Box<dyn Any + Send + Sync>> {
    match self {
      Provider::Singleton { cell, factory } => Some(cell.get_or_init(factory)),
      _ => None,
    }
  }

  pub(crate) fn get_transient_owned(&self) -> Option<Box<dyn Any + Send + Sync>> {
    match self {
      Provider::Transient { factory } => Some(factory()),
      _ => None,
    }
  }
}
