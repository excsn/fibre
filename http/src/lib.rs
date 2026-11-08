//! HTTP client whose handles switch between blocking and async with `to_sync` and `to_async`, sharing one connection pool.

pub mod engine;
mod error;

pub use error::Error;
