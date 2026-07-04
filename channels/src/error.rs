use core::fmt;

/// Error returned by `try_send` operations on a channel when the operation
/// could not be completed immediately, but the item being sent is returned.
#[derive(PartialEq, Eq, Clone)]
pub enum TrySendError<T> {
  // T is the generic type parameter for the enum
  /// The channel is full and cannot accept more items at this time.
  /// The item being sent is returned.
  Full(T),
  /// The channel is closed because all receivers have been dropped (or the
  /// oneshot receiver was dropped before send).
  /// The item being sent is returned.
  Closed(T),
  /// (For Oneshot channels) A value has already been successfully sent on this channel.
  /// The item being sent is returned.
  Sent(T),
}

impl<T> fmt::Debug for TrySendError<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      TrySendError::Full(_) => write!(f, "TrySendError::Full(..)"),
      TrySendError::Closed(_) => write!(f, "TrySendError::Closed(..)"),
      TrySendError::Sent(_) => write!(f, "TrySendError::Sent(..)"),
    }
  }
}

// Invocation of the macro:
// $name = TrySendError
// $inner_ty_param = T (this is the generic placeholder <T> in TrySendError<T>)
// $inner_concrete_ty = T (this is what the methods will often use, but for the impl line, it's $name<$inner_ty_param>)
// The macro was defined with `$inner_type:ty`. Let's stick to that.
// The macro arguments should be:
// 1. Name of the enum (ident)
// 2. The generic type parameter *identifier* used in the enum definition (e.g., `T`)
// 3. The variants and their messages.

// Redefine macro slightly for clarity
macro_rules! impl_error_for_enum_with_inner {
  (
    $enum_name:ident < $generic_param:ident >, // e.g., TrySendError<T>
    $($variant:ident ( $message:expr ) ),+ // e.g., Full("message")
    $(,)?
  ) => {
    // Impl methods on MyError<T>
    impl<$generic_param> $enum_name<$generic_param> {
      #[inline]
      pub fn into_inner(self) -> $generic_param {
        match self {
          $( $enum_name::$variant(v) => v, )+
        }
      }
    }

    // Impl Display for MyError<T>
    impl<$generic_param> fmt::Display for $enum_name<$generic_param> {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
          $( $enum_name::$variant(_) => f.write_str($message), )+
        }
      }
    }

    // Impl std::error::Error for MyError<T> where T: Debug
    impl<$generic_param: fmt::Debug> std::error::Error for $enum_name<$generic_param> {}
  };
}

impl_error_for_enum_with_inner!(
  TrySendError<T>,
  Full("channel full"),
  Closed("channel closed"),
  Sent("channel already sent a value")
);

/// Error returned by `send` operations that can block or are part of async operations.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SendError {
  Closed,
  Sent,
}
impl std::error::Error for SendError {}
impl fmt::Display for SendError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      SendError::Closed => write!(f, "channel closed"),
      SendError::Sent => write!(f, "channel already sent a value"),
    }
  }
}

/// Error returned by `try_recv` operations on a channel when an item
/// could not be received immediately.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TryRecvError {
  Empty,
  Disconnected,
}
impl std::error::Error for TryRecvError {}
impl fmt::Display for TryRecvError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      TryRecvError::Empty => write!(f, "channel empty"),
      TryRecvError::Disconnected => {
        write!(f, "channel disconnected (empty and all senders dropped)")
      }
    }
  }
}

/// Error returned by `recv` operations that can block or are part of async operations.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RecvError {
  Disconnected,
}
impl std::error::Error for RecvError {}
impl fmt::Display for RecvError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      // This was missing
      RecvError::Disconnected => write!(f, "channel disconnected (empty and all senders dropped)"),
    }
  }
}

/// Error returned when attempting to close an already closed channel.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct CloseError;
impl std::error::Error for CloseError {}
impl fmt::Display for CloseError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "channel is already closed")
  }
}

/// The reason a batch send operation stopped before sending every item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchSendErrorReason {
  /// The channel was full (or, for a rendezvous channel, no receiver was ready).
  Full,
  /// The channel is closed because all receivers have been dropped.
  Closed,
}

/// Error returned by `try_send_batch` operations.
///
/// Carries partial-progress state: `sent` items were delivered to the channel
/// before the operation stopped, and `unsent` holds the untouched remainder
/// so no owned value is silently dropped.
#[derive(PartialEq, Eq, Clone)]
pub struct TrySendBatchError<T> {
  /// The number of items successfully sent before the operation stopped.
  pub sent: usize,
  /// The items that were not sent, in their original order.
  pub unsent: Vec<T>,
  /// Why the batch stopped early.
  pub reason: BatchSendErrorReason,
}

impl<T> TrySendBatchError<T> {
  /// Consumes the error, returning the unsent items.
  #[inline]
  pub fn into_unsent(self) -> Vec<T> {
    self.unsent
  }
}

impl<T> fmt::Debug for TrySendBatchError<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("TrySendBatchError")
      .field("sent", &self.sent)
      .field("unsent", &self.unsent.len())
      .field("reason", &self.reason)
      .finish()
  }
}

impl<T> fmt::Display for TrySendBatchError<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let total = self.sent + self.unsent.len();
    match self.reason {
      BatchSendErrorReason::Full => {
        write!(
          f,
          "channel full after sending {} of {} items",
          self.sent, total
        )
      }
      BatchSendErrorReason::Closed => {
        write!(
          f,
          "channel closed after sending {} of {} items",
          self.sent, total
        )
      }
    }
  }
}

impl<T> std::error::Error for TrySendBatchError<T> {}

/// Error returned by blocking and async `send_batch` operations.
///
/// The only failure cause is channel closure. Carries partial-progress state:
/// `sent` items were delivered before closure was observed, and `unsent`
/// holds the remainder.
#[derive(PartialEq, Eq, Clone)]
pub struct SendBatchError<T> {
  /// The number of items successfully sent before the channel closed.
  pub sent: usize,
  /// The items that were not sent, in their original order.
  pub unsent: Vec<T>,
}

impl<T> SendBatchError<T> {
  /// Consumes the error, returning the unsent items.
  #[inline]
  pub fn into_unsent(self) -> Vec<T> {
    self.unsent
  }
}

impl<T> fmt::Debug for SendBatchError<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SendBatchError")
      .field("sent", &self.sent)
      .field("unsent", &self.unsent.len())
      .finish()
  }
}

impl<T> fmt::Display for SendBatchError<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let total = self.sent + self.unsent.len();
    write!(
      f,
      "channel closed after sending {} of {} items",
      self.sent, total
    )
  }
}

impl<T> std::error::Error for SendBatchError<T> {}

/// Error returned by `recv_timeout` operations.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RecvErrorTimeout {
  /// The channel is empty and closed because all senders have been dropped.
  Disconnected,
  /// The timeout elapsed before an item could be received.
  Timeout,
}

impl std::error::Error for RecvErrorTimeout {}
impl fmt::Display for RecvErrorTimeout {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      RecvErrorTimeout::Disconnected => write!(f, "channel disconnected"),
      RecvErrorTimeout::Timeout => write!(f, "receive operation timed out"),
    }
  }
}
