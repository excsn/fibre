//! Miri suite for the oneshot channel: single ownership transfer, competing
//! sender clones, unconsumed-value drop.

use fibre::error::TryRecvError;
use fibre::oneshot;
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter};

use std::pin::pin;
use std::thread;

#[test]
fn send_then_recv() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  tx.send(42).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 42);
}

#[test]
fn recv_registers_then_cancels() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  {
    let mut fut = pin!(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
  }
  tx.send(7).unwrap();
  assert_eq!(block_on(rx.recv()).unwrap(), 7);
}

#[test]
fn only_first_of_competing_clones_wins() {
  let counter = drop_counter();
  let (tx, rx) = oneshot::oneshot();
  let tx2 = tx.clone();
  tx.send(DropCounter::new(&counter)).unwrap();
  let second = tx2.send(DropCounter::new(&counter));
  assert!(second.is_err());
  // The losing value rides back inside the TrySendError; it only drops once
  // the error itself is dropped.
  drop(second);
  assert_eq!(drops(&counter), 1);
  drop(block_on(rx.recv()).unwrap());
  assert_eq!(drops(&counter), 2);
}

#[test]
fn unconsumed_value_dropped_with_channel() {
  let counter = drop_counter();
  {
    let (tx, rx) = oneshot::oneshot();
    tx.send(DropCounter::new(&counter)).unwrap();
    drop(rx);
  }
  assert_eq!(drops(&counter), 1);
}

#[test]
fn sender_drop_disconnects() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  drop(tx);
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
}

#[test]
fn try_recv_empty_then_value() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  tx.send(5).unwrap();
  assert_eq!(rx.try_recv().unwrap(), 5);
}

#[test]
fn cross_thread_completion() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  let sender = thread::spawn(move || {
    tx.send(99).unwrap();
  });
  assert_eq!(block_on(rx.recv()).unwrap(), 99);
  sender.join().unwrap();
}

#[test]
fn spinning_try_recv_races_send_and_sender_drop() {
  let (tx, rx) = oneshot::oneshot::<u32>();
  let sender = thread::spawn(move || tx.send(1).unwrap());
  loop {
    match rx.try_recv() {
      Ok(v) => {
        assert_eq!(v, 1);
        break;
      }
      Err(TryRecvError::Empty) => thread::yield_now(),
      Err(TryRecvError::Disconnected) => panic!("false disconnect after successful send"),
    }
  }
  sender.join().unwrap();
}

#[test]
fn receiver_drop_races_send() {
  let counter = drop_counter();
  let (tx, rx) = oneshot::oneshot();
  let value = DropCounter::new(&counter);
  let sender = thread::spawn(move || {
    let _ = tx.send(value);
  });
  drop(rx);
  sender.join().unwrap();
  assert_eq!(drops(&counter), 1);
}

mod exclusive {
  use super::*;

  #[test]
  fn send_then_recv() {
    let (tx, mut rx) = oneshot::exclusive::<u32>();
    tx.send(42).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), 42);
  }

  #[test]
  fn recv_registers_then_cancels() {
    let (tx, mut rx) = oneshot::exclusive::<u32>();
    {
      let mut fut = pin!(rx.recv());
      assert!(poll_once(fut.as_mut()).is_pending());
    }
    tx.send(7).unwrap();
    assert_eq!(block_on(rx.recv()).unwrap(), 7);
  }

  #[test]
  fn rejected_send_returns_value() {
    let counter = drop_counter();
    let (tx, rx) = oneshot::exclusive();
    drop(rx);
    let rejected = tx.send(DropCounter::new(&counter));
    assert!(rejected.is_err());
    drop(rejected);
    assert_eq!(drops(&counter), 1);
  }

  #[test]
  fn unconsumed_value_dropped_with_receiver() {
    let counter = drop_counter();
    {
      let (tx, rx) = oneshot::exclusive();
      tx.send(DropCounter::new(&counter)).unwrap();
      drop(rx);
    }
    assert_eq!(drops(&counter), 1);
  }

  #[test]
  fn sender_drop_disconnects() {
    let (tx, mut rx) = oneshot::exclusive::<u32>();
    drop(tx);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[test]
  fn cross_thread_completion() {
    let (tx, mut rx) = oneshot::exclusive::<u32>();
    let sender = thread::spawn(move || {
      tx.send(99).unwrap();
    });
    assert_eq!(block_on(rx.recv()).unwrap(), 99);
    sender.join().unwrap();
  }

  #[test]
  fn send_races_receiver_drop() {
    let counter = drop_counter();
    let (tx, rx) = oneshot::exclusive();
    let value = DropCounter::new(&counter);
    let sender = thread::spawn(move || {
      let _ = tx.send(value);
    });
    drop(rx);
    sender.join().unwrap();
    assert_eq!(drops(&counter), 1);
  }
}
