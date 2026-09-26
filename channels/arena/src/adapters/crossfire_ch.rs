use crossfire::{AsyncRxTrait, BlockingRxTrait, BlockingTxTrait};

use crate::adapters::macros::{async_adapter, fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "crossfire";

// crossfire rewrites a zero-sized bounded channel as size 1, so it has no
// rendezvous channel to measure.

sync_adapter! {
  name: MpscSync,
  sender: crossfire::MTx<crossfire::mpsc::Array<Payload>>,
  receiver: crossfire::Rx<crossfire::mpsc::Array<Payload>>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(crossfire::mpsc::bounded_blocking::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

sync_adapter! {
  name: MpscUnboundedSync,
  sender: crossfire::MTx<crossfire::mpsc::List<Payload>>,
  receiver: crossfire::Rx<crossfire::mpsc::List<Payload>>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(crossfire::mpsc::unbounded_blocking::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: MpscAsync,
  sender: crossfire::MAsyncTx<crossfire::mpsc::Array<Payload>>,
  receiver: crossfire::AsyncRx<crossfire::mpsc::Array<Payload>>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(crossfire::mpsc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

async_adapter! {
  name: MpscUnboundedAsync,
  sender: crossfire::MTx<crossfire::mpsc::List<Payload>>,
  receiver: crossfire::AsyncRx<crossfire::mpsc::List<Payload>>,
  senders: clone,
  receivers: single,
  build: |cap| match cap {
    Capacity::Unbounded => Some(crossfire::mpsc::unbounded_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

sync_adapter! {
  name: MpmcSync,
  sender: crossfire::MTx<crossfire::mpmc::Array<Payload>>,
  receiver: crossfire::MRx<crossfire::mpmc::Array<Payload>>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(crossfire::mpmc::bounded_blocking::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

sync_adapter! {
  name: MpmcUnboundedSync,
  sender: crossfire::MTx<crossfire::mpmc::List<Payload>>,
  receiver: crossfire::MRx<crossfire::mpmc::List<Payload>>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Unbounded => Some(crossfire::mpmc::unbounded_blocking::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

async_adapter! {
  name: MpmcAsync,
  sender: crossfire::MAsyncTx<crossfire::mpmc::Array<Payload>>,
  receiver: crossfire::MAsyncRx<crossfire::mpmc::Array<Payload>>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Bounded(n) => Some(crossfire::mpmc::bounded_async::<Payload>(n)),
    _ => None,
  },
  send: |tx, item| tx.send(item).await.is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

async_adapter! {
  name: MpmcUnboundedAsync,
  sender: crossfire::MTx<crossfire::mpmc::List<Payload>>,
  receiver: crossfire::MAsyncRx<crossfire::mpmc::List<Payload>>,
  senders: clone,
  receivers: clone,
  build: |cap| match cap {
    Capacity::Unbounded => Some(crossfire::mpmc::unbounded_async::<Payload>()),
    _ => None,
  },
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}
