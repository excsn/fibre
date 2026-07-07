//! Bounded MPSC V3 - a screamingly high performance, multimode sync/async, cancel safe queue with credit-BEFORE-claim design. Inspired by brutally fast MPSC C++ libraries and evolved over time.
//!
//! A producer checks the credit window and only claims a wait-free `fetch_add`
//! ticket into the chunked slot arrays when it looks open; a lost check-then-claim
//! race is resolved by a SKIP tombstone + retry (bounded waste). Parked producers
//! hold no ticket, so cancellation is trivial, and the wake policies (sync
//! batch-by-freed / async metered drip) apply verbatim. Chunks are recycled
//! through a fixed id-epoch table with reset-on-drain; `chunk_cap` is flavor-tuned
//! (sync/async). See `shared` for the core.

mod consumer;
mod producer;
mod shared;

pub use consumer::{
  AsyncReceiver, BoundedRecvBatchFuture, BoundedRecvBatchMutFuture, Receiver, RecvFuture,
};
pub use producer::{
  AsyncSender, BoundedSendBatchFuture, BoundedSendBatchMutFuture, SendFuture, Sender,
};

use std::marker::PhantomData;

use crate::internal::sync::AtomicBool;
use shared::{Shared, CACHE_FLUSH_CHUNK, CHUNK_FLOOR_ASYNC, CHUNK_FLOOR_SYNC};

pub fn bounded<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  // Sync receiver flush cadence: K = min(cap, CACHE_FLUSH_CHUNK); sync chunk floor.
  let shared = Shared::new(capacity, capacity.min(CACHE_FLUSH_CHUNK), CHUNK_FLOOR_SYNC);
  let sender = Sender {
    shared: shared.clone(),
    closed: AtomicBool::new(false),
  };
  let receiver = Receiver {
    shared,
    closed: AtomicBool::new(false),
    _not_sync: PhantomData,
  };
  (sender, receiver)
}

pub fn bounded_async<T: Send>(capacity: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
  // Async receiver flush cadence: K = full cap (hoard-then-dump arms the drip);
  // async chunk floor (small, L1-residency-friendly).
  let shared = Shared::new(capacity, capacity, CHUNK_FLOOR_ASYNC);
  let sender = AsyncSender {
    shared: shared.clone(),
    closed: AtomicBool::new(false),
  };
  let receiver = AsyncReceiver {
    shared,
    closed: AtomicBool::new(false),
    is_registered: false,
  };
  (sender, receiver)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::RecvError;
  use std::pin::pin;
  use std::task::{Context, Waker};
  use std::thread;

  /// Cancel-safety by construction: a Pending send holds no claim, so dropping
  /// it must leave the stream fully intact - no SKIP, no wedge, no loss.
  #[test]
  fn cancelled_pending_send_leaves_no_trace() {
    let (stx, srx) = bounded::<u64>(2);
    let atx = stx.clone().to_async();
    let mut cx = Context::from_waker(Waker::noop());

    stx.send(10).unwrap();
    stx.send(11).unwrap(); // window (cap 2) full
    {
      let mut fut = pin!(atx.send(99));
      assert!(fut.as_mut().poll(&mut cx).is_pending());
    } // dropped while Pending - held no ticket

    assert_eq!(srx.recv(), Ok(10));
    assert_eq!(srx.recv(), Ok(11));
    stx.send(12).unwrap();
    assert_eq!(srx.recv(), Ok(12));

    drop(stx);
    drop(atx);
    assert_eq!(srx.recv(), Err(RecvError::Disconnected));
  }

  /// Multi-producer per-producer FIFO under real contention at a tiny cap.
  #[test]
  fn smoke_sync_multi_producer_fifo() {
    const PRODUCERS: usize = 4;
    const PER: u64 = 10_000;
    let (stx, srx) = bounded::<u64>(4);
    let joins: Vec<_> = (0..PRODUCERS)
      .map(|p| {
        let tx = stx.clone();
        thread::spawn(move || {
          for i in 0..PER {
            tx.send(p as u64 * PER + i).unwrap();
          }
        })
      })
      .collect();
    drop(stx);

    let mut got = 0u64;
    let mut last = [None::<u64>; PRODUCERS];
    while let Ok(v) = srx.recv() {
      got += 1;
      let (p, i) = ((v / PER) as usize, v % PER);
      assert!(
        last[p].is_none_or(|l| i == l + 1),
        "per-producer FIFO broken"
      );
      last[p] = Some(i);
    }
    assert_eq!(got, PRODUCERS as u64 * PER);
    for j in joins {
      j.join().unwrap();
    }
  }
}
