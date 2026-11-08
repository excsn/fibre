//! HTTP client whose handles switch between blocking and async with `to_sync` and `to_async`, sharing one connection pool.

mod body;
mod client;
mod conn;
mod decode;
mod dial;
pub mod engine;
mod error;
#[cfg(feature = "http1")]
pub mod h1;
#[cfg(feature = "http2")]
pub mod h2;
mod io;
mod notify;
mod pool;
mod response;
mod wait;

pub use body::{AsyncBody, SyncBody};
pub use client::{Async, AsyncClient, Blocking, ClientBuilder, RequestBuilder, SyncClient};
pub use error::Error;
pub use pool::{Pool, PoolBuilder};
pub use response::Response;
