# API Reference: fibre_ioc

An Inversion of Control container with thread-safe and single-threaded variants plus resolution macros.

## Contents

* [1. Containers](#1-containers)
  * [`Container`](#container)
  * [`LocalContainer`](#localcontainer)
* [2. Macros](#2-macros)
  * [`resolve!`](#resolve)
  * [`maybe_resolve!`](#maybe_resolve)
  * [`resolve_from!`](#resolve_from)
  * [`maybe_resolve_from!`](#maybe_resolve_from)
* [3. Functions](#3-functions)
  * [`global`](#global)
* [4. Error Handling](#4-error-handling)

## 1. Containers

These rules apply to both containers:

* Registering again under the same type and name replaces the earlier provider and drops its cached singleton. Handles already resolved keep the old instance.
* `get` holds a map lock while a factory runs. `Container` registration from inside a factory on the same container can deadlock.
* A factory runs on the thread that resolved it.

### `Container`

`#[derive(Default)] pub struct Container`. Thread-safe. Shares through `Arc`. Registered types must be `Any + Send + Sync`, which includes `'static`.

* `pub fn new() -> Self`
* `pub fn add_instance<T: Any + Send + Sync>(&self, instance: T)`: a pre-built singleton.
* `pub fn add_instance_with_name<T: Any + Send + Sync>(&self, name: &str, instance: T)`
* `pub fn add_singleton<T: Any + Send + Sync>(&self, factory: impl Fn() -> T + Send + Sync + 'static)`: the factory runs once, on first resolution.
* `pub fn add_singleton_with_name<T: Any + Send + Sync>(&self, name: &str, factory: impl Fn() -> T + Send + Sync + 'static)`
* `pub fn add_transient<T: Any + Send + Sync>(&self, factory: impl Fn() -> T + Send + Sync + 'static)`: the factory runs on every resolution.
* `pub fn add_transient_with_name<T: Any + Send + Sync>(&self, name: &str, factory: impl Fn() -> T + Send + Sync + 'static)`
* `pub fn add_singleton_trait<I: ?Sized + Any + Send + Sync>(&self, factory: impl Fn() -> Arc<I> + Send + Sync + 'static)`
* `pub fn add_singleton_trait_with_name<I: ?Sized + Any + Send + Sync>(&self, name: &str, factory: impl Fn() -> Arc<I> + Send + Sync + 'static)`
* `pub fn get<T: ?Sized + Any + Send + Sync>(&self, name: Option<&str>) -> Option<Arc<T>>`: `None` when nothing is registered. Panics on a circular dependency.

### `LocalContainer`

`#[derive(Default)] pub struct LocalContainer`. Requires the `local` feature. Single-threaded. Shares through `Rc`. Types need not be `Send` or `Sync`. Registration takes `&mut self`. It has no `add_instance`.

* `pub fn new() -> Self`
* `pub fn add_singleton<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static)`
* `pub fn add_singleton_with_name<T: Any + 'static>(&mut self, name: &str, factory: impl Fn() -> T + 'static)`
* `pub fn add_transient<T: Any + 'static>(&mut self, factory: impl Fn() -> T + 'static)`
* `pub fn add_transient_with_name<T: Any + 'static>(&mut self, name: &str, factory: impl Fn() -> T + 'static)`
* `pub fn add_singleton_trait<I: ?Sized + Any + 'static>(&mut self, factory: impl Fn() -> Rc<I> + 'static)`
* `pub fn add_singleton_trait_with_name<I: ?Sized + Any + 'static>(&mut self, name: &str, factory: impl Fn() -> Rc<I> + 'static)`
* `pub fn get<T: ?Sized + Any + 'static>(&self, name: Option<&str>) -> Option<Rc<T>>`: `None` when nothing is registered. Panics on a circular dependency.

## 2. Macros

In a `trait` form, `MyTrait` is one identifier in scope. For a path, use the type form: `resolve!(dyn path::MyTrait)`.

### `resolve!`

Global container. Panics when the service is missing. Returns `Arc<T>`.

* `resolve!(MyType)`
* `resolve!(MyType, "my_name")`
* `resolve!(trait MyTrait)`
* `resolve!(trait MyTrait, "my_name")`

### `maybe_resolve!`

Global container. Returns `Option<Arc<T>>`.

* `maybe_resolve!(MyType)`
* `maybe_resolve!(MyType, "my_name")`
* `maybe_resolve!(trait MyTrait)`
* `maybe_resolve!(trait MyTrait, "my_name")`

### `resolve_from!`

Any container. Panics when the service is missing. Returns `Arc<T>` or `Rc<T>`.

* `resolve_from!(my_container, MyType)`
* `resolve_from!(my_container, MyType, "my_name")`
* `resolve_from!(my_container, trait MyTrait)`
* `resolve_from!(my_container, trait MyTrait, "my_name")`

### `maybe_resolve_from!`

Any container. Returns `Option<Arc<T>>` or `Option<Rc<T>>`.

* `maybe_resolve_from!(my_container, MyType)`
* `maybe_resolve_from!(my_container, MyType, "my_name")`
* `maybe_resolve_from!(my_container, trait MyTrait)`
* `maybe_resolve_from!(my_container, trait MyTrait, "my_name")`

## 3. Functions

### `global`

* `pub fn global() -> &'static Container`: the process-wide container, created on first access.

## 4. Error Handling

The crate defines no error type.

* **Missing service**: `resolve!` and `resolve_from!` panic with `Failed to resolve required service: {type}`, `Failed to resolve required service with name '{name}': {type}`, `Failed to resolve required trait service: {type}` or `Failed to resolve required trait service with name '{name}': {type}`. `maybe_resolve!`, `maybe_resolve_from!` and `get` return `None`.
* **Circular dependency**: every resolution path panics with `Circular dependency detected while resolving service: {key}`. The check is per thread and keyed on type and name, not on the container, so a factory that resolves its own type and name from a different container also panics.
