//! Span collection for the `tracing` ecosystem.
//!
//! `tracing` is fire and forget: a span goes to the subscriber and nothing
//! comes back. That is right for logs and wrong for the three things that want
//! to read a unit of work whole: an error page showing what this request did
//! before it failed, a test asserting what a body actually called, and a
//! header reporting per-step cost with no collector running.
//!
//! This crate is a layer that keeps them. A span marked as a **root** opens a
//! trace, every span under it joins, and closing the root finishes the trace,
//! offers it to whoever is listening and pushes it into a ring of recent ones.
//!
//! It knows nothing about requests. A web server calls its root a request, a
//! queue worker calls it a job, a build calls it a compilation.
//!
//! ```ignore
//! let (layer, traces) = fibre_tracing::layer();
//! tracing_subscriber::registry().with(fibre_logging_layer).with(layer).init();
//!
//! #[tracing::instrument(fields(fibre.root = true))]
//! fn handle() { ... }
//!
//! traces.current();     // inside the root, what has happened so far
//! traces.recent(50);    // the last finished ones
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// The field that marks a span as the top of a trace. Anything under it joins.
pub const ROOT_FIELD: &str = "fibre.root";

/// The field naming the outcome a span finished with, when it says.
pub const OUTCOME_FIELD: &str = "fibre.outcome";

/// How many finished traces the ring keeps by default.
pub const DEFAULT_RING: usize = 256;

/// A field value, in the shapes `tracing` records.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
  Str(String),
  Int(i64),
  UInt(u64),
  Float(f64),
  Bool(bool),
}

impl fmt::Display for Value {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Value::Str(v) => f.write_str(v),
      Value::Int(v) => write!(f, "{v}"),
      Value::UInt(v) => write!(f, "{v}"),
      Value::Float(v) => write!(f, "{v}"),
      Value::Bool(v) => write!(f, "{v}"),
    }
  }
}

/// One span of a trace, as it finished.
#[derive(Debug, Clone)]
pub struct Span {
  pub name: &'static str,
  pub target: String,
  pub level: Level,
  /// Its parent's index in the trace's own list; `None` at the root.
  pub parent: Option<usize>,
  /// How deep it sits, so a caller can print a waterfall without walking.
  pub depth: usize,
  /// When it opened, from the trace's own start.
  pub at: Duration,
  /// How long it was open. Zero while it still is.
  pub duration: Duration,
  pub fields: HashMap<String, Value>,
}

impl Span {
  /// What `fibre.outcome` said, when it said anything.
  pub fn outcome(&self) -> Option<&str> {
    match self.fields.get(OUTCOME_FIELD) {
      Some(Value::Str(s)) => Some(s.as_str()),
      _ => None,
    }
  }
}

/// One unit of work: the root and everything that happened inside it, in the
/// order the spans opened.
#[derive(Debug, Clone)]
pub struct Trace {
  pub id: u64,
  pub spans: Vec<Span>,
  /// How long the root was open. Zero while the trace is still running.
  pub duration: Duration,
}

impl Trace {
  pub fn root(&self) -> Option<&Span> {
    self.spans.first()
  }

  /// Every span of a name, which is how a caller asks "what calls did this
  /// make" without knowing the shape of the tree.
  pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Span> + 'a {
    self.spans.iter().filter(move |s| s.name == name)
  }
}

/// A trace being built.
struct Open {
  id: u64,
  started: Instant,
  spans: Vec<Span>,
  /// Span id to its index in `spans`, so a child finds its parent.
  index: HashMap<u64, usize>,
  /// Every span id the trace took, so the root's close can clear them all
  /// out of `belongs` rather than leaking one per span that outlives it.
  ids: Vec<u64>,
  /// How many spans are still open, so the root closing is unambiguous.
  root: u64,
}

/// The handle a caller reads through, and what the layer writes into.
#[derive(Clone)]
pub struct Traces {
  inner: Arc<Inner>,
}

struct Inner {
  next_id: AtomicU64,
  /// Traces still running, by the id of their root span.
  open: Mutex<HashMap<u64, Open>>,
  /// Every open span's root, so a deep span finds its trace in one lookup.
  belongs: Mutex<HashMap<u64, u64>>,
  finished: Mutex<Vec<Trace>>,
  ring: usize,
  listeners: Mutex<Vec<Arc<dyn Fn(&Trace) + Send + Sync>>>,
}

impl Traces {
  /// The trace the current span belongs to, as far as it has got. `None`
  /// outside a root.
  pub fn current(&self) -> Option<Trace> {
    let id = tracing::Span::current().id()?;
    self.of_span(id.into_u64())
  }

  /// The trace a span id belongs to, for a caller holding one.
  pub fn of_span(&self, span: u64) -> Option<Trace> {
    let root = *self.inner.belongs.lock().get(&span)?;
    let open = self.inner.open.lock();
    let held = open.get(&root)?;
    Some(Trace { id: held.id, spans: held.spans.clone(), duration: Duration::ZERO })
  }

  /// The most recently finished traces, newest last, at most `n`.
  pub fn recent(&self, n: usize) -> Vec<Trace> {
    let held = self.inner.finished.lock();
    let from = held.len().saturating_sub(n);
    held[from..].to_vec()
  }

  /// How many traces the ring is holding.
  pub fn len(&self) -> usize {
    self.inner.finished.lock().len()
  }

  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Drops every finished trace. What a test calls between cases.
  pub fn clear(&self) {
    self.inner.finished.lock().clear();
  }

  /// Called with each trace as its root closes, before it enters the ring.
  /// This is where an exporter goes, and where a caller that wants to keep
  /// only the slow or failed ones decides, which nothing sampling at span
  /// start can do.
  pub fn on_finish(&self, f: impl Fn(&Trace) + Send + Sync + 'static) {
    self.inner.listeners.lock().push(Arc::new(f));
  }
}

/// The layer and the handle to read it through. Compose the layer into a
/// registry beside whatever else is listening.
pub fn layer() -> (TraceLayer, Traces) {
  with_ring(DEFAULT_RING)
}

/// `layer` with the ring sized. Zero keeps nothing, which is what a process
/// that only exports wants.
pub fn with_ring(ring: usize) -> (TraceLayer, Traces) {
  let traces = Traces {
    inner: Arc::new(Inner {
      next_id: AtomicU64::new(1),
      open: Mutex::new(HashMap::new()),
      belongs: Mutex::new(HashMap::new()),
      finished: Mutex::new(Vec::new()),
      ring,
      listeners: Mutex::new(Vec::new()),
    }),
  };
  (TraceLayer { traces: traces.clone() }, traces)
}

pub struct TraceLayer {
  traces: Traces,
}

impl<S> tracing_subscriber::Layer<S> for TraceLayer
where
  S: Subscriber + for<'a> LookupSpan<'a>,
{
  /// Never clamp what reaches the registry: a collector wants every span a
  /// caller opened, whatever the log appenders are configured to write.
  fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
    Some(tracing::level_filters::LevelFilter::TRACE)
  }

  fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
    let mut fields = Fields::default();
    attrs.record(&mut fields);
    let is_root = matches!(fields.map.get(ROOT_FIELD), Some(Value::Bool(true)));

    let this = id.into_u64();
    let parent_span = ctx.span(id).and_then(|s| s.parent().map(|p| p.id().into_u64()));
    let root = match is_root {
      true => this,
      false => match parent_span.and_then(|p| self.traces.inner.belongs.lock().get(&p).copied()) {
        Some(root) => root,
        // Not under any root: nothing to collect it into.
        None => return,
      },
    };

    let mut open = self.traces.inner.open.lock();
    if is_root {
      let trace_id = self.traces.inner.next_id.fetch_add(1, Ordering::Relaxed);
      open.insert(this, Open { id: trace_id, started: Instant::now(), spans: Vec::new(), index: HashMap::new(), ids: Vec::new(), root: this });
    }
    let Some(held) = open.get_mut(&root) else { return };
    let parent = parent_span.and_then(|p| held.index.get(&p).copied());
    let depth = parent.map(|i| held.spans[i].depth + 1).unwrap_or(0);
    let metadata = attrs.metadata();
    held.index.insert(this, held.spans.len());
    held.ids.push(this);
    held.spans.push(Span {
      name: metadata.name(),
      target: metadata.target().to_owned(),
      level: *metadata.level(),
      parent,
      depth,
      at: held.started.elapsed(),
      duration: Duration::ZERO,
      fields: fields.map,
    });
    drop(open);
    self.traces.inner.belongs.lock().insert(this, root);
  }

  fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
    let this = id.into_u64();
    let Some(root) = self.traces.inner.belongs.lock().get(&this).copied() else { return };
    let mut fields = Fields::default();
    values.record(&mut fields);
    let mut open = self.traces.inner.open.lock();
    let Some(held) = open.get_mut(&root) else { return };
    let Some(at) = held.index.get(&this).copied() else { return };
    held.spans[at].fields.extend(fields.map);
  }

  fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
    let this = id.into_u64();
    let Some(root) = self.traces.inner.belongs.lock().remove(&this) else { return };
    let mut open = self.traces.inner.open.lock();
    let Some(held) = open.get_mut(&root) else { return };
    if let Some(at) = held.index.get(&this).copied() {
      held.spans[at].duration = held.started.elapsed().saturating_sub(held.spans[at].at);
    }
    if held.root != this {
      return;
    }
    let Some(held) = open.remove(&root) else { return };
    drop(open);
    {
      let mut belongs = self.traces.inner.belongs.lock();
      for id in &held.ids {
        belongs.remove(id);
      }
    }
    let trace = Trace { id: held.id, duration: held.started.elapsed(), spans: held.spans };
    for listener in self.traces.inner.listeners.lock().iter() {
      listener(&trace);
    }
    if self.traces.inner.ring == 0 {
      return;
    }
    let mut finished = self.traces.inner.finished.lock();
    finished.push(trace);
    let ring = self.traces.inner.ring;
    if finished.len() > ring {
      let over = finished.len() - ring;
      finished.drain(..over);
    }
  }
}

#[derive(Default)]
struct Fields {
  map: HashMap<String, Value>,
}

impl Visit for Fields {
  fn record_str(&mut self, field: &Field, value: &str) {
    self.map.insert(field.name().to_owned(), Value::Str(value.to_owned()));
  }
  fn record_i64(&mut self, field: &Field, value: i64) {
    self.map.insert(field.name().to_owned(), Value::Int(value));
  }
  fn record_u64(&mut self, field: &Field, value: u64) {
    self.map.insert(field.name().to_owned(), Value::UInt(value));
  }
  fn record_bool(&mut self, field: &Field, value: bool) {
    self.map.insert(field.name().to_owned(), Value::Bool(value));
  }
  fn record_f64(&mut self, field: &Field, value: f64) {
    self.map.insert(field.name().to_owned(), Value::Float(value));
  }
  fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
    self.map.insert(field.name().to_owned(), Value::Str(format!("{value:?}")));
  }
}
