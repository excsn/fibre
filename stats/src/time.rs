//! `std::time` panics on `wasm*-unknown-unknown`. There the `wasm-bindgen`
//! feature reads the browser's clocks through `web-time`. Without it a host
//! supplies a `Clock` and a timer measures with that.

use std::time::Duration;

use crate::recorder::Recorder;

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(all(target_family = "wasm", target_os = "unknown", feature = "wasm-bindgen"))]
use web_time::{Instant, SystemTime, UNIX_EPOCH};

/// Nanoseconds since the Unix epoch. Zero when the clock is before it.
#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
pub(crate) fn unix_now_ns() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
    .unwrap_or(0)
}

#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
pub(crate) struct Stopwatch(Instant);

#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
impl Stopwatch {
  pub(crate) fn start(_recorder: &dyn Recorder) -> Self {
    Stopwatch(Instant::now())
  }

  pub(crate) fn elapsed(&self, _recorder: &dyn Recorder) -> Duration {
    self.0.elapsed()
  }
}

#[cfg(all(target_family = "wasm", target_os = "unknown", not(feature = "wasm-bindgen")))]
pub(crate) struct Stopwatch(u64);

#[cfg(all(target_family = "wasm", target_os = "unknown", not(feature = "wasm-bindgen")))]
impl Stopwatch {
  pub(crate) fn start(recorder: &dyn Recorder) -> Self {
    Stopwatch(recorder.now_ns())
  }

  pub(crate) fn elapsed(&self, recorder: &dyn Recorder) -> Duration {
    Duration::from_nanos(recorder.now_ns().saturating_sub(self.0))
  }
}
