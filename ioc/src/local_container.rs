// ioc/src/local_container.rs

//! A single-threaded, non-thread-safe Inversion of Control container.

use crate::core::{InjectionKey, ResolutionGuard};
use once_cell::unsync::OnceCell;
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

// A local, non-thread-safe version of the Provider enum.
// It uses `Rc` and `unsync::OnceCell`.
enum LocalProvider {
  Singleton {
    cell: OnceCell<Box<dyn Any>>,
    factory: Box<dyn Fn() -> Box<dyn Any>>,
  },
  Transient {
    factory: Box<dyn Fn() -> Box<dyn Any>>,
  },
}

/// A single-threaded, non-thread-safe Inversion of Control (IoC) container.
///
/// This container is designed for use cases where thread safety is not required.
/// It uses a standard `HashMap` for storage and `Rc` for shared ownership,
/// which provides better performance than its thread-safe counterpart.
///
/// A key advantage is that it can store types that are not `Send` or `Sync`.
///
/// # Note on API
///
/// Unlike the thread-safe `Container`, registration methods like `add_singleton`
/// on `LocalContainer` require a mutable reference (`&mut self`) because `HashMap`
/// does not support interior mutability.
#[derive(Default)]
pub struct LocalContainer {
  providers: HashMap<InjectionKey, LocalProvider>,
}

impl LocalContainer {
  /// Creates a new, empty `LocalContainer`.
  pub fn new() -> Self {
    Self::default()
  }

  // --- PRIVATE HELPERS ---

  fn add_singleton_internal<T: Any + 'static>(
    &mut self,
    name: Option<&str>,
    factory: impl Fn() -> T + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };
    let provider = LocalProvider::Singleton {
      cell: OnceCell::new(),
      factory: Box::new(move || Box::new(Rc::new(factory()))),
    };
    self.providers.insert(key, provider);
  }

  fn add_transient_internal<T: Any + 'static>(
    &mut self,
    name: Option<&str>,
    factory: impl Fn() -> T + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };
    let provider = LocalProvider::Transient {
      factory: Box::new(move || Box::new(Rc::new(factory()))),
    };
    self.providers.insert(key, provider);
  }

  fn add_singleton_trait_internal<I: ?Sized + Any + 'static>(
    &mut self,
    name: Option<&str>,
    factory: impl Fn() -> Rc<I> + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<I>(n),
      None => InjectionKey::new::<I>(),
    };
    let provider = LocalProvider::Singleton {
      cell: OnceCell::new(),
      factory: Box::new(move || Box::new(factory())),
    };
    self.providers.insert(key, provider);
  }

  // --- PUBLIC API ---

  // --- Singleton Registration ---
  pub fn add_singleton<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static) {
    self.add_singleton_internal(None, factory);
  }

  pub fn add_singleton_with_name<T: Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> T + 'static,
  ) {
    self.add_singleton_internal(Some(name), factory);
  }

  // --- Transient Registration ---
  pub fn add_transient<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static) {
    self.add_transient_internal(None, factory);
  }

  pub fn add_transient_with_name<T: Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> T + 'static,
  ) {
    self.add_transient_internal(Some(name), factory);
  }

  // --- Trait Registration ---
  pub fn add_singleton_trait<I: ?Sized + Any + 'static>(
    &mut self,
    factory: impl Fn() -> Rc<I> + 'static,
  ) {
    self.add_singleton_trait_internal(None, factory);
  }

  pub fn add_singleton_trait_with_name<I: ?Sized + Any + 'static>(
    &mut self,
    name: &str,
    factory: impl Fn() -> Rc<I> + 'static,
  ) {
    self.add_singleton_trait_internal(Some(name), factory);
  }

  // --- Resolution ---

  /// Resolves a service from the container.
  ///
  /// Returns an `Option<Rc<T>>`. Returns `None` if the service is not found.
  /// This method will still panic if a circular dependency is detected.
  pub fn get<T: ?Sized + Any + 'static>(&self, name: Option<&str>) -> Option<Rc<T>> {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };

    let _guard = ResolutionGuard::new(key.clone());

    let provider = self.providers.get(&key)?;

    match provider {
      LocalProvider::Singleton { cell, factory } => {
        cell.get_or_init(factory).downcast_ref::<Rc<T>>().cloned()
      }
      LocalProvider::Transient { factory } => factory()
        .downcast::<Rc<T>>()
        .ok()
        .map(|rc_in_a_box| *rc_in_a_box),
    }
  }
}
