//! Unbounded MPMC: lock-free slab-chain producers + a mutex-serialized
//! consumer side that wakes one parked receiver per publish. See `shared.rs` for the design notes.

mod consumer;
mod producer;
mod shared;

use crate::internal::sync::Arc;

pub use consumer::{
  RecvBatchFuture, RecvBatchMutFuture, RecvFuture, UnboundedAsyncReceiver, UnboundedSyncReceiver,
};
pub use producer::{
  SendBatchFuture, SendBatchMutFuture, SendFuture, UnboundedAsyncSender, UnboundedSyncSender,
};

pub(crate) fn channel<T: Send>() -> (UnboundedSyncSender<T>, UnboundedSyncReceiver<T>) {
  let shared = Arc::new(shared::UnboundedShared::new());
  (
    UnboundedSyncSender::from_shared(Arc::clone(&shared)),
    UnboundedSyncReceiver::from_shared(shared),
  )
}

pub(crate) fn channel_async<T: Send>() -> (UnboundedAsyncSender<T>, UnboundedAsyncReceiver<T>) {
  let shared = Arc::new(shared::UnboundedShared::new());
  (
    UnboundedAsyncSender::from_shared(Arc::clone(&shared)),
    UnboundedAsyncReceiver::from_shared(shared),
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::error::{RecvError, TryRecvError};
  use crate::internal::slab_chain::SLAB_NODES;
  use std::future::Future;
  use std::pin::pin;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;
  use std::task::{Context, Poll, Waker};
  use std::thread;
  use std::time::{Duration, Instant};

  fn poll_once<F: Future + ?Sized>(fut: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    let waker = futures_util::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    fut.poll(&mut cx)
  }

  struct DropCounter(Arc<AtomicUsize>);
  impl Drop for DropCounter {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::Relaxed);
    }
  }

  #[test]
  fn fifo_across_slab_boundaries() {
    let (mut tx, mut rx) = channel();
    let total = SLAB_NODES * 2 + 33;
    for i in 0..total {
      tx.send(i).unwrap();
    }
    for i in 0..total {
      assert_eq!(rx.recv().unwrap(), i);
    }
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  }

  #[test]
  fn slab_recycling_preserves_values_and_drops() {
    // Interleaved send/recv fully retires each slab shortly after its
    // boundary, so from the third slab on every acquisition reuses a pooled
    // one (re-armed nodes, no allocator round trip).
    let drops = Arc::new(AtomicUsize::new(0));
    let total = SLAB_NODES * 3 + 7;
    let (mut tx, mut rx) = channel();
    for _ in 0..total {
      tx.send(DropCounter(drops.clone())).unwrap();
      drop(rx.recv().unwrap());
    }
    assert_eq!(drops.load(Ordering::Relaxed), total);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  }

  #[test]
  fn mp_mc_threads_sum() {
    let (tx, rx) = channel();
    let producers = 3usize;
    let consumers = 2usize;
    let per = 2_000usize;
    let sum = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..producers {
      let mut txc = tx.clone();
      handles.push(thread::spawn(move || {
        for i in 1..=per {
          txc.send(i).unwrap();
        }
      }));
    }
    drop(tx);
    for _ in 0..consumers {
      let mut rxc = rx.clone();
      let s = sum.clone();
      handles.push(thread::spawn(move || {
        while let Ok(v) = rxc.recv() {
          s.fetch_add(v, Ordering::Relaxed);
        }
      }));
    }
    drop(rx);
    for h in handles {
      h.join().unwrap();
    }
    assert_eq!(sum.load(Ordering::Relaxed), producers * (per * (per + 1) / 2));
  }

  #[test]
  fn handoff_delivers_to_parked_consumers() {
    let (mut tx, rx) = channel::<u32>();
    let mut r1 = rx.clone();
    let mut r2 = rx.clone();
    let c1 = thread::spawn(move || r1.recv().unwrap());
    let c2 = thread::spawn(move || r2.recv().unwrap());
    // Both consumers park (eventually); each send hands one item over.
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    let mut got = vec![c1.join().unwrap(), c2.join().unwrap()];
    got.sort_unstable();
    assert_eq!(got, vec![1, 2]);
  }

  #[test]
  fn disconnect_wakes_all_parked_consumers() {
    let (tx, rx) = channel::<u32>();
    let mut r1 = rx.clone();
    let mut r2 = rx.clone();
    let c1 = thread::spawn(move || r1.recv());
    let c2 = thread::spawn(move || r2.recv());
    drop(tx); // NOTIFIED wake-all; both must observe Disconnected
    assert!(matches!(c1.join().unwrap(), Err(RecvError::Disconnected)));
    assert!(matches!(c2.join().unwrap(), Err(RecvError::Disconnected)));
  }

  #[test]
  fn cancelled_pending_future_loses_nothing() {
    let (mut tx, mut rx) = channel_async::<u32>();
    {
      let mut fut = pin!(rx.recv());
      assert!(poll_once(fut.as_mut()).is_pending());
    }
    tx.try_send(7).unwrap();
    assert_eq!(rx.try_recv().unwrap(), 7);
  }

  #[test]
  fn a_parked_future_dropped_after_a_send_loses_no_item() {
    let (mut tx, mut rx) = channel_async::<u32>();
    {
      let mut fut = pin!(rx.recv());
      assert!(poll_once(fut.as_mut()).is_pending());
      tx.try_send(42).unwrap();
    }
    assert_eq!(rx.try_recv().unwrap(), 42);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
  }

  #[test]
  fn items_sent_while_a_future_was_parked_keep_their_order_after_it_drops() {
    let (mut tx, mut rx) = channel_async::<u32>();
    {
      let mut fut = pin!(rx.recv());
      assert!(poll_once(fut.as_mut()).is_pending());
      tx.try_send(1).unwrap();
      tx.try_send(2).unwrap();
    }
    assert_eq!(rx.try_recv().unwrap(), 1);
    assert_eq!(rx.try_recv().unwrap(), 2);
  }

  #[test]
  fn values_dropped_exactly_once_on_channel_drop() {
    let drops = Arc::new(AtomicUsize::new(0));
    let total = SLAB_NODES + 40;
    {
      let (mut tx, mut rx) = channel();
      for _ in 0..total {
        tx.send(DropCounter(drops.clone())).unwrap();
      }
      for _ in 0..10 {
        drop(rx.recv().unwrap());
      }
      drop(tx);
      drop(rx);
    }
    assert_eq!(drops.load(Ordering::Relaxed), total);
  }

  #[test]
  fn clone_storm_both_sides() {
    let (tx, rx) = channel::<usize>();
    let mut handles = Vec::new();
    for s in 0..10usize {
      let mut txc = tx.clone();
      handles.push(thread::spawn(move || {
        for i in 0..50 {
          txc.send(s * 50 + i).unwrap();
        }
      }));
    }
    drop(tx);
    let seen = Arc::new(AtomicUsize::new(0));
    for _ in 0..4 {
      let mut rxc = rx.clone();
      let seen = seen.clone();
      handles.push(thread::spawn(move || {
        while rxc.recv().is_ok() {
          seen.fetch_add(1, Ordering::Relaxed);
        }
      }));
    }
    drop(rx);
    for h in handles {
      h.join().unwrap();
    }
    assert_eq!(seen.load(Ordering::Relaxed), 500);
  }

  #[test]
  fn recv_timeout_zero_and_success() {
    let (mut tx, mut rx) = channel::<u32>();
    assert!(matches!(
      rx.recv_timeout(std::time::Duration::ZERO),
      Err(crate::error::RecvErrorTimeout::Timeout)
    ));
    tx.send(9).unwrap();
    assert_eq!(rx.recv_timeout(std::time::Duration::ZERO).unwrap(), 9);
  }

  #[test]
  fn batch_roundtrip() {
    let (mut tx, mut rx) = channel::<usize>();
    let total = SLAB_NODES + 20;
    assert_eq!(tx.send_batch((0..total).collect()).unwrap(), total);
    let mut out = Vec::new();
    let mut got = 0;
    while got < total {
      got += rx.recv_batch_mut(&mut out, 48).unwrap();
    }
    assert_eq!(out, (0..total).collect::<Vec<_>>());
  }

  struct CountWake(AtomicUsize);
  impl std::task::Wake for CountWake {
    fn wake(self: Arc<Self>) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  fn count_waker() -> (Arc<CountWake>, Waker) {
    let count = Arc::new(CountWake(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    (count, waker)
  }

  fn poll_with<F: Future + ?Sized>(fut: std::pin::Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    fut.poll(&mut Context::from_waker(waker))
  }

  fn wait_for_parked<T: Send>(shared: &shared::UnboundedShared<T>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.parked_receivers() != count {
      assert!(Instant::now() < deadline, "receivers did not park");
      thread::yield_now();
    }
  }

  #[test]
  fn batch_send_reaches_every_parked_receiver() {
    let (tx, rx) = channel::<u32>();
    let (got_tx, got_rx) = std::sync::mpsc::channel();
    for _ in 0..3 {
      let mut rx = rx.clone();
      let got_tx = got_tx.clone();
      thread::spawn(move || {
        let item = rx.recv();
        let _ = got_tx.send(item);
      });
    }
    wait_for_parked(&rx.shared, 3);

    let mut producer = tx.clone();
    assert_eq!(producer.send_batch(vec![1, 2, 3]).unwrap(), 3);

    let mut got = Vec::new();
    for _ in 0..3 {
      let item = got_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("a parked receiver was left out of the batch");
      got.push(item.unwrap());
    }
    got.sort_unstable();
    assert_eq!(got, vec![1, 2, 3]);
  }

  #[test]
  fn batch_send_completes_every_pending_receive() {
    let (mut tx, rx) = channel_async::<u32>();
    let mut receivers = [rx.clone(), rx.clone(), rx];
    let (counts, wakers): (Vec<_>, Vec<_>) = (0..3).map(|_| count_waker()).unzip();
    let mut futs: Vec<_> = receivers.iter_mut().map(|r| Box::pin(r.recv())).collect();
    for (fut, waker) in futs.iter_mut().zip(&wakers) {
      assert!(poll_with(fut.as_mut(), waker).is_pending());
    }

    {
      let mut batch = pin!(tx.send_batch(vec![1, 2, 3]));
      assert!(matches!(poll_once(batch.as_mut()), Poll::Ready(Ok(3))));
    }

    // Polls a receive only after its waker fired, as an executor does.
    let mut got = Vec::new();
    let mut seen = [0usize; 3];
    let mut done = [false; 3];
    loop {
      let mut progressed = false;
      for i in 0..3 {
        let fired = counts[i].0.load(Ordering::SeqCst);
        if done[i] || fired == seen[i] {
          continue;
        }
        seen[i] = fired;
        progressed = true;
        if let Poll::Ready(item) = poll_with(futs[i].as_mut(), &wakers[i]) {
          got.push(item.unwrap());
          done[i] = true;
        }
      }
      if !progressed {
        break;
      }
    }
    got.sort_unstable();
    assert_eq!(got, vec![1, 2, 3], "a pending receive was not woken for its item");
  }

  #[test]
  fn receive_dropped_after_its_wake_hands_the_wake_to_the_next_receiver() {
    let (mut tx, rx) = channel_async::<u32>();
    let mut first = rx.clone();
    let mut second = rx;
    let (first_count, first_waker) = count_waker();
    let (second_count, second_waker) = count_waker();
    let mut first_fut = Box::pin(first.recv());
    let mut second_fut = Box::pin(second.recv());
    assert!(poll_with(first_fut.as_mut(), &first_waker).is_pending());
    assert!(poll_with(second_fut.as_mut(), &second_waker).is_pending());

    tx.try_send(7).unwrap();
    assert_eq!(first_count.0.load(Ordering::SeqCst), 1);
    drop(first_fut);

    assert_ne!(
      second_count.0.load(Ordering::SeqCst),
      0,
      "the item is queued and the receiver still pending was not woken"
    );
    assert!(matches!(poll_with(second_fut.as_mut(), &second_waker), Poll::Ready(Ok(7))));
  }

  #[test]
  fn a_stream_registration_left_unpolled_does_not_hold_the_wake_chain() {
    let (mut tx, rx) = channel_async::<u32>();
    let mut stream = rx.clone();
    let mut first = rx.clone();
    let mut second = rx;
    assert!(futures_core::Stream::poll_next(std::pin::Pin::new(&mut stream), &mut Context::from_waker(Waker::noop())).is_pending());
    let (first_count, first_waker) = count_waker();
    let (second_count, second_waker) = count_waker();
    let mut first_fut = Box::pin(first.recv());
    let mut second_fut = Box::pin(second.recv());
    assert!(poll_with(first_fut.as_mut(), &first_waker).is_pending());
    assert!(poll_with(second_fut.as_mut(), &second_waker).is_pending());

    tx.try_send_batch(vec![1, 2, 3]).unwrap();
    assert_ne!(first_count.0.load(Ordering::SeqCst), 0, "the batch woke only the stream registration");
    assert!(matches!(poll_with(first_fut.as_mut(), &first_waker), Poll::Ready(Ok(1))));
    assert_ne!(second_count.0.load(Ordering::SeqCst), 0, "an item is queued and the second receive was not woken");
    assert!(matches!(poll_with(second_fut.as_mut(), &second_waker), Poll::Ready(Ok(2))));
  }
}
