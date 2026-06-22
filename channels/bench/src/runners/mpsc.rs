use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use fibre::mpsc;

use crate::watchdog::{self, WatchdogConfig};
use crate::Flavor;
use super::{BenchConfig, RunResult};

// mpsc: Sender is Clone (multiple producers), Receiver is NOT Clone (single consumer).

pub fn run_sync(cfg: &BenchConfig, flavor: &Flavor) -> RunResult {
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

  let start = Instant::now();
  let items_per_prod = cfg.items / cfg.producers;

  match flavor {
    Flavor::Bounded => {
      let (tx, rx) = mpsc::bounded::<usize>(cfg.capacity);

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

      let received2 = received.clone();
      let consumer = thread::spawn(move || {
        while rx.recv().is_ok() {
          received2.fetch_add(1, Ordering::Relaxed);
        }
      });

      for p in producers {
        p.join().unwrap();
      }
      consumer.join().unwrap();
    }
    Flavor::Unbounded => {
      let (tx, rx) = mpsc::unbounded::<usize>();

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

      let received2 = received.clone();
      let consumer = thread::spawn(move || {
        while rx.recv().is_ok() {
          received2.fetch_add(1, Ordering::Relaxed);
        }
      });

      for p in producers {
        p.join().unwrap();
      }
      consumer.join().unwrap();
    }
    Flavor::Rendezvous => {
      eprintln!("error: mpsc does not support rendezvous flavor");
      std::process::exit(1);
    }
  }

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
  }
}

pub fn run_async(cfg: &BenchConfig, flavor: &Flavor) -> RunResult {
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

  match flavor {
    Flavor::Bounded => {
      rt.block_on(async {
        let (tx, rx) = mpsc::bounded_async::<usize>(cfg.capacity);
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

        let received2 = received.clone();
        let consumer: JoinHandle<()> = tokio::spawn(async move {
          while rx.recv().await.is_ok() {
            received2.fetch_add(1, Ordering::Relaxed);
          }
        });

        for p in producer_handles {
          p.await.unwrap();
        }
        consumer.await.unwrap();
      });
    }
    Flavor::Unbounded => {
      rt.block_on(async {
        let (tx, rx) = mpsc::unbounded_async::<usize>();
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

        let received2 = received.clone();
        let consumer: JoinHandle<()> = tokio::spawn(async move {
          while rx.recv().await.is_ok() {
            received2.fetch_add(1, Ordering::Relaxed);
          }
        });

        for p in producer_handles {
          p.await.unwrap();
        }
        consumer.await.unwrap();
      });
    }
    Flavor::Rendezvous => {
      eprintln!("error: mpsc does not support rendezvous flavor");
      std::process::exit(1);
    }
  }

  done.store(true, Ordering::Relaxed);
  let _ = wd.join();

  RunResult {
    sent: sent.load(Ordering::Relaxed),
    received: received.load(Ordering::Relaxed),
    duration: start.elapsed(),
  }
}
