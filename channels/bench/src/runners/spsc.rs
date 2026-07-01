use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use fibre::spsc;

use crate::watchdog::{self, WatchdogConfig};
use super::{BenchConfig, RunResult};

// spsc: Sender and Receiver are NOT Clone (single producer, single consumer).
// Only bounded flavor is supported.

pub fn run_sync(cfg: &BenchConfig) -> RunResult {
  let sent = Arc::new(AtomicUsize::new(0));
  let received = Arc::new(AtomicUsize::new(0));
  let done = Arc::new(AtomicBool::new(false));

  let wd = watchdog::spawn(
    sent.clone(),
    received.clone(),
    done.clone(),
    None,
    WatchdogConfig {
      stall_ms: cfg.stall_ms,
      stall_count: cfg.stall_count,
      expected: cfg.items,
    },
  );

  let (tx, rx) = spsc::bounded_sync::<usize>(cfg.capacity);

  let start = Instant::now();
  let items = cfg.items;
  let batch_size = cfg.batch_size;

  let sent2 = sent.clone();
  let producer = thread::spawn(move || {
    if batch_size == 0 {
      for i in 0..items {
        let _ = tx.send(i);
        sent2.fetch_add(1, Ordering::Relaxed);
      }
    } else {
      let mut remaining = items;
      while remaining > 0 {
        let chunk_size = remaining.min(batch_size);
        match tx.send_batch(vec![0usize; chunk_size]) {
          Ok(n) => {
            sent2.fetch_add(n, Ordering::Relaxed);
            remaining -= n;
          }
          Err(_) => break,
        }
      }
    }
  });

  let received2 = received.clone();
  let consumer = thread::spawn(move || {
    if batch_size == 0 {
      while rx.recv().is_ok() {
        received2.fetch_add(1, Ordering::Relaxed);
      }
    } else {
      while let Ok(batch) = rx.recv_batch(batch_size) {
        received2.fetch_add(batch.len(), Ordering::Relaxed);
      }
    }
  });

  producer.join().unwrap();
  consumer.join().unwrap();

  done.store(true, Ordering::Relaxed);
  let watchdog_ticks = wd.join().unwrap_or_default();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
    watchdog_ticks,
  }
}

pub fn run_async(cfg: &BenchConfig) -> RunResult {
  let sent = Arc::new(AtomicUsize::new(0));
  let received = Arc::new(AtomicUsize::new(0));
  let done = Arc::new(AtomicBool::new(false));

  let wd = watchdog::spawn(
    sent.clone(),
    received.clone(),
    done.clone(),
    None,
    WatchdogConfig {
      stall_ms: cfg.stall_ms,
      stall_count: cfg.stall_count,
      expected: cfg.items,
    },
  );

  let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .unwrap();

  let start = Instant::now();

  rt.block_on(async {
    let (mut tx, mut rx) = spsc::bounded_async::<usize>(cfg.capacity);
    let items = cfg.items;
    let batch_size = cfg.batch_size;

    let sent2 = sent.clone();
    let producer = tokio::spawn(async move {
      if batch_size == 0 {
        for i in 0..items {
          let _ = tx.send(i).await;
          sent2.fetch_add(1, Ordering::Relaxed);
        }
      } else {
        let mut remaining = items;
        while remaining > 0 {
          let chunk_size = remaining.min(batch_size);
          match tx.send_batch(vec![0usize; chunk_size]).await {
            Ok(n) => {
              sent2.fetch_add(n, Ordering::Relaxed);
              remaining -= n;
            }
            Err(_) => break,
          }
        }
      }
    });

    let received2 = received.clone();
    let consumer = tokio::spawn(async move {
      if batch_size == 0 {
        while rx.recv().await.is_ok() {
          received2.fetch_add(1, Ordering::Relaxed);
        }
      } else {
        while let Ok(batch) = rx.recv_batch(batch_size).await {
          received2.fetch_add(batch.len(), Ordering::Relaxed);
        }
      }
    });

    producer.await.unwrap();
    consumer.await.unwrap();
  });

  done.store(true, Ordering::Relaxed);
  let watchdog_ticks = wd.join().unwrap_or_default();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
    watchdog_ticks,
  }
}
