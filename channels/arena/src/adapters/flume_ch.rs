use crate::adapters::macros::{async_adapter, fan, sync_adapter};
use crate::channel::Payload;
use crate::spec::Capacity;

pub const LIBRARY: &str = "flume";

fn build(cap: Capacity) -> Option<(flume::Sender<Payload>, flume::Receiver<Payload>)> {
  match cap {
    Capacity::Rendezvous => Some(flume::bounded::<Payload>(0)),
    Capacity::Bounded(n) => Some(flume::bounded::<Payload>(n)),
    Capacity::Unbounded => Some(flume::unbounded::<Payload>()),
  }
}

sync_adapter! {
  name: Sync_,
  sender: flume::Sender<Payload>,
  receiver: flume::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| build(cap),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
  batch: Recv,
  // `drain` is consumed fully rather than truncated at `max`: dropping it early
  // discards the rest of the buffered items.
  recv_batch: |rx, out, max| {
    let _ = max;
    match rx.recv() {
      Ok(item) => out.push(item),
      Err(_) => return 0,
    }
    let mut count = 1;
    for item in rx.drain() {
      out.push(item);
      count += 1;
    }
    count
  },
}

async_adapter! {
  name: Async_,
  sender: flume::Sender<Payload>,
  receiver: flume::Receiver<Payload>,
  senders: clone,
  receivers: clone,
  build: |cap| build(cap),
  send: |tx, item| tx.send_async(item).await.is_ok(),
  recv: |rx| rx.recv_async().await.ok(),
  batch: Recv,
  recv_batch: |rx, out, max| {
    let _ = max;
    match rx.recv_async().await {
      Ok(item) => out.push(item),
      Err(_) => return 0,
    }
    let mut count = 1;
    for item in rx.drain() {
      out.push(item);
      count += 1;
    }
    count
  },
}
