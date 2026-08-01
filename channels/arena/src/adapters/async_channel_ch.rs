use crate::adapters::macros::{async_adapter, fan};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "async-channel";

async_adapter! {
  name: Async_,
  sender: async_channel::Sender<Payload>,
  receiver: async_channel::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(async_channel::bounded::<Payload>(n)),
    Capacity::Unbounded => Some(async_channel::unbounded::<Payload>()),
    Capacity::Rendezvous => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}
