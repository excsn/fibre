//! A bounded mpsc batch send that unwinds from a panicking receiver waker
//! leaves the channel's sender wakes working.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use fibre::mpsc;

struct PanicWaker;

impl Wake for PanicWaker {
  fn wake(self: Arc<Self>) {
    panic!("receiver waker panics");
  }
  fn wake_by_ref(self: &Arc<Self>) {
    panic!("receiver waker panics");
  }
}

fn run(batch_mut: bool) {
  let (tx, mut rx) = mpsc::bounded_async::<u32>(2);
  let tx = tx.to_sync();
  let batcher = tx.clone();
  let unwound = Arc::new(AtomicBool::new(false));
  let flag = unwound.clone();
  let handle = thread::spawn(move || {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      if batch_mut {
        let mut items: Vec<u32> = (0..1000).collect();
        let _ = batcher.send_batch_mut(&mut items);
      } else {
        let _ = batcher.send_batch((0..1000).collect());
      }
    }));
    flag.store(r.is_err(), Ordering::SeqCst);
  });

  let waker = Waker::from(Arc::new(PanicWaker));
  for _ in 0..500 {
    if handle.is_finished() {
      break;
    }
    while rx.len() < 2 && !handle.is_finished() {
      thread::yield_now();
    }
    while rx.try_recv().is_ok() {}
    let mut recv = pin!(rx.recv());
    let _ = recv.as_mut().poll(&mut Context::from_waker(&waker));
    thread::sleep(Duration::from_millis(1));
  }
  handle.join().unwrap();
  assert!(unwound.load(Ordering::SeqCst), "the batch send never unwound from the receiver's waker");

  while rx.try_recv().is_ok() {}
  while tx.try_send(0).is_ok() {}
  let late = tx.clone();
  let (done_tx, done_rx) = std_mpsc::channel();
  thread::spawn(move || {
    let _ = late.send(1);
    let _ = done_tx.send(());
  });
  thread::sleep(Duration::from_millis(50));
  while rx.try_recv().is_ok() {}
  assert!(
    done_rx.recv_timeout(Duration::from_secs(2)).is_ok(),
    "a sender parked after the unwind was not woken when the receiver drained"
  );
}

#[test]
fn a_parked_sender_is_woken_after_send_batch_unwinds_from_a_receiver_waker() {
  run(false);
}

#[test]
fn a_parked_sender_is_woken_after_send_batch_mut_unwinds_from_a_receiver_waker() {
  run(true);
}
