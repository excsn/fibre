use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
  /// The peer broke the protocol.
  Protocol(String),
  /// The peer reset one exchange with this error code.
  Reset(u32),
  /// The connection closed before the exchange finished.
  Closed,
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Error::Protocol(reason) => write!(f, "protocol error: {reason}"),
      Error::Reset(code) => write!(f, "exchange reset by peer with code {code}"),
      Error::Closed => f.write_str("connection closed"),
    }
  }
}

impl std::error::Error for Error {}
