//! The main `Container` struct and its associated methods.

use crate::core::{InjectionKey, Provider, ResolutionGuard};
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::sync::Arc;

/// The Inversion of Control (IoC) container.
///
/// This struct holds the registrations for all services. It is thread-safe
/// and allows for dynamic registration and resolution of services.
#[derive(Default)]
pub struct Container {
  providers: DashMap<InjectionKey, Provider>,
}

impl Container {
  /// Creates a new, empty `Container`.
  pub fn new() -> Self {
    Self::default()
  }

  // --- PRIVATE HELPERS ---

  fn add_instance_internal<T: Any + Send + Sync>(&self, name: Option<&str>, instance: T) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };
    let provider = Provider::Singleton {
      cell: once_cell::sync::OnceCell::with_value(Box::new(Arc::new(instance))),
      factory: Box::new(|| panic!("Pre-initialized singleton factory should not be called")),
    };
    self.providers.insert(key, provider);
  }

  fn add_singleton_internal<T: Any + Send + Sync>(
    &self,
    name: Option<&str>,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };
    let provider = Provider::Singleton {
      cell: once_cell::sync::OnceCell::new(),
      factory: Box::new(move || Box::new(Arc::new(factory()))),
    };
    self.providers.insert(key, provider);
  }

  fn add_transient_internal<T: Any + Send + Sync>(
    &self,
    name: Option<&str>,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };
    let provider = Provider::Transient {
      factory: Box::new(move || Box::new(Arc::new(factory()))),
    };
    self.providers.insert(key, provider);
  }

  fn add_singleton_trait_internal<I: ?Sized + Any + Send + Sync>(
    &self,
    name: Option<&str>,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
  ) {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<I>(n),
      None => InjectionKey::new::<I>(),
    };
    let provider = Provider::Singleton {
      cell: once_cell::sync::OnceCell::new(),
      factory: Box::new(move || Box::new(factory())),
    };
    self.providers.insert(key, provider);
  }

  // --- PUBLIC API ---

  // --- Instance Registration ---
  pub fn add_instance<T: Any + Send + Sync>(&self, instance: T) {
    self.add_instance_internal(None, instance);
  }
  pub fn add_instance_with_name<T: Any + Send + Sync>(&self, name: &str, instance: T) {
    self.add_instance_internal(Some(name), instance);
  }

  // --- Singleton Registration ---
  pub fn add_singleton<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    self.add_singleton_internal(None, factory);
  }
  pub fn add_singleton_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    self.add_singleton_internal(Some(name), factory);
  }

  // --- Transient Registration ---
  pub fn add_transient<T: Any + Send + Sync>(
    &self,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    self.add_transient_internal(None, factory);
  }
  pub fn add_transient_with_name<T: Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> T + Send + Sync + 'static,
  ) {
    self.add_transient_internal(Some(name), factory);
  }

  // --- Trait Registration ---
  pub fn add_singleton_trait<I: ?Sized + Any + Send + Sync>(
    &self,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
  ) {
    self.add_singleton_trait_internal(None, factory);
  }
  pub fn add_singleton_trait_with_name<I: ?Sized + Any + Send + Sync>(
    &self,
    name: &str,
    factory: impl Fn() -> Arc<I> + Send + Sync + 'static,
  ) {
    self.add_singleton_trait_internal(Some(name), factory);
  }

  // --- Resolution ---
  /// Resolves a service from the container.
  pub fn get<T: ?Sized + Any + Send + Sync>(&self, name: Option<&str>) -> Option<Arc<T>> {
    let key = match name {
      Some(n) => InjectionKey::new_with_name::<T>(n),
      None => InjectionKey::new::<T>(),
    };

    // Create the RAII guard at the beginning of resolution.
    // Its constructor will panic on a circular dependency.
    // Its destructor will clean up the stack automatically when `get` returns.
    let _guard = ResolutionGuard::new(key.clone());

    let provider_ref = self.providers.get(&key)?;
    let provider = provider_ref.value();

    match provider {
      Provider::Singleton { .. } => provider
        .get_singleton_ref()
        .and_then(|instance_box| instance_box.downcast_ref::<Arc<T>>())
        .cloned(),
      Provider::Transient { .. } => provider
        .get_transient_owned()
        .and_then(|instance_box| instance_box.downcast::<Arc<T>>().ok())
        .map(|arc_in_a_box| *arc_in_a_box),
    }
  }
}
