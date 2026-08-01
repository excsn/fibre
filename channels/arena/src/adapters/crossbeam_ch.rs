use crate::adapters::macros::{fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "crossbeam";

sync_adapter! {
  name: Sync_,
  sender: crossbeam_channel::Sender<Payload>,
  receiver: crossbeam_channel::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(crossbeam_channel::bounded::<Payload>(0)),
    Capacity::Bounded(n) => Some(crossbeam_channel::bounded::<Payload>(n)),
    Capacity::Unbounded => Some(crossbeam_channel::unbounded::<Payload>()),
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}
