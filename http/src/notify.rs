//! A wake-up that blocking threads and async tasks wait on alike.
//!
//! A waiter reads `epoch`, looks for what it wants and parks only if `epoch` is unchanged. `notify_all` raises `epoch` before it wakes anyone, so a change made between the look and the park is never missed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Poll, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use parking_lot::Mutex;

#[derive(Default)]
pub(crate) enum Waiter {
  #[default]
  None,
  Thread(Thread),
  Task(Waker),
}

impl Waiter {
  pub(crate) fn wake(&mut self) {
    match std::mem::take(self) {
      Waiter::Thread(t) => t.unpark(),
      Waiter::Task(w) => w.wake(),
      Waiter::None => {}
    }
  }

  pub(crate) fn is_some(&self) -> bool {
    !matches!(self, Waiter::None)
  }
}

#[derive(Default)]
pub(crate) struct Notify {
  epoch: AtomicU64,
  waiters: Mutex<Vec<Waiter>>,
}

impl Notify {
  pub(crate) fn epoch(&self) -> u64 {
    self.epoch.load(Ordering::SeqCst)
  }

  pub(crate) fn notify_all(&self) {
    self.epoch.fetch_add(1, Ordering::SeqCst);
    let waiters = std::mem::take(&mut *self.waiters.lock());
    for mut w in waiters {
      w.wake();
    }
  }

  /// Parks the thread until a notify after `seen` or until `limit` passes.
  pub(crate) fn wait_sync(&self, seen: u64, limit: Duration) {
    {
      let mut waiters = self.waiters.lock();
      if self.epoch() != seen {
        return;
      }
      waiters.push(Waiter::Thread(thread::current()));
    }
    thread::park_timeout(limit);
  }

  /// Waits until a notify after `seen` or until `limit` passes.
  pub(crate) async fn wait_async(&self, seen: u64, limit: Duration) {
    let changed = std::future::poll_fn(|cx| {
      let mut waiters = self.waiters.lock();
      if self.epoch() != seen {
        return Poll::Ready(());
      }
      waiters.push(Waiter::Task(cx.waker().clone()));
      Poll::Pending
    });
    let _ = tokio::time::timeout(limit, changed).await;
  }
}
