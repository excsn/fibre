use crate::adapters::macros::{fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "std";

sync_adapter! {
  name: Sync_,
  sender: std::sync::mpsc::SyncSender<Payload>,
  receiver: std::sync::mpsc::Receiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(std::sync::mpsc::sync_channel::<Payload>(0)),
    Capacity::Bounded(n) => Some(std::sync::mpsc::sync_channel::<Payload>(n)),
    Capacity::Unbounded => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

sync_adapter! {
  name: UnboundedSync,
  sender: std::sync::mpsc::Sender<Payload>,
  receiver: std::sync::mpsc::Receiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(std::sync::mpsc::channel::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}
