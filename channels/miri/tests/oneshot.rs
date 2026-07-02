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
  assert_eq!(drops(&counter), 1); // the losing value came back and dropped
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
