//! Metrics for applications and libraries.
//!
//! A metric is defined once with [`metric!`]: its name, kind, unit, description
//! and the tags it may carry. The definition is a handle that records. It is
//! also registered automatically: [`registry`] lists it without the program
//! doing anything, provided the linker keeps it. A definition whose handle the
//! program refers to is always kept. See [`registry`] for the other cases.
//!
//! Recordings go to a [`Recorder`]. [`Stats`] is the one that aggregates: one
//! point per series per window, with counters summed, gauges and states
//! keeping their last value and timers kept as a [`Histogram`]. A flush hands
//! the closed windows to a [`Sink`] as a [`Batch`].
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//! use fibre_stats::{metric, tags, Stats, StatsConfig, VecSink};
//!
//! metric! {
//!   /// Requests answered.
//!   #[tags("status")]
//!   pub static REQUESTS: Counter = "http.requests";
//!
//!   /// Time to answer one request.
//!   pub static LATENCY: Timer = "http.latency";
//! }
//!
//! let stats = Stats::new(StatsConfig::default());
//! REQUESTS.add_to(&stats, 1, tags!("status" => "ok"));
//! LATENCY.record_to(&stats, Duration::from_millis(3), tags!());
//!
//! let batch = stats.drain(true);
//! assert_eq!(batch.series.len(), 2);
//! assert!(fibre_stats::registry().any(|m| m.name == "http.requests"));
//! ```
//!
//! With the global recorder installed, the handles need no recorder:
//!
//! ```
//! # use std::sync::Arc;
//! # use fibre_stats::{metric, tags, Stats, StatsConfig};
//! # metric! {
//! #   /// Requests answered.
//! #   #[tags("status")]
//! #   pub static REQUESTS: Counter = "http.requests";
//! # }
//! # fn main() -> Result<(), fibre_stats::SetGlobalError> {
//! # let stats = Stats::new(StatsConfig::default());
//! fibre_stats::set_global(Arc::new(stats.clone()))?;
//! REQUESTS.incr(tags!("status" => "ok"));
//! # Ok(())
//! # }
//! ```

pub mod collect;
pub mod error;
pub mod histogram;
mod macros;
pub mod metric;
pub mod recorder;
pub mod sink;
pub mod stats;
pub mod tag;
mod time;
pub mod value;

pub use collect::Collector;
pub use error::{CollectError, SetGlobalError};
pub use histogram::Histogram;
pub use metric::{Counter, Gauge, Kind, Metric, MetricAttr, State, Timer, TimerGuard, Unit, find, registry};
pub use recorder::{Clock, Context, Noop, Observation, Recorder, RecorderExt, Sample, global, global_arc, set_global};
#[cfg(any(not(all(target_family = "wasm", target_os = "unknown")), feature = "wasm-bindgen"))]
pub use recorder::SystemClock;
#[cfg(feature = "xs_foundation")]
pub use recorder::XsClock;
pub use sink::{Batch, Dropped, Points, SeriesBatch, Sink, VecSink};
pub use stats::{CollectorId, Refusal, RefusalReason, Stats, StatsConfig};
pub use tag::{Name, Tag, Tags};
pub use value::{Encoding, EnumState, IntValue, Normalized, NumberValue, Percent, Ratio32, StatValue, StateValue};

#[doc(hidden)]
pub use inventory as __inventory;
