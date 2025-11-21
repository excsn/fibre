use crate::async_util::AtomicWaker;
use crate::error::{RecvError, TryRecvError};
use crate::mpsc::block_queue::UnboundedBlockQueue;
use crate::{sync_util, TrySendError};

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread::{self, Thread};

/// The shared state of the MPSC channel.
pub(crate) struct MpscShared<T> {
  pub(crate) queue: UnboundedBlockQueue<T>,

  // --- Receiver waiting state ---
  pub(crate) consumer_parked: AtomicBool,
  pub(crate) consumer_thread: Mutex<Option<Thread>>,
  pub(crate) consumer_waker: AtomicWaker,
  pub(crate) receiver_dropped: AtomicBool,

  pub(crate) sender_count: AtomicUsize,
  /// Approximate length tracking.
  /// Note: With BlockQueue, exact length is expensive (iterating blocks).
  /// We maintain this atomic counter for `len()` API support,
  /// though it may slightly lag the actual queue state under high contention.
  pub(crate) current_len: AtomicUsize,
}

impl<T> fmt::Debug for MpscShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("MpscShared")
      .field(
        "consumer_parked",
        &self.consumer_parked.load(Ordering::Relaxed),
      )
      .field("consumer_waker", &self.consumer_waker)
      .field("sender_count", &self.sender_count.load(Ordering::Relaxed))
      .field("current_len", &self.current_len.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

// Safe to send MpscShared across threads if T is Send.
unsafe impl<T: Send> Send for MpscShared<T> {}
unsafe impl<T: Send> Sync for MpscShared<T> {}

impl<T> MpscShared<T> {
  pub(crate) fn new() -> Self {
    MpscShared {
      queue: UnboundedBlockQueue::new(),
      consumer_parked: AtomicBool::new(false),
      consumer_thread: Mutex::new(None),
      consumer_waker: AtomicWaker::new(),
      receiver_dropped: AtomicBool::new(false),
      sender_count: AtomicUsize::new(1),
      current_len: AtomicUsize::new(0),
    }
  }

  /// Wakes the consumer if it is parked.
  #[inline]
  pub(crate) fn wake_consumer(&self) {
    self.consumer_waker.wake();
    if self
      .consumer_parked
      .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
      .is_ok()
    {
      if let Some(thread_handle) = self.consumer_thread.lock().unwrap().take() {
        sync_util::unpark_thread(&thread_handle);
      }
    }
  }

  /// The core non-blocking receive logic.
  pub(crate) fn try_recv_internal(&self) -> Result<T, TryRecvError> {
    match self.queue.pop() {
      Some(val) => {
        self.current_len.fetch_sub(1, Ordering::Relaxed);
        Ok(val)
      }
      None => {
        if self.sender_count.load(Ordering::Acquire) == 0 {
          Err(TryRecvError::Disconnected)
        } else {
          Err(TryRecvError::Empty)
        }
      }
    }
  }

  pub(crate) fn poll_recv_internal(&self, cx: &mut Context<'_>) -> Poll<Result<T, RecvError>> {
    loop {
      match self.try_recv_internal() {
        Ok(value) => return Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          self.consumer_waker.register(cx.waker());
          match self.try_recv_internal() {
            Ok(value) => return Poll::Ready(Ok(value)),
            Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
            Err(TryRecvError::Empty) => {
              if self.sender_count.load(Ordering::Acquire) == 0 {
                match self.try_recv_internal() {
                  Ok(value) => return Poll::Ready(Ok(value)),
                  _ => return Poll::Ready(Err(RecvError::Disconnected)),
                }
              }
              return Poll::Pending;
            }
          }
        }
      }
    }
  }
}
