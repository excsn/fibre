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
  /// The peer never processed the exchange, so it is safe to send again on another connection.
  Refused,
  /// A deadline passed.
  Timeout,
  Io(String),
  Tls(String),
  Url(String),
  /// The response body could not be decoded as asked.
  Decode(String),
  /// More redirects than the client follows.
  TooManyRedirects,
  /// The response body is larger than the client accepts.
  TooLarge,
  /// A 4xx or 5xx status, from `Response::error_for_status`.
  Status(u16),
  /// The proxy asked for credentials (407).
  ProxyAuth,
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Error::Protocol(reason) => write!(f, "protocol error: {reason}"),
      Error::Reset(code) => write!(f, "exchange reset by peer with code {code}"),
      Error::Closed => f.write_str("connection closed"),
      Error::Refused => f.write_str("exchange refused by peer before processing"),
      Error::Timeout => f.write_str("timed out"),
      Error::Io(e) => write!(f, "i/o error: {e}"),
      Error::Tls(e) => write!(f, "tls error: {e}"),
      Error::Url(e) => write!(f, "invalid url: {e}"),
      Error::Decode(e) => write!(f, "could not decode body: {e}"),
      Error::TooManyRedirects => f.write_str("too many redirects"),
      Error::TooLarge => f.write_str("response body too large"),
      Error::Status(code) => write!(f, "status {code}"),
      Error::ProxyAuth => f.write_str("the proxy requires authentication"),
    }
  }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
  fn from(e: std::io::Error) -> Self {
    if e.kind() == std::io::ErrorKind::TimedOut {
      return Error::Timeout;
    }
    Error::Io(e.to_string())
  }
}
