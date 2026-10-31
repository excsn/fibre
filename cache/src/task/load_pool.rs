use fibre::mpmc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// At most this many loaders run in the background per cache; more wait in the queue.
const MAX_WORKERS: usize = 64;
const IDLE_EXIT: Duration = Duration::from_secs(1);

pub(crate) type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Counts {
  workers: AtomicUsize,
  idle: AtomicUsize,
  queued: AtomicUsize,
}

/// Runs sync loaders off the caller's thread, for stale refreshes and for async
/// handles. Workers start on demand and exit after `IDLE_EXIT` without work or
/// when the cache is dropped.
pub(crate) struct LoadPool {
  tx: mpmc::UnboundedSyncSender<Job>,
  rx: mpmc::UnboundedSyncReceiver<Job>,
  counts: Arc<Counts>,
}

impl LoadPool {
  pub(crate) fn new() -> Self {
    let (tx, rx) = mpmc::unbounded();
    Self { tx, rx, counts: Arc::default() }
  }

  /// Never blocks, so an async path may call it.
  pub(crate) fn submit(&self, job: Job) {
    let queued = self.counts.queued.fetch_add(1, Ordering::SeqCst) + 1;
    if self.tx.clone().send(job).is_err() {
      self.counts.queued.fetch_sub(1, Ordering::SeqCst);
      return;
    }
    if queued <= self.counts.idle.load(Ordering::SeqCst) {
      return;
    }
    let claimed = self.counts.workers.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |w| (w < MAX_WORKERS).then_some(w + 1));
    if claimed.is_ok() {
      let (rx, counts) = (self.rx.clone(), self.counts.clone());
      thread::spawn(move || worker(rx, counts));
    }
  }
}

fn worker(mut rx: mpmc::UnboundedSyncReceiver<Job>, counts: Arc<Counts>) {
  loop {
    counts.idle.fetch_add(1, Ordering::SeqCst);
    let job = rx.recv_timeout(IDLE_EXIT);
    counts.idle.fetch_sub(1, Ordering::SeqCst);
    match job {
      Ok(job) => {
        counts.queued.fetch_sub(1, Ordering::SeqCst);
        job();
      }
      Err(_) => {
        counts.workers.fetch_sub(1, Ordering::SeqCst);
        // A job sent after the timeout saw no idle worker to take it.
        match rx.try_recv() {
          Ok(job) => {
            counts.workers.fetch_add(1, Ordering::SeqCst);
            counts.queued.fetch_sub(1, Ordering::SeqCst);
            job();
          }
          Err(_) => return,
        }
      }
    }
  }
}
