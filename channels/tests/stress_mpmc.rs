#![cfg(not(miri))]

//! Total Test Plan: Interleaved API Fuzzing for MPMC v2.
//!
//! This suite runs highly chaotic, randomized workflows where every thread/task
//! selects a completely different API (send, try_send, send_batch, etc.) and
//! batch size on every single iteration. It proves that the mixed paradigm
//! queues cooperate perfectly under extreme fragmentation and backpressure.

use fibre::error::{SendBatchError, SendError};
use fibre::mpmc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Minimal, fast, deterministic PRNG to safely generate chaos per-thread.
struct Lcg(usize);
impl Lcg {
  fn new(seed: usize) -> Self {
    Lcg(seed.max(1))
  }
  fn next(&mut self) -> usize {
    let mut x = self.0 as u64;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    self.0 = x as usize;
    self.0
  }
  fn range(&mut self, lo: usize, hi: usize) -> usize {
    if lo >= hi {
      lo
    } else {
      lo + self.next() % (hi - lo)
    }
  }
}

// ============================================================================
// Phase 1 & 2: The Core Fuzzer Engine & Matrix
// ============================================================================

async fn run_async_fuzzer(
  cap: usize,
  num_producers: usize,
  num_consumers: usize,
  items_per: usize,
) {
  let (tx, rx) = mpmc::bounded_async::<usize>(cap);
  let total_items = num_producers * items_per;

  // Lock-free validation array to track exactly which items are received
  let seen = Arc::new(
    (0..total_items)
      .map(|_| AtomicBool::new(false))
      .collect::<Vec<_>>(),
  );
  let received_count = Arc::new(AtomicUsize::new(0));

  let mut handles = Vec::new();

  // SPAM PRODUCERS
  for p in 0..num_producers {
    let tx = tx.clone();
    handles.push(tokio::spawn(async move {
      let mut rng = Lcg::new(0x1337 + p);
      let base = p * items_per;
      let end = base + items_per;
      let mut next_item = base;

      while next_item < end {
        let max_batch = cap.max(1) * 2; // Intentionally overshoot capacity
        let batch_size = rng.range(1, max_batch).min(end - next_item);

        match rng.range(0, 5) {
          0 => {
            if tx.send(next_item).await.is_ok() {
              next_item += 1;
            }
          }
          1 => {
            if tx.try_send(next_item).is_ok() {
              next_item += 1;
            } else {
              tokio::task::yield_now().await;
            }
          }
          2 => {
            let batch: Vec<_> = (next_item..next_item + batch_size).collect();
            if let Ok(n) = tx.send_batch(batch).await {
              next_item += n;
            }
          }
          3 => {
            let mut batch: Vec<_> = (next_item..next_item + batch_size).collect();
            if tx.send_batch_mut(&mut batch).await.is_ok() {
              next_item += batch_size - batch.len();
            }
          }
          4 => {
            let mut batch: Vec<_> = (next_item..next_item + batch_size).collect();
            if let Ok(n) = tx.try_send_batch_mut(&mut batch) {
              next_item += n;
            } else {
              tokio::task::yield_now().await;
            }
          }
          _ => unreachable!(),
        }
      }
    }));
  }
  drop(tx);

  // SPAM CONSUMERS
  for c in 0..num_consumers {
    let rx = rx.clone();
    let seen = seen.clone();
    let received_count = received_count.clone();

    handles.push(tokio::spawn(async move {
      let mut rng = Lcg::new(0x9000 + c);

      loop {
        let max_batch = rng.range(1, cap.max(1) * 2).max(5);
        let mut got = Vec::new();

        match rng.range(0, 5) {
          0 => {
            match rx.recv().await {
              Ok(v) => got.push(v),
              Err(_) => break, // Channel closed
            }
          }
          1 => match rx.try_recv() {
            Ok(v) => got.push(v),
            Err(fibre::error::TryRecvError::Disconnected) => break,
            Err(_) => {
              tokio::task::yield_now().await;
              continue;
            }
          },
          2 => match rx.recv_batch(max_batch).await {
            Ok(batch) => got.extend(batch),
            Err(_) => break,
          },
          3 => match rx.recv_batch_mut(&mut got, max_batch).await {
            Ok(_) => {}
            Err(_) => break,
          },
          4 => match rx.try_recv_batch_mut(&mut got, max_batch) {
            Ok(_) => {}
            Err(fibre::error::TryRecvError::Disconnected) => break,
            Err(_) => {
              tokio::task::yield_now().await;
              continue;
            }
          },
          _ => unreachable!(),
        }

        for val in got {
          let old = seen[val].swap(true, Ordering::Relaxed);
          assert!(!old, "FATAL: Duplicate item received: {}", val);
          received_count.fetch_add(1, Ordering::Relaxed);
        }
      }
    }));
  }
  drop(rx);

  // Watchdog
  let _ = tokio::time::timeout(WATCHDOG_TIMEOUT, async move {
    for h in handles {
      h.await.unwrap();
    }
  })
  .await
  .expect("TEST TIMED OUT: A lost wakeup occurred during API interleaving!");

  assert_eq!(
    received_count.load(Ordering::Relaxed),
    total_items,
    "Missing items!"
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_matrix_fuzz_async_apis() {
  // Tiny (Cap 2), Unaligned (Cap 13), Large (Cap 513/1024)
  for cap in [2, 13, 513, 1024] {
    run_async_fuzzer(cap, 8, 8, 1_000_000).await;
  }
}

#[test]
fn test_matrix_fuzz_sync_apis() {
  fn run_sync_fuzzer(cap: usize, num_producers: usize, num_consumers: usize, items_per: usize) {
    let (tx, rx) = mpmc::bounded::<usize>(cap);
    let total_items = num_producers * items_per;
    let seen = Arc::new(
      (0..total_items)
        .map(|_| AtomicBool::new(false))
        .collect::<Vec<_>>(),
    );
    let received_count = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    // SPAM PRODUCERS
    for p in 0..num_producers {
      let tx = tx.clone();
      handles.push(std::thread::spawn(move || {
        let mut rng = Lcg::new(0x1337 + p);
        let base = p * items_per;
        let end = base + items_per;
        let mut next_item = base;

        while next_item < end {
          let max_batch = cap.max(1) * 2;
          let batch_size = rng.range(1, max_batch).min(end - next_item);

          match rng.range(0, 5) {
            0 => {
              if tx.send(next_item).is_ok() {
                next_item += 1;
              }
            }
            1 => {
              if tx.try_send(next_item).is_ok() {
                next_item += 1;
              } else {
                std::thread::yield_now();
              }
            }
            2 => {
              let batch: Vec<_> = (next_item..next_item + batch_size).collect();
              if let Ok(n) = tx.send_batch(batch) {
                next_item += n;
              }
            }
            3 => {
              let mut batch: Vec<_> = (next_item..next_item + batch_size).collect();
              if tx.send_batch_mut(&mut batch).is_ok() {
                next_item += batch_size - batch.len();
              }
            }
            4 => {
              let mut batch: Vec<_> = (next_item..next_item + batch_size).collect();
              if let Ok(n) = tx.try_send_batch_mut(&mut batch) {
                next_item += n;
              } else {
                std::thread::yield_now();
              }
            }
            _ => unreachable!(),
          }
        }
      }));
    }
    drop(tx);

    // SPAM CONSUMERS
    for c in 0..num_consumers {
      let rx = rx.clone();
      let seen = seen.clone();
      let received_count = received_count.clone();

      handles.push(std::thread::spawn(move || {
        let mut rng = Lcg::new(0x9000 + c);
        loop {
          let max_batch = rng.range(1, cap.max(1) * 2).max(5);
          let mut got = Vec::new();

          match rng.range(0, 5) {
            0 => {
              if let Ok(v) = rx.recv() {
                got.push(v);
              } else {
                break;
              }
            }
            1 => match rx.try_recv() {
              Ok(v) => got.push(v),
              Err(fibre::error::TryRecvError::Disconnected) => break,
              Err(_) => {
                std::thread::yield_now();
                continue;
              }
            },
            2 => {
              if let Ok(batch) = rx.recv_batch(max_batch) {
                got.extend(batch);
              } else {
                break;
              }
            }
            3 => {
              if rx.recv_batch_mut(&mut got, max_batch).is_err() {
                break;
              }
            }
            4 => match rx.try_recv_batch_mut(&mut got, max_batch) {
              Ok(_) => {}
              Err(fibre::error::TryRecvError::Disconnected) => break,
              Err(_) => {
                std::thread::yield_now();
                continue;
              }
            },
            _ => unreachable!(),
          }

          for val in got {
            let old = seen[val].swap(true, Ordering::Relaxed);
            assert!(!old, "FATAL: Duplicate item received: {}", val);
            received_count.fetch_add(1, Ordering::Relaxed);
          }
        }
      }));
    }
    drop(rx);

    // Standard thread join
    for h in handles {
      h.join().unwrap();
    }
    assert_eq!(
      received_count.load(Ordering::Relaxed),
      total_items,
      "Missing items!"
    );
  }

  for cap in [2, 13, 1024] {
    run_sync_fuzzer(cap, 8, 8, 1_000_000);
  }
}

// ============================================================================
// Phase 4: Destructive Interleaving (Cancel-Safety & Mid-Batch Drop)
// ============================================================================

#[tokio::test]
async fn test_cancel_safety_timeout_masking() {
  let (tx, rx) = mpmc::bounded_async::<usize>(2);

  // Fill the channel
  tx.send(1).await.unwrap();
  tx.send(2).await.unwrap();

  let mut items = vec![3, 4, 5, 6, 7];

  // Run an aggressive timeout that cancels the future after partial execution
  let fut = tx.send_batch_mut(&mut items);
  let res = tokio::time::timeout(Duration::from_millis(50), fut).await;

  // It must have timed out because cap is 2, the channel is full, and we tried to send 5
  assert!(res.is_err(), "Future did not timeout!");

  // Validate state integrity
  assert_eq!(
    items,
    vec![3, 4, 5, 6, 7],
    "Items were lost during cancellation!"
  );
  assert_eq!(rx.recv().await.unwrap(), 1);
  assert_eq!(rx.recv().await.unwrap(), 2);

  // Must be completely functional afterward.
  // Spawn a consumer so `send_batch_mut(5)` doesn't deadlock on a cap=2 channel!
  let consumer = tokio::spawn(async move {
    let mut got = Vec::new();
    while got.len() < 5 {
      got.push(rx.recv().await.unwrap());
    }
    got
  });

  assert_eq!(tx.send_batch_mut(&mut items).await.unwrap(), 5);
  assert_eq!(consumer.await.unwrap(), vec![3, 4, 5, 6, 7]);
}

#[tokio::test]
async fn test_mid_batch_receiver_disconnection() {
  let (tx, rx) = mpmc::bounded_async::<usize>(10);

  let send_task = tokio::spawn(async move {
    // Will send 10 and block waiting for space for the rest
    tx.send_batch((1..=100).collect()).await
  });

  // Wait to ensure producer is parked mid-batch
  tokio::time::sleep(Duration::from_millis(50)).await;

  // Destroy the receiver violently
  drop(rx);

  let res = send_task.await.unwrap();
  match res {
    Err(SendBatchError { sent, unsent }) => {
      assert_eq!(sent, 10, "Expected exactly 10 sent before drop");
      assert_eq!(
        unsent.len(),
        90,
        "Expected exactly 90 unsent items returned"
      );
      assert_eq!(unsent[0], 11, "Sequence must be perfectly preserved");
    }
    Ok(_) => panic!("Send incorrectly succeeded after channel was destroyed!"),
  }
}
