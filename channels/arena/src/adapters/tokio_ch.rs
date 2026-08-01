use crate::adapters::macros::{async_adapter, fan};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "tokio";

async_adapter! {
  name: Async_,
  sender: tokio::sync::mpsc::Sender<Payload>,
  receiver: tokio::sync::mpsc::Receiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(tokio::sync::mpsc::channel::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await,
  batch: Recv,
  recv_batch: |rx, out, max| rx.recv_many(out, max).await,
}

async_adapter! {
  name: UnboundedAsync,
  sender: tokio::sync::mpsc::UnboundedSender<Payload>,
  receiver: tokio::sync::mpsc::UnboundedReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(tokio::sync::mpsc::unbounded_channel::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await,
  batch: Recv,
  recv_batch: |rx, out, max| rx.recv_many(out, max).await,
}
