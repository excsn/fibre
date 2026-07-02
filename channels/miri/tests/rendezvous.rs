//! Miri suite for the zero-capacity rendezvous channels (spsc, mpsc, mpmc
//! variants): direct handoff always requires both sides live, so every test is
//! cross-thread by construction.

use fibre::{mpmc, mpsc, spsc};
use fibre_miri::{block_on, drop_counter, drops, DropCounter, ITEMS_SMALL};

use std::thread;

#[test]
fn spsc_rendezvous_handoff() {
  let (tx, rx) = spsc::rendezvous::rendezvous::<usize>();
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      tx.send(i).unwrap();
    }
  });
  for i in 0..ITEMS_SMALL {
    assert_eq!(rx.recv().unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn spsc_rendezvous_async_handoff() {
  let (tx, rx) = spsc::rendezvous::rendezvous_async::<usize>();
  let producer = thread::spawn(move || {
    for i in 0..ITEMS_SMALL {
      block_on(tx.send(i)).unwrap();
    }
  });
  for i in 0..ITEMS_SMALL {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn mpsc_rendezvous_two_producers() {
  let (tx, rx) = mpsc::rendezvous::rendezvous::<usize>();
  let tx2 = tx.clone();
  let p1 = thread::spawn(move || {
    for _ in 0..20 {
      tx.send(1).unwrap();
    }
  });
  let p2 = thread::spawn(move || {
    for _ in 0..20 {
      tx2.send(2).unwrap();
    }
  });
  let mut sum = 0;
  for _ in 0..40 {
    sum += rx.recv().unwrap();
  }
  assert_eq!(sum, 20 + 40);
  p1.join().unwrap();
  p2.join().unwrap();
}

#[test]
fn mpsc_rendezvous_async_handoff() {
  let (tx, rx) = mpsc::rendezvous::rendezvous_async::<usize>();
  let producer = thread::spawn(move || {
    for i in 0..20 {
      block_on(tx.send(i)).unwrap();
    }
  });
  for i in 0..20 {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn mpmc_rendezvous_async_handoff() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<usize>();
  let producer = thread::spawn(move || {
    for i in 0..20 {
      block_on(tx.send(i)).unwrap();
    }
  });
  for i in 0..20 {
    assert_eq!(block_on(rx.recv()).unwrap(), i);
  }
  producer.join().unwrap();
}

#[test]
fn blocked_receiver_unblocks_on_sender_drop() {
  let (tx, rx) = spsc::rendezvous::rendezvous::<u32>();
  let consumer = thread::spawn(move || rx.recv());
  drop(tx);
  assert!(consumer.join().unwrap().is_err());
}

#[test]
fn blocked_sender_unblocks_on_receiver_drop() {
  let (tx, rx) = spsc::rendezvous::rendezvous::<u32>();
  let producer = thread::spawn(move || tx.send(1));
  drop(rx);
  assert!(producer.join().unwrap().is_err());
}

#[test]
fn mpmc_rendezvous_pair_and_disconnect() {
  let counter = drop_counter();
  let (tx, rx) = mpmc::rendezvous::rendezvous();
  let c = counter.clone();
  let producer = thread::spawn(move || {
    for _ in 0..10 {
      tx.send(DropCounter::new(&c)).unwrap();
    }
  });
  for _ in 0..10 {
    drop(rx.recv().unwrap());
  }
  producer.join().unwrap();
  drop(rx);
  assert_eq!(drops(&counter), 10);
}
