use crate::adapters::macros::{async_adapter, fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "fibre";

sync_adapter! {
  name: SpscSync,
  sender: fibre::spsc::BoundedSyncSender<Payload>,
  receiver: fibre::spsc::BoundedSyncReceiver<Payload>,
  senders: single,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::spsc::bounded_sync::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

sync_adapter! {
  name: SpscRendezvousSync,
  sender: fibre::spsc::rendezvous::RendezvousSyncSender<Payload>,
  receiver: fibre::spsc::rendezvous::RendezvousSyncReceiver<Payload>,
  senders: single,
  receivers: single,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::spsc::rendezvous::rendezvous::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: SpscAsync,
  sender: fibre::spsc::BoundedAsyncSender<Payload>,
  receiver: fibre::spsc::BoundedAsyncReceiver<Payload>,
  senders: single,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::spsc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

async_adapter! {
  name: SpscRendezvousAsync,
  sender: fibre::spsc::rendezvous::RendezvousAsyncSender<Payload>,
  receiver: fibre::spsc::rendezvous::RendezvousAsyncReceiver<Payload>,
  senders: single,
  receivers: single,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::spsc::rendezvous::rendezvous_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

sync_adapter! {
  name: MpscSync,
  sender: fibre::mpsc::BoundedSyncSender<Payload>,
  receiver: fibre::mpsc::BoundedSyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::mpsc::bounded::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

sync_adapter! {
  name: MpscUnboundedSync,
  sender: fibre::mpsc::UnboundedSyncSender<Payload>,
  receiver: fibre::mpsc::UnboundedSyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(fibre::mpsc::unbounded::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

sync_adapter! {
  name: MpscRendezvousSync,
  sender: fibre::mpsc::rendezvous::RendezvousSyncSender<Payload>,
  receiver: fibre::mpsc::rendezvous::RendezvousSyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::mpsc::rendezvous::rendezvous::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: MpscAsync,
  sender: fibre::mpsc::BoundedAsyncSender<Payload>,
  receiver: fibre::mpsc::BoundedAsyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::mpsc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

async_adapter! {
  name: MpscUnboundedAsync,
  sender: fibre::mpsc::UnboundedAsyncSender<Payload>,
  receiver: fibre::mpsc::UnboundedAsyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(fibre::mpsc::unbounded_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

async_adapter! {
  name: MpscRendezvousAsync,
  sender: fibre::mpsc::rendezvous::RendezvousAsyncSender<Payload>,
  receiver: fibre::mpsc::rendezvous::RendezvousAsyncReceiver<Payload>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::mpsc::rendezvous::rendezvous_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

sync_adapter! {
  name: SpmcSync,
  sender: fibre::spmc::BoundedSyncSender<Payload>,
  receiver: fibre::spmc::BoundedSyncReceiver<Payload>,
  senders: single,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::spmc::bounded::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

async_adapter! {
  name: SpmcAsync,
  sender: fibre::spmc::BoundedAsyncSender<Payload>,
  receiver: fibre::spmc::BoundedAsyncReceiver<Payload>,
  senders: single,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::spmc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

sync_adapter! {
  name: MpmcSync,
  sender: fibre::mpmc::Sender<Payload>,
  receiver: fibre::mpmc::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::mpmc::bounded::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

sync_adapter! {
  name: MpmcUnboundedSync,
  sender: fibre::mpmc::UnboundedSyncSender<Payload>,
  receiver: fibre::mpmc::UnboundedSyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Unbounded => Some(fibre::mpmc::unbounded::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).unwrap_or(0),
}

sync_adapter! {
  name: MpmcRendezvousSync,
  sender: fibre::mpmc::rendezvous::RendezvousSyncSender<Payload>,
  receiver: fibre::mpmc::rendezvous::RendezvousSyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::mpmc::rendezvous::rendezvous::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: MpmcAsync,
  sender: fibre::mpmc::AsyncSender<Payload>,
  receiver: fibre::mpmc::AsyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(fibre::mpmc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

async_adapter! {
  name: MpmcUnboundedAsync,
  sender: fibre::mpmc::UnboundedAsyncSender<Payload>,
  receiver: fibre::mpmc::UnboundedAsyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Unbounded => Some(fibre::mpmc::unbounded_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
  batch: SendRecv,
  send_batch: |tx, items| tx.send_batch_mut(items).await.is_ok(),
  recv_batch: |rx, out, max| rx.recv_batch_mut(out, max).await.unwrap_or(0),
}

async_adapter! {
  name: MpmcRendezvousAsync,
  sender: fibre::mpmc::rendezvous::RendezvousAsyncSender<Payload>,
  receiver: fibre::mpmc::rendezvous::RendezvousAsyncReceiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Rendezvous => Some(fibre::mpmc::rendezvous::rendezvous_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}
