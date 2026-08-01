use crate::adapters::macros::{async_adapter, fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "kanal";

sync_adapter! {
  name: Sync_,
  sender: kanal::Sender<Payload>,
  receiver: kanal::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(kanal::bounded::<Payload>(0)),
    Capacity::Bounded(n) => Some(kanal::bounded::<Payload>(n)),
    Capacity::Unbounded => Some(kanal::unbounded::<Payload>()),
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: Async_,
  sender: kanal::AsyncSender<Payload>,
  receiver: kanal::AsyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(kanal::bounded_async::<Payload>(0)),
    Capacity::Bounded(n) => Some(kanal::bounded_async::<Payload>(n)),
    Capacity::Unbounded => Some(kanal::unbounded_async::<Payload>()),
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}
