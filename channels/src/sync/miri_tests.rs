//! Tests that only run under miri (`cargo +nightly miri test -p fibre_cache --lib sync::`).
//!
//! The regular unit tests in mutex.rs / rwlock.rs also run under miri with
//! scaled-down iteration counts; this module holds the scenarios whose value
//! is specifically miri's UB, leak, and weak-memory checking of the raw
//! pointer WaitQueue lifecycle - the assertions themselves are secondary.

use super::mutex::HybridMutex;
use super::rwlock::HybridRwLock;
use crate::sync::test_util::counting_waker;

use std::sync::Arc;
use std::task::Context;
use std::thread;

const THREADS: usize = 3;
const ITERS: usize = 20;

/// Concurrent lock/unlock traffic: exercises the state-word atomics, the
/// intrusive FIFO list under its spinlock, and cross-thread wakes of
/// stack-owned waiter nodes.
#[test]
fn mutex_hammer() {
  let m = Arc::new(HybridMutex::new(0usize));
  let handles: Vec<_> = (0..THREADS)
    .map(|_| {
      let m = m.clone();
      thread::spawn(move || {
        for _ in 0..ITERS {
          *m.lock() += 1;
        }
      })
    })
    .collect();
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(*m.lock(), THREADS * ITERS);
}

/// Mixed reader/writer traffic through both unlock paths (last-reader-out
/// and write unlock both drain the queue).
#[test]
fn rwlock_hammer() {
  let l = Arc::new(HybridRwLock::new((0u64, 0u64)));
  let mut handles = Vec::new();
  for _ in 0..2 {
    let l = l.clone();
    handles.push(thread::spawn(move || {
      for _ in 0..ITERS {
        let mut g = l.write();
        g.0 += 1;
        g.1 += 1;
      }
    }));
  }
  for _ in 0..2 {
    let l = l.clone();
    handles.push(thread::spawn(move || {
      for _ in 0..ITERS {
        let g = l.read();
        assert_eq!(g.0, g.1);
      }
    }));
  }
  for h in handles {
    h.join().unwrap();
  }
  assert_eq!(*l.read(), (2 * ITERS as u64, 2 * ITERS as u64));
}

/// Async waiter-node lifecycle: one node per future, cancellation unlinks,
/// unlock grants to the FIFO head. Miri verifies every Box::into_raw has a
/// matching from_raw and nothing dangles or leaks.
#[test]
fn async_node_lifecycle() {
  let m = HybridMutex::new(());
  let g = m.lock();

  let (c1, w1) = counting_waker();
  let (c2, w2) = counting_waker();
  let mut cx1 = Context::from_waker(&w1);
  let mut cx2 = Context::from_waker(&w2);

  let mut f1 = Box::pin(m.lock_async());
  let mut f2 = Box::pin(m.lock_async());

  // f1 re-polled (single node, waker updated in place), f2 polled once.
  assert!(f1.as_mut().poll(&mut cx1).is_pending());
  assert!(f1.as_mut().poll(&mut cx1).is_pending());
  assert!(f2.as_mut().poll(&mut cx2).is_pending());

  // Cancel f2: its node is unlinked and freed immediately.
  drop(f2);

  drop(g); // releases the lock and wakes f1, the FIFO head.
  assert_eq!(c1.count(), 1);
  assert_eq!(c2.count(), 0, "cancelled waiter must not be woken");

  let guard = match f1.as_mut().poll(&mut cx1) {
    std::task::Poll::Ready(guard) => guard,
    std::task::Poll::Pending => panic!("lock is free; re-contention must succeed"),
  };
  drop(guard);
}

/// Same lifecycle through the rwlock: a writer waiter is woken by a reader
/// unlocking on another thread; the node itself is freed by its owning
/// future back on this thread after it re-contends and wins.
#[test]
fn rwlock_async_node_lifecycle_cross_thread() {
  let l = HybridRwLock::new(());

  let (counter, waker) = counting_waker();
  let mut cx = Context::from_waker(&waker);

  thread::scope(|s| {
    let r = l.read();
    let mut fut = Box::pin(l.write_async());
    assert!(fut.as_mut().poll(&mut cx).is_pending());

    // Move the read guard to another thread and drop it there: the grant
    // (unlink + WOKEN + wake) happens on that thread, against a node
    // allocated on this one.
    s.spawn(move || {
      drop(r);
    })
    .join()
    .unwrap();

    assert_eq!(counter.count(), 1);
    assert!(fut.as_mut().poll(&mut cx).is_ready());
  });
}
