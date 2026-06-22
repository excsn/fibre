use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use fibre::spmc;

use crate::watchdog::{self, WatchdogConfig};
use super::{BenchConfig, RunResult};

// spmc: Sender is NOT Clone (single producer), Receiver IS Clone (multi-consumer).
// Broadcast channel: every consumer receives every item.
// Total received = items * consumers when complete.

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
      expected: cfg.items * cfg.consumers,
    },
  );

  let (tx, rx) = spmc::bounded::<u64>(cfg.capacity);

  let start = Instant::now();

  let mut consumers = Vec::new();
  for _ in 0..cfg.consumers {
    let rx = rx.clone();
    let received = received.clone();
    consumers.push(thread::spawn(move || {
      while rx.recv().is_ok() {
        received.fetch_add(1, Ordering::Relaxed);
      }
    }));
  }
  drop(rx);

  let items = cfg.items;
  let sent2 = sent.clone();
  let producer = thread::spawn(move || {
    for i in 0..items as u64 {
      let _ = tx.send(i);
      sent2.fetch_add(1, Ordering::Relaxed);
    }
  });

  producer.join().unwrap();
  for c in consumers {
    c.join().unwrap();
  }

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
  }
}

pub fn run_async(cfg: &BenchConfig) -> RunResult {
  use tokio::task::JoinHandle;

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
      expected: cfg.items * cfg.consumers,
    },
  );

  let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .unwrap();

  let start = Instant::now();

  rt.block_on(async {
    let (tx, rx) = spmc::bounded_async::<u64>(cfg.capacity);

    let mut consumer_handles: Vec<JoinHandle<()>> = Vec::new();
    for _ in 0..cfg.consumers {
      let rx = rx.clone();
      let received = received.clone();
      consumer_handles.push(tokio::spawn(async move {
        while rx.recv().await.is_ok() {
          received.fetch_add(1, Ordering::Relaxed);
        }
      }));
    }
    drop(rx);

    let items = cfg.items;
    let sent = sent.clone();
    let producer = tokio::spawn(async move {
      for i in 0..items as u64 {
        let _ = tx.send(i).await;
        sent.fetch_add(1, Ordering::Relaxed);
      }
    });

    producer.await.unwrap();
    for c in consumer_handles {
      c.await.unwrap();
    }
  });

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
  }
}
