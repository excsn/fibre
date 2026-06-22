mod async_impl;
mod core;
mod sync_impl;

pub use async_impl::{
  RecvBatchFuture, RecvBatchMutFuture, RecvFuture, SendBatchFuture, SendBatchMutFuture, SendFuture,
};
// Crate-internal surface used by the public handles.
pub(crate) use async_impl::poll_stream_next;
pub(crate) use core::Shared;
pub(crate) use sync_impl::{
  recv_batch_sync, recv_sync, recv_timeout_sync, send_batch_sync, send_sync, try_recv,
  try_recv_batch_core, try_send, try_send_batch_core,
};
