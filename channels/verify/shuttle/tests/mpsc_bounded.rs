//! Shuttle models of the bounded MPSC: many producers at small capacities,
//! explored with the PCT and random schedulers. A lost wakeup shows up as a
//! shuttle deadlock report with the schedule that produced it.

#![cfg(shuttle)]

use fibre::mpsc::{bounded, bounded_async};
use shuttle::{check_pct, check_random, future, thread};

const PRODUCERS: u32 = 3;
const PER_PRODUCER: u32 = 2;

fn assert_per_producer_fifo(got: &[u32]) {
  assert_eq!(got.len() as u32, PRODUCERS * PER_PRODUCER);
  for p in 0..PRODUCERS {
    let seq: Vec<u32> = got.iter().copied().filter(|v| v / 10 == p).collect();
    let want: Vec<u32> = (0..PER_PRODUCER).map(|i| p * 10 + i).collect();
    assert_eq!(seq, want, "producer {p} out of order");
  }
}

fn sync_many_producers(cap: usize) {
  let (tx, rx) = bounded::<u32>(cap);
  let handles: Vec<_> = (0..PRODUCERS)
    .map(|p| {
      let tx = tx.clone();
      thread::spawn(move || {
        for i in 0..PER_PRODUCER {
          tx.send(p * 10 + i).unwrap();
        }
      })
    })
    .collect();
  drop(tx);
  let mut got = Vec::new();
  while let Ok(v) = rx.recv() {
    got.push(v);
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_per_producer_fifo(&got);
}

fn async_many_producers(cap: usize) {
  future::block_on(async move {
    let (tx, rx) = bounded_async::<u32>(cap);
    let handles: Vec<_> = (0..PRODUCERS)
      .map(|p| {
        let tx = tx.clone();
        future::spawn(async move {
          for i in 0..PER_PRODUCER {
            tx.send(p * 10 + i).await.unwrap();
          }
        })
      })
      .collect();
    drop(tx);
    let mut got = Vec::new();
    while let Ok(v) = rx.recv().await {
      got.push(v);
    }
    for h in handles {
      h.await.unwrap();
    }
    assert_per_producer_fifo(&got);
  });
}

/// The full-drain regression: the channel is full, a producer blocks on one
/// more send, the consumer takes a single item and then waits for the producer
/// to acknowledge. The blocked send must complete off that single drain.
fn single_drain_releases_blocked_sender() {
  let (tx, rx) = bounded::<u32>(2);
  let (ack_tx, ack_rx) = bounded::<()>(1);
  tx.send(0).unwrap();
  tx.send(1).unwrap();
  let producer = thread::spawn(move || {
    tx.send(2).unwrap();
    ack_tx.send(()).unwrap();
  });
  assert_eq!(rx.recv().unwrap(), 0);
  ack_rx.recv().unwrap();
  assert_eq!(rx.recv().unwrap(), 1);
  assert_eq!(rx.recv().unwrap(), 2);
  producer.join().unwrap();
}

#[test]
fn sync_cap1_pct() {
  check_pct(|| sync_many_producers(1), 20_000, 3);
}

#[test]
fn sync_cap2_pct() {
  check_pct(|| sync_many_producers(2), 20_000, 3);
}

#[test]
fn async_cap1_pct() {
  check_pct(|| async_many_producers(1), 20_000, 3);
}

#[test]
fn async_cap2_pct() {
  check_pct(|| async_many_producers(2), 20_000, 3);
}

#[test]
fn async_cap1_random() {
  check_random(|| async_many_producers(1), 20_000);
}

#[test]
fn single_drain_releases_blocked_sender_pct() {
  check_pct(single_drain_releases_blocked_sender, 20_000, 3);
}

/// Async sender parked on a full cap-2 channel, released by one sync recv.
fn single_drain_releases_blocked_async_sender() {
  let (tx, rx) = bounded::<u32>(2);
  tx.try_send(1).unwrap();
  tx.try_send(2).unwrap();
  let atx = tx.to_async();
  let producer = thread::spawn(move || future::block_on(atx.send(3)).unwrap());
  assert_eq!(rx.recv().unwrap(), 1);
  producer.join().unwrap();
  assert_eq!(rx.recv().unwrap(), 2);
  assert_eq!(rx.recv().unwrap(), 3);
}

#[test]
fn single_drain_releases_blocked_async_sender_pct() {
  check_pct(single_drain_releases_blocked_async_sender, 20_000, 3);
}

/// Wider sync model: 4 producers with 3 items each, so the token holder's
/// release-after-send can race a drain while another sender is parked.
fn sync_wide(cap: usize) {
  let (tx, rx) = bounded::<u32>(cap);
  let handles: Vec<_> = (0..4u32)
    .map(|p| {
      let tx = tx.clone();
      thread::spawn(move || {
        for i in 0..3 {
          tx.send(p * 10 + i).unwrap();
        }
      })
    })
    .collect();
  drop(tx);
  let mut n = 0;
  while rx.recv().is_ok() {
    n += 1;
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(n, 12);
}

#[test]
fn sync_wide_cap1_pct_depth5() {
  check_pct(|| sync_wide(1), 50_000, 5);
}

#[test]
fn sync_wide_cap2_pct_depth5() {
  check_pct(|| sync_wide(2), 50_000, 5);
}

#[test]
fn sync_wide_cap1_random() {
  check_random(|| sync_wide(1), 50_000);
}

/// Batch senders at small capacities: each producer sends its items as one
/// `send_batch`, the consumer drains one at a time.
fn sync_batch(cap: usize) {
  let (tx, rx) = bounded::<u32>(cap);
  let handles: Vec<_> = (0..PRODUCERS)
    .map(|p| {
      let tx = tx.clone();
      thread::spawn(move || {
        tx.send_batch((0..PER_PRODUCER + 1).map(|i| p * 10 + i).collect()).unwrap();
      })
    })
    .collect();
  drop(tx);
  let mut n = 0;
  while rx.recv().is_ok() {
    n += 1;
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(n, PRODUCERS * (PER_PRODUCER + 1));
}

fn async_batch(cap: usize) {
  future::block_on(async move {
    let (tx, rx) = bounded_async::<u32>(cap);
    let handles: Vec<_> = (0..PRODUCERS)
      .map(|p| {
        let tx = tx.clone();
        future::spawn(async move {
          tx.send_batch((0..PER_PRODUCER + 1).map(|i| p * 10 + i).collect()).await.unwrap();
        })
      })
      .collect();
    drop(tx);
    let mut n = 0;
    while rx.recv().await.is_ok() {
      n += 1;
    }
    for h in handles {
      h.await.unwrap();
    }
    assert_eq!(n, PRODUCERS * (PER_PRODUCER + 1));
  });
}

#[test]
fn sync_batch_cap1_pct() {
  check_pct(|| sync_batch(1), 20_000, 3);
}

#[test]
fn sync_batch_cap2_pct() {
  check_pct(|| sync_batch(2), 20_000, 3);
}

#[test]
fn async_batch_cap1_pct() {
  check_pct(|| async_batch(1), 20_000, 3);
}

#[test]
fn async_batch_cap2_pct() {
  check_pct(|| async_batch(2), 20_000, 3);
}
