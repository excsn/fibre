// src/error.rs

use core::fmt;

// Helper macro to avoid boilerplate for into_inner and Display/Error impls
macro_rules! impl_error_with_inner {
    ($name:ident, $inner_ty_param:ident, $inner_concrete_ty:ty, $($variant:ident($message:expr)),+ $(,)?) => {
        impl<$inner_ty_param> $name<$inner_ty_param> {
            /// Consumes the error, returning the inner value.
            #[inline]
            pub fn into_inner(self) -> $inner_ty_param { // Use the generic type parameter here
                match self {
                    $( $name::$variant(v) => v, )+
                }
            }
        }

        impl<$inner_ty_param> fmt::Display for $name<$inner_ty_param> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $( $name::$variant(_) => f.write_str($message), )+
                }
            }
        }

        // Implement Error for the concrete type (e.g. TrySendError<T>)
        // if T: Debug. If T is not part of the error message or source,
        // we might not need T: Debug for the Error trait itself.
        // Let's make it conditional on $inner_ty_param: fmt::Debug for now.
        impl<$inner_ty_param: fmt::Debug> std::error::Error for $name<$inner_ty_param> {}
    };
}

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
  TrySendError<T>, // This captures the enum name and its generic parameter
  Full("channel full"),
  Closed("channel closed"),
  Sent("channel already sent a value")
);

// ... (rest of the error types remain the same) ...
// The other error types (SendError, TryRecvError, RecvError, CloseError) do not take a generic <T>
// that holds the value, so they don't need this specific macro. They can have their
// Display and Error traits implemented directly.

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
      TryRecvError::Disconnected => write!(f, "channel disconnected (empty and all senders dropped)"),
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
