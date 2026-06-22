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

  let sent2 = sent.clone();
  let producer = thread::spawn(move || {
    for i in 0..items {
      let _ = tx.send(i);
      sent2.fetch_add(1, Ordering::Relaxed);
    }
  });

  let received2 = received.clone();
  let consumer = thread::spawn(move || {
    while rx.recv().is_ok() {
      received2.fetch_add(1, Ordering::Relaxed);
    }
  });

  producer.join().unwrap();
  consumer.join().unwrap();

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
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
    let (tx, rx) = spsc::bounded_async::<usize>(cfg.capacity);
    let items = cfg.items;

    let sent2 = sent.clone();
    let producer = tokio::spawn(async move {
      for i in 0..items {
        let _ = tx.send(i).await;
        sent2.fetch_add(1, Ordering::Relaxed);
      }
    });

    let received2 = received.clone();
    let consumer = tokio::spawn(async move {
      while rx.recv().await.is_ok() {
        received2.fetch_add(1, Ordering::Relaxed);
      }
    });

    producer.await.unwrap();
    consumer.await.unwrap();
  });

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
  }
}
