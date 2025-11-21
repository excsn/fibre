use super::shared::MpscShared;
use crate::error::{CloseError, RecvError, TryRecvError};
use crate::{sync_util, RecvErrorTimeout};

use futures_util::stream::Stream;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Receiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send> Send for Receiver<T> {}

#[derive(Debug)]
pub struct AsyncReceiver<T: Send> {
  pub(crate) shared: Arc<MpscShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send> Send for AsyncReceiver<T> {}

impl<T: Send> Receiver<T> {
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      *self.shared.consumer_thread.lock().unwrap() = Some(thread::current());
      self.shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv() {
        Ok(value) => {
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvError::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          sync_util::park_thread();
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let start_time = Instant::now();
    loop {
      match self.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => {}
      }

      let elapsed = start_time.elapsed();
      if elapsed >= timeout {
        return Err(RecvErrorTimeout::Timeout);
      }
      let remaining_timeout = timeout - elapsed;

      *self.shared.consumer_thread.lock().unwrap() = Some(thread::current());
      self.shared.consumer_parked.store(true, Ordering::Release);

      match self.try_recv() {
        Ok(value) => {
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Ok(value);
        }
        Err(TryRecvError::Disconnected) => {
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
          return Err(RecvErrorTimeout::Disconnected);
        }
        Err(TryRecvError::Empty) => {
          sync_util::park_thread_timeout(remaining_timeout);
          if self
            .shared
            .consumer_parked
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
          {
            *self.shared.consumer_thread.lock().unwrap() = None;
          }
        }
      }
    }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.sender_count.load(Ordering::Acquire) == 0 && self.shared.queue.is_empty()
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    self.shared.receiver_dropped.store(true, Ordering::Release);
    // Drain queue to drop items
    while self.shared.try_recv_internal().is_ok() {}
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count.load(Ordering::Relaxed)
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    self.shared.queue.is_empty()
  }

  pub fn to_async(self) -> AsyncReceiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);
    AsyncReceiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

impl<T: Send> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    self.shared.try_recv_internal()
  }
  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture { consumer: self }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.sender_count.load(Ordering::Acquire) == 0 && self.shared.queue.is_empty()
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    self.shared.receiver_dropped.store(true, Ordering::Release);
    while self.shared.try_recv_internal().is_ok() {}
  }

  pub fn sender_count(&self) -> usize {
    self.shared.sender_count.load(Ordering::Relaxed)
  }

  pub fn len(&self) -> usize {
    self.shared.current_len.load(Ordering::Relaxed)
  }

  pub fn is_empty(&self) -> bool {
    self.shared.queue.is_empty()
  }

  pub fn to_sync(self) -> Receiver<T> {
    let shared = unsafe { std::ptr::read(&self.shared) };
    std::mem::forget(self);
    Receiver {
      shared,
      closed: AtomicBool::new(false),
    }
  }
}

// --- Futures ---

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send> {
  consumer: &'a AsyncReceiver<T>,
}

impl<'a, T: Send> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.consumer.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    self.consumer.shared.poll_recv_internal(cx)
  }
}

impl<T: Send> Stream for AsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    if self.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    match self.shared.poll_recv_internal(cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

impl<T: Send> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
impl<T: Send> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
