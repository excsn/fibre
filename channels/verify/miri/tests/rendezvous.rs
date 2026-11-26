//! Miri suite for the zero-capacity rendezvous channels (spsc, mpsc, mpmc
//! variants): direct handoff always requires both sides live, so every test is
//! cross-thread by construction.

use fibre::{mpmc, mpsc, spsc};
use fibre_miri::{block_on, drop_counter, drops, poll_once, DropCounter, ITEMS_SMALL};

use std::thread;

#[test]
fn spsc_rendezvous_handoff() {
  let (tx, mut rx) = spsc::rendezvous::rendezvous::<usize>();
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
  let (tx, mut rx) = spsc::rendezvous::rendezvous_async::<usize>();
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
  let (tx, mut rx) = mpsc::rendezvous::rendezvous::<usize>();
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
  let (tx, mut rx) = mpsc::rendezvous::rendezvous_async::<usize>();
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
  let (tx, mut rx) = spsc::rendezvous::rendezvous::<u32>();
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

#[test]
fn mpmc_item_returned_by_a_dropped_recv_future_is_received_later() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<Box<usize>>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
    assert!(tx.try_send(Box::new(5)).is_ok());
  }
  assert_eq!(*rx.try_recv().unwrap(), 5);
}

#[test]
fn mpsc_item_returned_by_a_dropped_recv_future_is_received_later() {
  let (tx, mut rx) = mpsc::rendezvous::rendezvous_async::<Box<usize>>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
    assert!(tx.try_send(Box::new(5)).is_ok());
  }
  assert_eq!(*rx.try_recv().unwrap(), 5);
}

#[test]
fn spsc_item_returned_by_a_dropped_recv_future_is_received_later() {
  let (tx, mut rx) = spsc::rendezvous::rendezvous_async::<Box<usize>>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
    assert!(tx.try_send(Box::new(5)).is_ok());
  }
  assert_eq!(*rx.try_recv().unwrap(), 5);
}

#[test]
fn returned_item_is_dropped_once_when_the_channel_goes() {
  let counter = drop_counter();
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<DropCounter>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
    assert!(tx.try_send(DropCounter::new(&counter)).is_ok());
  }
  drop(rx);
  drop(tx);
  assert_eq!(drops(&counter), 1);
}

#[test]
fn returned_item_crosses_to_a_receiver_on_another_thread() {
  let (tx, rx) = mpmc::rendezvous::rendezvous_async::<Box<usize>>();
  {
    let mut fut = Box::pin(rx.recv());
    assert!(poll_once(fut.as_mut()).is_pending());
    assert!(tx.try_send(Box::new(9)).is_ok());
  }
  let consumer = thread::spawn(move || *block_on(rx.recv()).unwrap());
  assert_eq!(consumer.join().unwrap(), 9);
  drop(tx);
}

macro_rules! repolled_future_tests {
  ($module:ident, $flavor:path) => {
    mod $module {
      use fibre_miri::poll_once;
      use std::pin::pin;
      use std::task::Poll;
      use $flavor as rv;

      #[test]
      fn a_recv_future_polled_again_is_filled_through_its_first_registration() {
        let (tx, mut rx) = rv::rendezvous_async::<Box<u32>>();
        let mut recv = pin!(rx.recv());
        assert!(poll_once(recv.as_mut()).is_pending());
        assert!(poll_once(recv.as_mut()).is_pending());
        assert!(tx.try_send(Box::new(5)).is_ok());
        assert_eq!(poll_once(recv.as_mut()), Poll::Ready(Ok(Box::new(5))));
      }

      #[test]
      fn a_send_future_polled_again_is_taken_through_its_first_registration() {
        let (tx, mut rx) = rv::rendezvous_async::<Box<u32>>();
        let mut send = pin!(tx.send(Box::new(9)));
        assert!(poll_once(send.as_mut()).is_pending());
        assert!(poll_once(send.as_mut()).is_pending());
        assert_eq!(rx.try_recv(), Ok(Box::new(9)));
        assert_eq!(poll_once(send.as_mut()), Poll::Ready(Ok(())));
      }

      #[test]
      fn a_filled_recv_future_dropped_after_a_second_poll_passes_its_item_on() {
        let (tx, mut rx) = rv::rendezvous_async::<Box<u32>>();
        {
          let mut recv = pin!(rx.recv());
          assert!(poll_once(recv.as_mut()).is_pending());
          assert!(poll_once(recv.as_mut()).is_pending());
          assert!(tx.try_send(Box::new(3)).is_ok());
        }
        let mut next = pin!(rx.recv());
        assert_eq!(poll_once(next.as_mut()), Poll::Ready(Ok(Box::new(3))));
      }
    }
  };
}

repolled_future_tests!(spsc_repolled, fibre::spsc::rendezvous);
repolled_future_tests!(mpsc_repolled, fibre::mpsc::rendezvous);
repolled_future_tests!(mpmc_repolled, fibre::mpmc::rendezvous);
