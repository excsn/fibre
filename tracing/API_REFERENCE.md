# API Reference: fibre_tracing

A `tracing` layer that collects the spans of one unit of work into a trace a caller can read back, keep and forward.

## Contents

* [1. Installing the Layer](#1-installing-the-layer)
  * [`layer`](#layer)
  * [`with_ring`](#with_ring)
  * [`TraceLayer`](#tracelayer)
* [2. Reading](#2-reading)
  * [`Traces`](#traces)
* [3. What a Trace Holds](#3-what-a-trace-holds)
  * [`Trace`](#trace)
  * [`Span`](#span)
  * [`Value`](#value)
* [4. Field Names](#4-field-names)
* [5. Error Handling](#5-error-handling)

## 1. Installing the Layer

Neither constructor sets a global subscriber. The caller composes the layer and installs it.

### `layer`

* `pub fn layer() -> (TraceLayer, Traces)`: the layer and the handle to read it through, with a ring of [`DEFAULT_RING`](#4-field-names).

### `with_ring`

* `pub fn with_ring(ring: usize) -> (TraceLayer, Traces)`: `layer` with the ring sized. `0` keeps no finished trace and still calls listeners.

### `TraceLayer`

`pub struct TraceLayer`. Implements `tracing_subscriber::Layer<S>` for any `S: Subscriber + for<'a> LookupSpan<'a>`.

* `max_level_hint` returns `LevelFilter::TRACE`, so a log sink's level does not clamp what the collector sees.
* A span carrying `fibre.root = true` opens a trace. A span whose nearest enclosing span belongs to a trace joins it. A span in neither is not collected.
* Fields recorded after a span opens are merged into it, so `record` on an `Empty` field lands.
* Closing a span sets its duration. Closing a root finishes the trace, calls every listener in registration order, then pushes it to the ring, dropping the oldest when full.

## 2. Reading

### `Traces`

`pub struct Traces`. `Clone`; every clone shares one collector.

* `pub fn current(&self) -> Option<Trace>`: the trace the current span belongs to, with the spans opened so far. `None` outside a root. The returned `Trace` has `duration` of `Duration::ZERO`, since the root has not closed.
* `pub fn of_span(&self, span: u64) -> Option<Trace>`: `current` for a caller holding a span id rather than being inside it. Takes the value of `tracing::span::Id::into_u64`.
* `pub fn recent(&self, n: usize) -> Vec<Trace>`: the last finished traces, newest last, at most `n` and at most what the ring holds.
* `pub fn len(&self) -> usize` and `pub fn is_empty(&self) -> bool`: how many finished traces the ring holds.
* `pub fn clear(&self)`: drops every finished trace. Does not affect traces in flight.
* `pub fn on_finish(&self, f: impl Fn(&Trace) + Send + Sync + 'static)`: called with each trace as its root closes, before the ring. Listeners run in registration order, on the thread that closed the root, so a slow one delays that close.

## 3. What a Trace Holds

### `Trace`

`pub struct Trace`. `Debug + Clone`.

* `pub id: u64`: unique per collector, from 1, in the order roots opened.
* `pub spans: Vec<Span>`: in the order they opened, the root first.
* `pub duration: Duration`: how long the root was open. `Duration::ZERO` while the trace is in flight.
* `pub fn root(&self) -> Option<&Span>`: the first span.
* `pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Span> + 'a`: every span of a name.

### `Span`

`pub struct Span`. `Debug + Clone`.

* `pub name: &'static str`
* `pub target: String`
* `pub level: tracing::Level`
* `pub parent: Option<usize>`: the parent's index in the trace's own `spans`. `None` at the root.
* `pub depth: usize`: how far it nests, so a waterfall prints without walking parents.
* `pub at: Duration`: when it opened, measured from the root opening.
* `pub duration: Duration`: how long it was open. `Duration::ZERO` while it still is.
* `pub fields: HashMap<String, Value>`: including the `fibre.` fields.
* `pub fn outcome(&self) -> Option<&str>`: the `fibre.outcome` field when it is a string.

### `Value`

`pub enum Value`. `Debug + Clone + PartialEq + Display`.

* `Str(String)`, `Int(i64)`, `UInt(u64)`, `Float(f64)`, `Bool(bool)`.

A field recorded through `Debug` is kept as `Str` of its `{:?}`.

## 4. Field Names

* `pub const ROOT_FIELD: &str = "fibre.root"`: marks a span as the top of a trace. Only a `bool` of `true` counts.
* `pub const OUTCOME_FIELD: &str = "fibre.outcome"`: what `Span::outcome` reads.
* `pub const DEFAULT_RING: usize = 256`: what `layer` sizes the ring to.

## 5. Error Handling

The crate defines no error type and no call returns `Result`.

* A span outside any root is not collected and nothing is reported.
* A field of a type the visitor does not name is kept as `Value::Str` of its `Debug`.
* A full ring drops its oldest trace.
* A listener that panics unwinds through the span close of whoever finished the trace.

Installing the layer can fail in the caller, through `tracing::subscriber::set_global_default`, when a subscriber is already set. Nothing is collected in that case.
