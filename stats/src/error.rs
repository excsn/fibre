use std::fmt;
use std::sync::Arc;

use crate::recorder::Recorder;

/// `set_global` was called when a global recorder was already set. Holds the
/// recorder that was not installed.
pub struct SetGlobalError(pub Arc<dyn Recorder>);

impl fmt::Debug for SetGlobalError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("SetGlobalError")
  }
}

impl fmt::Display for SetGlobalError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("a global recorder is already set")
  }
}

impl std::error::Error for SetGlobalError {}

/// A collector could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectError {
  /// The platform or the library behind the collector could not provide what it reads.
  Unavailable(String),
}

impl fmt::Display for CollectError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      CollectError::Unavailable(why) => write!(f, "collector unavailable: {why}"),
    }
  }
}

impl std::error::Error for CollectError {}
