//! Single-sender oneshot channel.
//!
//! Unlike the clonable [`oneshot()`](super::oneshot) channel, the sender here
//! cannot be cloned and the receiver's methods take `&mut self`, so the whole
//! transfer needs no claim protocol: the type system provides exclusivity on
//! both sides. A successful send is one plain write plus one `fetch_or`.
//!
//! State is a single word of flag bits. The value slot is initialized iff
//! SENT is set; the slot is written before SENT is published (Release) and
//! only read after observing SENT (Acquire). The send-vs-receiver-drop race
//! is resolved by the total order of the two `fetch_or`s on the word: whoever
//! is second sees the first's bit and takes responsibility for the value.
//!
//! # Examples
//!
//! ```
//! use fibre::oneshot;
//!
//! let (tx, mut rx) = oneshot::exclusive::<u32>();
//!
//! tokio::runtime::Runtime::new().unwrap().block_on(async {
//!     tx.send(42).unwrap();
//!     assert_eq!(rx.recv().await.unwrap(), 42);
//! });
//! ```

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, TryRecvError, TrySendError};
use crate::internal::sync::{AtomicUsize, Ordering};

use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

const SENT: usize = 1 << 0;
const RX_CLOSED: usize = 1 << 1;
const TX_CLOSED: usize = 1 << 2;

struct ExclusiveShared<T> {
  state: AtomicUsize,
  value_slot: UnsafeCell<MaybeUninit<T>>,
  receiver_waker: AtomicWaker,
}

unsafe impl<T: Send> Send for ExclusiveShared<T> {}
unsafe impl<T: Send> Sync for ExclusiveShared<T> {}

impl<T> ExclusiveShared<T> {
  unsafe fn take_value(&self) -> T {
    unsafe { (*self.value_slot.get()).assume_init_read() }
  }
}

/// Creates a single-sender oneshot channel.
///
/// The [`ExclusiveSender`] cannot be cloned and is consumed by `send`. The
/// [`ExclusiveReceiver`]'s receive methods take `&mut self`. This makes the
/// transfer considerably cheaper than the clonable [`oneshot()`](super::oneshot).
pub fn exclusive<T>() -> (ExclusiveSender<T>, ExclusiveReceiver<T>) {
  let shared = Arc::new(ExclusiveShared {
    state: AtomicUsize::new(0),
    value_slot: UnsafeCell::new(MaybeUninit::uninit()),
    receiver_waker: AtomicWaker::new(),
  });
  (
    ExclusiveSender {
      shared: ManuallyDrop::new(Arc::clone(&shared)),
    },
    ExclusiveReceiver {
      shared,
      done: false,
    },
  )
}

/// The sending side of an [`exclusive()`] oneshot channel. Not clonable.
pub struct ExclusiveSender<T> {
  shared: ManuallyDrop<Arc<ExclusiveShared<T>>>,
}

impl<T> ExclusiveSender<T> {
  /// Sends a value, consuming the sender.
  ///
  /// Fails with [`TrySendError::Closed`] returning the value if the receiver
  /// was dropped or explicitly closed.
  pub fn send(mut self, value: T) -> Result<(), TrySendError<T>> {
    let shared = unsafe { ManuallyDrop::take(&mut self.shared) };
    std::mem::forget(self);

    unsafe {
      (*shared.value_slot.get()).write(value);
    }
    let prev = shared.state.fetch_or(SENT, Ordering::AcqRel);
    if prev & RX_CLOSED != 0 {
      // The receiver's fetch_or ordered first and saw no SENT, so it did not
      // touch the slot; the value is ours to take back.
      return Err(TrySendError::Closed(unsafe { shared.take_value() }));
    }
    shared.receiver_waker.wake();
    Ok(())
  }

  /// Closes this sender without sending. Equivalent to dropping it.
  pub fn close(self) {}

  /// Checks if the channel's `ExclusiveReceiver` has been dropped or closed.
  pub fn is_closed(&self) -> bool {
    self.shared.state.load(Ordering::Acquire) & RX_CLOSED != 0
  }
}

impl<T> Drop for ExclusiveSender<T> {
  fn drop(&mut self) {
    self.shared.state.fetch_or(TX_CLOSED, Ordering::AcqRel);
    self.shared.receiver_waker.wake();
    unsafe { ManuallyDrop::drop(&mut self.shared) };
  }
}

impl<T> fmt::Debug for ExclusiveSender<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ExclusiveSender")
      .field("is_closed", &self.is_closed())
      .finish_non_exhaustive()
  }
}

/// The receiving side of an [`exclusive()`] oneshot channel. Not clonable.
pub struct ExclusiveReceiver<T> {
  shared: Arc<ExclusiveShared<T>>,
  done: bool,
}

unsafe impl<T: Send> Send for ExclusiveReceiver<T> {}

impl<T> ExclusiveReceiver<T> {
  /// Attempts to receive the value non-blockingly.
  ///
  /// Returns `Err(TryRecvError::Empty)` while the sender is alive and has not
  /// sent, and `Err(TryRecvError::Disconnected)` once the value has been taken,
  /// the sender dropped without sending, or this handle was closed.
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    if self.done {
      return Err(TryRecvError::Disconnected);
    }
    let state = self.shared.state.load(Ordering::Acquire);
    if state & SENT != 0 {
      self.done = true;
      return Ok(unsafe { self.shared.take_value() });
    }
    if state & TX_CLOSED != 0 {
      self.done = true;
      return Err(TryRecvError::Disconnected);
    }
    Err(TryRecvError::Empty)
  }

  /// Waits asynchronously for the value.
  pub fn recv(&mut self) -> ExclusiveReceiveFuture<'_, T> {
    ExclusiveReceiveFuture { receiver: self }
  }

  /// Closes the receiving end. After this, `send` fails and returns the value.
  ///
  /// This is an explicit alternative to `drop`. If a value was already sent
  /// but not yet received, it is dropped.
  pub fn close(&mut self) {
    if self.done {
      return;
    }
    self.done = true;
    self.close_internal();
  }

  fn close_internal(&self) {
    let prev = self.shared.state.fetch_or(RX_CLOSED, Ordering::AcqRel);
    if prev & SENT != 0 {
      unsafe {
        (*self.shared.value_slot.get()).assume_init_drop();
      }
    }
  }

  /// Checks if no value can ever be received anymore: the value was taken,
  /// this handle was closed, or the sender dropped without sending.
  pub fn is_closed(&self) -> bool {
    if self.done {
      return true;
    }
    let state = self.shared.state.load(Ordering::Acquire);
    state & SENT == 0 && state & TX_CLOSED != 0
  }
}

impl<T> Drop for ExclusiveReceiver<T> {
  fn drop(&mut self) {
    if !self.done {
      self.close_internal();
    }
  }
}

impl<T> fmt::Debug for ExclusiveReceiver<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ExclusiveReceiver")
      .field("done", &self.done)
      .finish_non_exhaustive()
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ExclusiveReceiveFuture<'a, T> {
  receiver: &'a mut ExclusiveReceiver<T>,
}

impl<'a, T> Future for ExclusiveReceiveFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let rx = &mut *self.receiver;
    match rx.try_recv() {
      Ok(value) => return Poll::Ready(Ok(value)),
      Err(TryRecvError::Disconnected) => return Poll::Ready(Err(RecvError::Disconnected)),
      Err(TryRecvError::Empty) => {}
    }
    rx.shared.receiver_waker.register(cx.waker());
    match rx.try_recv() {
      Ok(value) => Poll::Ready(Ok(value)),
      Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
      Err(TryRecvError::Empty) => Poll::Pending,
    }
  }
}
