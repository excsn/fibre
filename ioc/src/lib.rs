//! # Fibre IoC
//!
//! A flexible, thread-safe, and dynamic Inversion of Control (IoC) container for Rust.
//!
//! Fibre IoC provides a way to manage dependencies within your application. Unlike some
//! other containers that require a single, upfront initialization, Fibre IoC allows
//! for dynamic registration of services at any point during the application's lifecycle.
//!
//! ## Core Concepts
//!
//! - **Container**: The central registry for all your services.
//! - **Global Container**: A static, globally-available container, accessible via `global()`.
//! - **Resolution**: Services are resolved using the `resolve!` macro, which panics
//!   if a dependency is missing.
//! - **Traits**: Services can be registered against a trait and resolved as a trait object.
//!
//! ## Quick Start
//!
//! ```
//! use fibre_ioc::{global, resolve};
//! use std::sync::Arc;
//!
//! // Define a trait and a concrete implementation.
//! trait Greeter {
//!     fn greet(&self) -> String;
//! }
//!
//! struct EnglishGreeter {
//!     message: String,
//! }
//!
//! impl Greeter for EnglishGreeter {
//!     fn greet(&self) -> String {
//!         self.message.clone()
//!     }
//! }
//!
//! fn main() {
//!     // Register a simple value from anywhere in your app.
//!     global().add_singleton(Some("greeting_message"), || String::from("Hello, World!"));
//!
//!     // Register a service that implements a trait.
//!     // The factory can itself resolve other dependencies.
//!     global().add_singleton_trait::<dyn Greeter, _>(None, || {
//!         let message = resolve!(String, "greeting_message");
//!         EnglishGreeter { message: (*message).clone() }
//!     });
//!
//!     // In another part of your application, resolve the service by its trait.
//!     let greeter_service = resolve!(trait Greeter);
//!
//!     assert_eq!(greeter_service.greet(), "Hello, World!");
//!     println!("{}", greeter_service.greet());
//! }
//! ```

mod container;
mod core;
mod global;
#[cfg(feature = "local")]
mod local_container; 
mod macros;

pub use container::Container;
pub use global::global;
#[cfg(feature = "local")]
pub use local_container::LocalContainer;