use crate::spec::{BatchSupport, Capacity};

use std::future::Future;

pub type Payload = u64;

pub type Handles<S, R> = Option<(Vec<S>, Vec<R>)>;

/// A synchronous channel under test. Implementors are zero-sized; every method
/// is a free function over the library's own handle types, so no foreign trait
/// impls are needed.
///
/// The batch methods default to looping the single-item ones, which is what a
/// caller using a library without a batch API would write. An implementation
/// overrides only the side it supports natively and declares that in [`BATCH`].
///
/// [`BATCH`]: SyncChannel::BATCH
pub trait SyncChannel: Send + Sync + 'static {
  type Sender: Send + 'static;
  type Receiver: Send + 'static;

  const BATCH: BatchSupport = BatchSupport::None;

  /// `None` means this implementation cannot serve the requested capacity or
  /// handle count, and the cell is reported as unsupported rather than failed.
  fn build(cap: Capacity, producers: usize, consumers: usize) -> Handles<Self::Sender, Self::Receiver>;

  /// `false` means the channel closed early.
  fn send(tx: &mut Self::Sender, item: Payload) -> bool;

  /// `None` means disconnected and drained.
  fn recv(rx: &mut Self::Receiver) -> Option<Payload>;

  /// Sends every item, draining those accepted from the front of `items`.
  fn send_batch(tx: &mut Self::Sender, items: &mut Vec<Payload>) -> bool {
    let mut sent = 0;
    let mut ok = true;
    for &item in items.iter() {
      if !Self::send(tx, item) {
        ok = false;
        break;
      }
      sent += 1;
    }
    items.drain(..sent);
    ok
  }

  /// Appends up to `max` items to `out`, returning how many. `0` means
  /// disconnected and drained.
  fn recv_batch(rx: &mut Self::Receiver, out: &mut Vec<Payload>, max: usize) -> usize {
    let _ = max;
    match Self::recv(rx) {
      Some(item) => {
        out.push(item);
        1
      }
      None => 0,
    }
  }
}

pub trait AsyncChannel: Send + Sync + 'static {
  type Sender: Send + 'static;
  type Receiver: Send + 'static;

  const BATCH: BatchSupport = BatchSupport::None;

  fn build(cap: Capacity, producers: usize, consumers: usize) -> Handles<Self::Sender, Self::Receiver>;

  fn send<'a>(tx: &'a mut Self::Sender, item: Payload) -> impl Future<Output = bool> + Send + 'a;

  fn recv<'a>(rx: &'a mut Self::Receiver) -> impl Future<Output = Option<Payload>> + Send + 'a;

  fn send_batch<'a>(
    tx: &'a mut Self::Sender,
    items: &'a mut Vec<Payload>,
  ) -> impl Future<Output = bool> + Send + 'a {
    async move {
      let mut sent = 0;
      let mut ok = true;
      for index in 0..items.len() {
        let item = items[index];
        if !Self::send(&mut *tx, item).await {
          ok = false;
          break;
        }
        sent += 1;
      }
      items.drain(..sent);
      ok
    }
  }

  fn recv_batch<'a>(
    rx: &'a mut Self::Receiver,
    out: &'a mut Vec<Payload>,
    max: usize,
  ) -> impl Future<Output = usize> + Send + 'a {
    async move {
      let _ = max;
      match Self::recv(&mut *rx).await {
        Some(item) => {
          out.push(item);
          1
        }
        None => 0,
      }
    }
  }
}

pub fn fan_clone<T: Clone>(handle: T, n: usize) -> Option<Vec<T>> {
  if n == 0 {
    return None;
  }
  let mut out = Vec::with_capacity(n);
  for _ in 1..n {
    out.push(handle.clone());
  }
  out.push(handle);
  Some(out)
}

pub fn fan_single<T>(handle: T, n: usize) -> Option<Vec<T>> {
  if n == 1 { Some(vec![handle]) } else { None }
}
