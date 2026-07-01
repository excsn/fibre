use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use fibre::mpmc_v2 as mpmc;

use crate::watchdog::{self, WatchdogConfig};
use crate::Flavor;
use super::{BenchConfig, RunResult};

pub fn run_sync(cfg: &BenchConfig, flavor: &Flavor) -> RunResult {
  if matches!(flavor, Flavor::Rendezvous) {
    return run_sync_rendezvous(cfg);
  }

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

  let (tx, rx) = match flavor {
    Flavor::Bounded => mpmc::bounded::<usize>(cfg.capacity),
    Flavor::Unbounded => mpmc::unbounded::<usize>(),
    Flavor::Rendezvous => unreachable!(),
  };

  let start = Instant::now();
  let items_per_prod = cfg.items / cfg.producers;
  let batch_size = cfg.batch_size;

  let mut producers = Vec::new();
  for _ in 0..cfg.producers {
    let tx = tx.clone();
    let sent = sent.clone();
    producers.push(thread::spawn(move || {
      if batch_size == 0 {
        for i in 0..items_per_prod {
          let _ = tx.send(i);
          sent.fetch_add(1, Ordering::Relaxed);
        }
      } else {
        let mut remaining = items_per_prod;
        while remaining > 0 {
          let chunk_size = remaining.min(batch_size);
          match tx.send_batch(vec![0usize; chunk_size]) {
            Ok(n) => {
              sent.fetch_add(n, Ordering::Relaxed);
              remaining -= n;
            }
            Err(_) => break,
          }
        }
      }
    }));
  }
  drop(tx);

  let mut consumers = Vec::new();
  for _ in 0..cfg.consumers {
    let rx = rx.clone();
    let received = received.clone();
    consumers.push(thread::spawn(move || {
      if batch_size == 0 {
        while rx.recv().is_ok() {
          received.fetch_add(1, Ordering::Relaxed);
        }
      } else {
        while let Ok(batch) = rx.recv_batch(batch_size) {
          received.fetch_add(batch.len(), Ordering::Relaxed);
        }
      }
    }));
  }
  drop(rx);

  for p in producers {
    p.join().unwrap();
  }
  for c in consumers {
    c.join().unwrap();
  }

  done.store(true, Ordering::Relaxed);
  let watchdog_ticks = wd.join().unwrap_or_default();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
    watchdog_ticks,
  }
}

fn run_sync_rendezvous(cfg: &BenchConfig) -> RunResult {
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

  let (tx, rx) = mpmc::rendezvous::rendezvous::<usize>();

  let start = Instant::now();
  let items_per_prod = cfg.items / cfg.producers;

  let mut producers = Vec::new();
  for _ in 0..cfg.producers {
    let tx = tx.clone();
    let sent = sent.clone();
    producers.push(thread::spawn(move || {
      for i in 0..items_per_prod {
        let _ = tx.send(i);
        sent.fetch_add(1, Ordering::Relaxed);
      }
    }));
  }
  drop(tx);

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

  for p in producers {
    p.join().unwrap();
  }
  for c in consumers {
    c.join().unwrap();
  }

  done.store(true, Ordering::Relaxed);
  let watchdog_ticks = wd.join().unwrap_or_default();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
    watchdog_ticks,
  }
}

pub fn run_async(cfg: &BenchConfig, flavor: &Flavor) -> RunResult {
  if matches!(flavor, Flavor::Rendezvous) {
    return run_async_rendezvous(cfg);
  }

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
      expected: cfg.items,
    },
  );

  let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .unwrap();

  let start = Instant::now();

  rt.block_on(async {
    let (tx, rx) = match flavor {
      Flavor::Bounded => mpmc::bounded_async::<usize>(cfg.capacity),
      Flavor::Unbounded => mpmc::unbounded_async::<usize>(),
      Flavor::Rendezvous => unreachable!(),
    };

    let items_per_prod = cfg.items / cfg.producers;
    let batch_size = cfg.batch_size;
    let mut producer_handles: Vec<JoinHandle<()>> = Vec::new();
    for _ in 0..cfg.producers {
      let tx = tx.clone();
      let sent = sent.clone();
      producer_handles.push(tokio::spawn(async move {
        if batch_size == 0 {
          for i in 0..items_per_prod {
            let _ = tx.send(i).await;
            sent.fetch_add(1, Ordering::Relaxed);
          }
        } else {
          let mut remaining = items_per_prod;
          while remaining > 0 {
            let chunk_size = remaining.min(batch_size);
            match tx.send_batch(vec![0usize; chunk_size]).await {
              Ok(n) => {
                sent.fetch_add(n, Ordering::Relaxed);
                remaining -= n;
              }
              Err(_) => break,
            }
          }
        }
      }));
    }
    drop(tx);

    let mut consumer_handles: Vec<JoinHandle<()>> = Vec::new();
    for _ in 0..cfg.consumers {
      let rx = rx.clone();
      let received = received.clone();
      consumer_handles.push(tokio::spawn(async move {
        if batch_size == 0 {
          while rx.recv().await.is_ok() {
            received.fetch_add(1, Ordering::Relaxed);
          }
        } else {
          while let Ok(batch) = rx.recv_batch(batch_size).await {
            received.fetch_add(batch.len(), Ordering::Relaxed);
          }
        }
      }));
    }
    drop(rx);

    for p in producer_handles {
      p.await.unwrap();
    }
    for c in consumer_handles {
      c.await.unwrap();
    }
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

fn run_async_rendezvous(cfg: &BenchConfig) -> RunResult {
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
      expected: cfg.items,
    },
  );

  let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .unwrap();

  let start = Instant::now();

  rt.block_on(async {
    let (tx, rx) = mpmc::rendezvous::rendezvous_async::<usize>();

    let items_per_prod = cfg.items / cfg.producers;
    let mut producer_handles: Vec<JoinHandle<()>> = Vec::new();
    for _ in 0..cfg.producers {
      let tx = tx.clone();
      let sent = sent.clone();
      producer_handles.push(tokio::spawn(async move {
        for i in 0..items_per_prod {
          let _ = tx.send(i).await;
          sent.fetch_add(1, Ordering::Relaxed);
        }
      }));
    }
    drop(tx);

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

    for p in producer_handles {
      p.await.unwrap();
    }
    for c in consumer_handles {
      c.await.unwrap();
    }
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
