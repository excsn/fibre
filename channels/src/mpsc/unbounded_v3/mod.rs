//! Unbounded MPSC v3: strict-FIFO Vyukov intrusive chain with per-handle bump
//! slabs.
//!
//! Producers pay exactly one shared atomic RMW per send — the always-succeeding
//! `swap` that linearizes the global FIFO order (the provable floor for any
//! strictly ordered multi-producer queue). Everything else is handle-local:
//! nodes come from private `SLAB_NODES`-node slabs (recycled through a small
//! per-channel pool once fully retired), `len()` uses sharded monotonic
//! counters, and sends never block so no send-waiter machinery exists at all.

mod consumer;
mod producer;
mod shared;

use std::sync::Arc;

pub use consumer::{AsyncReceiver, Receiver, RecvBatchFuture, RecvBatchMutFuture, RecvFuture};
pub use producer::{AsyncSender, SendBatchFuture, SendBatchMutFuture, SendFuture, Sender};

pub(crate) fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
  let shared = Arc::new(shared::MpscShared::new());
  (
    Sender::from_shared(Arc::clone(&shared)),
    Receiver::from_shared(shared),
  )
}

pub(crate) fn channel_async<T: Send>() -> (AsyncSender<T>, AsyncReceiver<T>) {
  let shared = Arc::new(shared::MpscShared::new());
  (
    AsyncSender::from_shared(Arc::clone(&shared)),
    AsyncReceiver::from_shared(shared),
  )
}

#[cfg(test)]
mod tests {
  use super::shared::SLAB_NODES;
  use super::*;
  use crate::error::{RecvError, TryRecvError};
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;
  use std::thread;

  struct DropCounter(Arc<AtomicUsize>);
  impl Drop for DropCounter {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::Relaxed);
    }
  }

  #[test]
  fn smoke() {
    let (mut tx, rx) = channel();
    tx.send(7u32).unwrap();
    assert_eq!(rx.recv().unwrap(), 7);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  }

  #[test]
  fn fifo_across_slab_boundaries() {
    let (mut tx, rx) = channel();
    let total = SLAB_NODES * 3 + 17;
    for i in 0..total {
      tx.send(i).unwrap();
    }
    for i in 0..total {
      assert_eq!(rx.recv().unwrap(), i);
    }
    assert!(rx.is_empty());
  }

  #[test]
  fn slab_recycling_preserves_fifo() {
    // Interleaved send/recv fully retires each slab shortly after its
    // boundary, so from the third slab on every acquisition reuses a pooled
    // one (re-armed nodes, no allocator round trip).
    let (mut tx, rx) = channel();
    for i in 0..SLAB_NODES * 3 + 7 {
      tx.send(i).unwrap();
      assert_eq!(rx.recv().unwrap(), i);
    }
    assert!(rx.is_empty());
  }

  #[test]
  fn drop_with_in_flight_items_spanning_slabs() {
    let drops = Arc::new(AtomicUsize::new(0));
    let total = SLAB_NODES * 2 + 100;
    {
      let (mut tx, rx) = channel();
      for _ in 0..total {
        tx.send(DropCounter(drops.clone())).unwrap();
      }
      for _ in 0..100 {
        drop(rx.recv().unwrap());
      }
      assert_eq!(drops.load(Ordering::Relaxed), 100);
      drop(tx);
      drop(rx); // receiver close drains and drops the rest
    }
    assert_eq!(drops.load(Ordering::Relaxed), total);
  }

  #[test]
  fn handle_drop_seals_partial_slab() {
    let (mut tx, rx) = channel();
    for i in 0..10u32 {
      tx.send(i).unwrap();
    }
    drop(tx); // sealed with 10 of SLAB_NODES used
    for i in 0..10 {
      assert_eq!(rx.recv().unwrap(), i);
    }
    assert_eq!(rx.recv(), Err(RecvError::Disconnected));
  }

  #[test]
  fn clone_storm_short_lived_senders() {
    let (tx, rx) = channel();
    let per_sender = 20usize;
    let senders = 50usize;
    let mut handles = Vec::new();
    for s in 0..senders {
      let mut txc = tx.clone();
      handles.push(thread::spawn(move || {
        for i in 0..per_sender {
          txc.send(s * per_sender + i).unwrap();
        }
      }));
    }
    drop(tx);
    for h in handles {
      h.join().unwrap();
    }
    let mut got = Vec::new();
    loop {
      match rx.recv() {
        Ok(v) => got.push(v),
        Err(RecvError::Disconnected) => break,
      }
    }
    assert_eq!(got.len(), senders * per_sender);
    got.sort_unstable();
    for (i, v) in got.into_iter().enumerate() {
      assert_eq!(i, v);
    }
  }

  #[test]
  fn per_producer_order_is_preserved() {
    let (tx, rx) = channel();
    let producers = 4usize;
    let per = 5_000usize;
    let mut handles = Vec::new();
    for p in 0..producers {
      let mut txc = tx.clone();
      handles.push(thread::spawn(move || {
        for i in 0..per {
          txc.send((p, i)).unwrap();
        }
      }));
    }
    drop(tx);
    let mut next = vec![0usize; producers];
    for _ in 0..producers * per {
      let (p, i) = rx.recv().unwrap();
      assert_eq!(next[p], i, "producer {p} reordered");
      next[p] += 1;
    }
    for h in handles {
      h.join().unwrap();
    }
  }

  #[test]
  fn len_tracks_across_many_handles() {
    let (mut tx, rx) = channel::<usize>();
    // More clones than LEN_SHARDS so shard assignment wraps.
    let mut clones: Vec<_> = (0..40).map(|_| tx.clone()).collect();
    for (i, c) in clones.iter_mut().enumerate() {
      c.send(i).unwrap();
    }
    tx.send(usize::MAX).unwrap();
    assert_eq!(rx.len(), 41);
    assert_eq!(tx.len(), 41);
    for _ in 0..41 {
      rx.recv().unwrap();
    }
    assert_eq!(rx.len(), 0);
    assert!(rx.is_empty());
  }

  #[test]
  fn batch_send_and_batch_recv() {
    let (mut tx, rx) = channel();
    let total = SLAB_NODES + 50;
    assert_eq!(tx.send_batch((0..total).collect()).unwrap(), total);
    let mut out = Vec::new();
    let mut got = 0;
    while got < total {
      got += rx.recv_batch_mut(&mut out, 64).unwrap();
    }
    assert_eq!(out, (0..total).collect::<Vec<_>>());
  }

  #[test]
  fn send_batch_mut_drains_and_close_semantics() {
    let (mut tx, rx) = channel::<u32>();
    let mut items = vec![1, 2, 3];
    assert_eq!(tx.send_batch_mut(&mut items).unwrap(), 3);
    assert!(items.is_empty());
    drop(rx);
    let mut items = vec![4, 5];
    assert_eq!(tx.send_batch_mut(&mut items), Err(crate::error::SendError::Closed));
    assert_eq!(items, vec![4, 5]);
  }

  #[test]
  fn receiver_close_drains_promptly() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (mut tx, rx) = channel();
    for _ in 0..5 {
      tx.send(DropCounter(drops.clone())).unwrap();
    }
    rx.close().unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), 5);
    assert!(tx.is_closed());
    drop(tx);
  }

  #[tokio::test]
  async fn async_smoke_and_mut_receiver() {
    let (mut tx, mut rx) = channel_async();
    tx.send(11u32).await.unwrap();
    assert_eq!(rx.recv().await.unwrap(), 11);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    drop(tx);
    assert_eq!(rx.recv().await, Err(RecvError::Disconnected));
  }

  #[tokio::test]
  async fn async_recv_wakes_on_send() {
    let (mut tx, mut rx) = channel_async();
    let handle = tokio::spawn(async move {
      tokio::time::sleep(std::time::Duration::from_millis(50)).await;
      tx.send("hi").await.unwrap();
    });
    assert_eq!(rx.recv().await.unwrap(), "hi");
    handle.await.unwrap();
  }

  #[tokio::test]
  async fn async_recv_future_cancel_safe() {
    let (mut tx, mut rx) = channel_async::<u32>();
    {
      let fut = rx.recv();
      let res = tokio::time::timeout(std::time::Duration::from_millis(50), fut).await;
      assert!(res.is_err(), "future should be pending on empty channel");
    }
    // Dropping the pending future must not lose the registration slot.
    tx.send(9).await.unwrap();
    assert_eq!(rx.recv().await.unwrap(), 9);
  }
}
