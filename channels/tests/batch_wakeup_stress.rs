//! Stress tests hunting a suspected lost-wakeup in the batch send/recv paths.
//!
//! Models the downstream (rzmq) workload that reportedly stalls: N async
//! producers pushing small bursts via `send_batch_mut` into one bounded async
//! MPMC channel, a single consumer doing `recv_batch(64)` + sleep. Producers
//! finish and go idle *without dropping their handles* (a sender-drop would
//! force-wake the consumer and mask a parking race), then the consumer must
//! still drain everything.
//!
//! Each test is wrapped in a watchdog that panics with diagnostics instead of
//! hanging the suite. Run in a loop to hunt the race:
//!
//! ```sh
//! for i in $(seq 1 200); do
//!   cargo test --release --test batch_wakeup_stress -- --test-threads=2 || break
//! done
//! ```

#![cfg(not(miri))]

use fibre::{mpmc, mpsc};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const WATCHDOG: Duration = Duration::from_secs(30);

/// Tiny deterministic PRNG (xorshift64*), `Send` and reproducible per task.
struct Lcg(u64);

impl Lcg {
  fn new(seed: u64) -> Self {
    Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).max(1))
  }
  fn next(&mut self) -> u64 {
    let mut x = self.0;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    self.0 = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
  }
  /// Uniform-ish value in `lo..=hi`.
  fn range(&mut self, lo: u64, hi: u64) -> u64 {
    lo + self.next() % (hi - lo + 1)
  }
  /// True with probability ~`percent`/100.
  fn chance(&mut self, percent: u64) -> bool {
    self.next() % 100 < percent
  }
}

/// Faithful repro of the downstream workload shape.
/// 16 producers x 200 items in bursts of 1..=8 via send_batch_mut, one
/// consumer recv_batch(64) + sleep. Senders stay alive until full delivery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpmc_burst_idle_consumer_must_drain() {
  run_mpmc_burst(2000, 16, 200, ConsumerMode::PlainWithSleep(Duration::from_millis(2))).await;
}

/// Variant (a): the consumer's recv_batch is wrapped in a short timeout and
/// frequently cancelled, interleaved with occasional try_recv_batch_mut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpmc_burst_with_timeout_cancelled_recv_batch() {
  run_mpmc_burst(2000, 16, 200, ConsumerMode::TimeoutCancel(Duration::from_millis(2))).await;
}

/// Variant (b): tiny capacity so producers park at the channel constantly,
/// stressing the batch-send park path and the batch-recv coalesced wake of
/// freed senders.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpmc_small_capacity_producer_parking() {
  run_mpmc_burst(8, 16, 200, ConsumerMode::PlainWithSleep(Duration::from_micros(200))).await;
}

/// High-frequency park/wake: consumer never sleeps, so it parks on an empty
/// queue thousands of times per second and every producer burst must re-wake
/// it. This maximizes pressure on the check/register/wake protocol.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpmc_high_frequency_park_wake() {
  run_mpmc_burst(2000, 8, 400, ConsumerMode::Plain).await;
}

enum ConsumerMode {
  Plain,
  PlainWithSleep(Duration),
  TimeoutCancel(Duration),
}

async fn run_mpmc_burst(cap: usize, producers: usize, items_per_producer: usize, mode: ConsumerMode) {
  let (tx, rx) = mpmc::bounded_async::<u64>(cap);
  let expected_total = producers * items_per_producer;
  let received = Arc::new(AtomicUsize::new(0));

  let mut producer_handles = Vec::new();
  // Keep a clone of every producer handle alive in the test body so the
  // channel can NEVER disconnect while the consumer drains. A disconnect
  // wakes the consumer and would mask a lost wakeup.
  let keepalive: Vec<_> = (0..producers).map(|_| tx.clone()).collect();

  for p in 0..producers {
    let tx = tx.clone();
    producer_handles.push(tokio::spawn(async move {
      let mut rng = Lcg::new(0xC0FFEE ^ (p as u64 + 1));
      let mut next: u64 = (p * items_per_producer) as u64;
      let end: u64 = next + items_per_producer as u64;
      while next < end {
        let burst = (rng.range(1, 8) as usize).min((end - next) as usize);
        let mut items: Vec<u64> = (next..next + burst as u64).collect();
        next += burst as u64;
        tx.send_batch_mut(&mut items).await.expect("receiver alive");
        assert!(items.is_empty());
        // Small idle gaps so the consumer repeatedly drains to empty and
        // parks; the next burst must then re-wake it.
        match rng.range(0, 2) {
          0 => {} // immediately continue
          1 => tokio::task::yield_now().await,
          _ => tokio::time::sleep(Duration::from_micros(rng.range(50, 500))).await,
        }
      }
    }));
  }
  drop(tx);

  let received_for_consumer = received.clone();
  let consumer = tokio::spawn(async move {
    let mut rng = Lcg::new(0xDEAD_BEEF);
    let mut count = 0usize;
    while count < expected_total {
      match &mode {
        ConsumerMode::Plain => {
          let got = rx.recv_batch(64).await.expect("senders alive");
          count += got.len();
        }
        ConsumerMode::PlainWithSleep(d) => {
          let got = rx.recv_batch(64).await.expect("senders alive");
          count += got.len();
          tokio::time::sleep(*d).await;
        }
        ConsumerMode::TimeoutCancel(d) => {
          // Cancel recv_batch futures aggressively; occasionally use the
          // try-batch op instead, mirroring the downstream call sites.
          if rng.chance(20) {
            let mut out = Vec::new();
            if let Ok(k) = rx.try_recv_batch_mut(&mut out, 64) {
              count += k;
            }
            tokio::task::yield_now().await;
          } else {
            match tokio::time::timeout(*d, rx.recv_batch(64)).await {
              Ok(Ok(got)) => count += got.len(),
              Ok(Err(e)) => panic!("unexpected recv error: {e:?}"),
              Err(_elapsed) => {} // cancelled; loop and try again
            }
          }
        }
      }
      received_for_consumer.store(count, Ordering::Relaxed);
    }
    count
  });

  let result = tokio::time::timeout(WATCHDOG, async {
    for h in producer_handles {
      h.await.unwrap();
    }
    consumer.await.unwrap()
  })
  .await;

  match result {
    Ok(count) => assert_eq!(count, expected_total),
    Err(_) => {
      let got = received.load(Ordering::Relaxed);
      let in_channel = keepalive[0].len();
      panic!(
        "STALL: consumer parked with items undelivered. received={got}/{expected_total}, \
         channel len={in_channel} (lost wakeup if len > 0)"
      );
    }
  }
  drop(keepalive);
}

/// Variant (c): mpsc bounded, sync producer thread using try_send_batch_mut,
/// async consumer alternating Stream::poll_next with try_recv_batch —
/// exercising the shared consumer waker across both call styles.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpsc_sync_producer_async_consumer_stream_mix() {
  use futures_util::StreamExt;

  const TOTAL: usize = 4000;
  const CAP: usize = 64;

  let (tx, rx) = mpsc::bounded_async::<u64>(CAP);
  let tx_sync = tx.clone().to_sync();
  let keepalive = tx; // keep an async sender alive: no disconnect masking

  let producer = std::thread::spawn(move || {
    let mut next: u64 = 0;
    let mut pending: Vec<u64> = Vec::new();
    while (next as usize) < TOTAL || !pending.is_empty() {
      if pending.is_empty() {
        let burst = (1 + (next % 7) as usize).min(TOTAL - next as usize);
        pending = (next..next + burst as u64).collect();
        next += burst as u64;
      }
      match tx_sync.try_send_batch_mut(&mut pending) {
        Ok(0) => std::thread::yield_now(), // full; retry
        Ok(_) => {}
        Err(e) => panic!("unexpected send error: {e:?}"),
      }
    }
  });

  let consumer = tokio::spawn(async move {
    let mut rx = rx;
    let mut rng = Lcg::new(0xFEED_FACE);
    let mut count = 0usize;
    while count < TOTAL {
      if rng.chance(50) {
        // Park via the Stream waker.
        let item = rx.next().await.expect("senders alive");
        let _ = item;
        count += 1;
      } else {
        // Drain via the try-batch op (no parking).
        match rx.try_recv_batch(32) {
          Ok(items) => count += items.len(),
          Err(_) => tokio::task::yield_now().await,
        }
      }
    }
    count
  });

  let result = tokio::time::timeout(WATCHDOG, consumer).await;
  match result {
    Ok(joined) => assert_eq!(joined.unwrap(), TOTAL),
    Err(_) => {
      panic!(
        "STALL: mpsc async consumer parked; channel len={} (lost wakeup if > 0)",
        keepalive.len()
      );
    }
  }
  producer.join().unwrap();
  drop(keepalive);
}

/// Variant (d): mpsc bounded, async producer send_batch with a sync consumer
/// on an OS thread using try_recv_batch_mut. The producer parks at the
/// capacity gate; every consumer drain must release permits and wake it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mpsc_async_batch_producer_sync_try_consumer() {
  const TOTAL: usize = 4000;
  const CAP: usize = 8; // tiny: producer parks at the gate constantly

  let (tx, rx) = mpsc::bounded_async::<u64>(CAP);
  let rx_sync = rx.to_sync();
  let keepalive = tx.clone();

  let producer = tokio::spawn(async move {
    let mut next: u64 = 0;
    while (next as usize) < TOTAL {
      let burst = (1 + (next % 7) as usize).min(TOTAL - next as usize);
      let items: Vec<u64> = (next..next + burst as u64).collect();
      next += burst as u64;
      let sent = tx.send_batch(items).await.expect("receiver alive");
      assert_eq!(sent, burst);
    }
  });

  let consumer = std::thread::spawn(move || {
    let mut count = 0usize;
    let mut out = Vec::new();
    while count < TOTAL {
      match rx_sync.try_recv_batch_mut(&mut out, 64) {
        Ok(k) => {
          count += k;
          out.clear();
        }
        Err(fibre::error::TryRecvError::Empty) => {
          std::thread::sleep(Duration::from_micros(100));
        }
        Err(e) => panic!("unexpected recv error: {e:?}"),
      }
    }
    count
  });

  let result = tokio::time::timeout(WATCHDOG, producer).await;
  match result {
    Ok(joined) => joined.unwrap(),
    Err(_) => {
      panic!(
        "STALL: async batch producer parked at the gate; channel len={} \
         (permits not released / sender not woken if < capacity)",
        keepalive.len()
      );
    }
  }
  assert_eq!(consumer.join().unwrap(), TOTAL);
  drop(keepalive);
}
