//! HTTP client whose handles switch between blocking and async with `to_sync` and `to_async`, sharing one connection pool.

pub mod engine;
mod error;
#[cfg(feature = "http1")]
pub mod h1;
#[cfg(feature = "http2")]
pub mod h2;

pub use error::Error;
