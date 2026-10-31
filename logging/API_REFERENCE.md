# API Reference: fibre_logging

A logging library that routes `log` and `tracing` events through appenders and encoders configured from a YAML file.

## Contents

* [1. Initialization and Lifecycle](#1-initialization-and-lifecycle)
  * [`init`](#init)
  * [`InitResult`](#initresult)
  * [`DispatchLayer`](#dispatchlayer)
* [2. Events](#2-events)
  * [`LogEvent`](#logevent)
  * [`LogValue`](#logvalue)
* [3. Debug Instrumentation](#3-debug-instrumentation)
  * [`debug_report`](#debug_report)
* [4. Encoders](#4-encoders)
  * [`EventFormatter`](#eventformatter)
  * [`PatternFormatter`](#patternformatter)
  * [`JsonLinesFormatter`](#jsonlinesformatter)
  * [`encoders::util`](#encodersutil)
* [5. Configuration Types](#5-configuration-types)
  * [`config::raw`](#configraw)
  * [`config::processed`](#configprocessed)
* [6. Type Aliases](#6-type-aliases)
* [7. Error Handling](#7-error-handling)
  * [`Result`](#result)
  * [`Error`](#error)
  * [`InternalErrorReport`](#internalerrorreport)
  * [`InternalErrorSource`](#internalerrorsource)

## 1. Initialization and Lifecycle

### `init`

`find_config_file` and `init_from_file` are also re-exported at the crate root.

* `pub fn find_config_file(environment_suffix: Option<&str>) -> Result<PathBuf>`: searches the current directory. The environment comes from the argument, then `FIBRE_ENV`, then `APP_ENV`. Tries `fibre_logging.{env}.yaml` then `fibre_logging.yaml`, so a missing environment file silently loads the plain one. `Error::ConfigNotFound` only when neither exists.
* `pub fn init_from_file(config_path: &Path) -> Result<InitResult>`: builds the pipeline, sets the global subscriber and installs the `log` bridge. The file is read once; there is no reload. A second call in one process fails with `GlobalSubscriberSet`.
* `pub fn layer_from_file(config_path: &Path) -> Result<(DispatchLayer, InitResult)>`: the same pipeline and the `log` bridge with no global subscriber. Set the composed registry with `tracing::subscriber::set_global_default`. `SubscriberInitExt::init` panics here, since it tries to install a second `log` logger.
* `pub fn console_layer<S>() -> impl tracing_subscriber::Layer<S> + Send + Sync + 'static` where `S: tracing::Subscriber + for<'a> LookupSpan<'a>`: requires the `tokio-console` feature. The tokio-console layer with its gRPC server started. `init_from_file` attaches it itself.

### `InitResult`

`#[must_use] pub struct InitResult`. Dropping it shuts logging down, waiting at most 5 seconds. It must be held for the life of the application.

* `pub appender_task_handles: Vec<AppenderTaskHandle>`: the background appender threads.
* `pub internal_error_rx: Option<fibre::mpsc::BoundedSyncReceiver<InternalErrorReport>>`: `Some` when `internal_error_reporting.enabled` is true. Capacity 256.
* `pub custom_streams: HashMap<String, CustomEventReceiver>`: one receiver per `kind: custom` appender, by appender name. Nothing drains these for you.
* `pub fn shutdown(self, timeout: Duration)`: closes the channels and joins the tasks, waiting at most `timeout`. A task still running at the deadline is reported on stderr and abandoned. Custom-stream receivers drain what is buffered and then see `Disconnected`.

### `DispatchLayer`

`pub struct DispatchLayer` at `subscriber::DispatchLayer`. `Clone`. Implements `tracing_subscriber::Layer<S>` for any `S: Subscriber + for<'span> LookupSpan<'span>`. It has no public constructor; `layer_from_file` returns one. Spans stay enabled regardless of appender levels.

## 2. Events

### `LogEvent`

`pub struct LogEvent`, also at `model::LogEvent`. `Debug + Clone`. The type a custom stream yields.

* `pub timestamp: chrono::DateTime<chrono::Utc>`
* `pub level: tracing::Level`
* `pub target: String`
* `pub name: String`: for a `tracing` event its metadata name. For a `log` record its target.
* `pub message: Option<String>`
* `pub fields: HashMap<String, LogValue>`: a `log` record carries `module_path`, `file` and `line` here.
* `pub span_id: Option<String>`: `{:?}` of the current span's `Id`. Always `None` for a `log` record.
* `pub parent_id: Option<String>`: as `span_id`, for the parent.
* `pub thread_id: Option<String>`: always `None` for a `log` record.
* `pub thread_name: Option<String>`: always `None` for a `log` record.
* `pub fn new<S1: Into<String>, S2: Into<String>>(level: Level, target: S1, name: S2, message: Option<String>) -> Self`

### `LogValue`

`pub enum LogValue`, also at `model::LogValue`. `Debug + Clone + PartialEq + Display`.

* `String(String)`, `Int(i64)`, `Float(f64)`, `Bool(bool)`, `Debug(String)`.

A `u64` above `i64::MAX` is stored as `String`.

## 3. Debug Instrumentation

### `debug_report`

In a release build every function is a no-op, `debug_report_totals` returns `(0, 0)` and a `debug_report` appender is skipped.

* `pub fn print_debug_report()`: prints every event and counter captured by `debug_report` appenders.
* `pub fn clear_debug_report()`: clears the collector.
* `pub fn debug_report_totals() -> (usize, usize)`: recorded events and the sum of all counter values.

## 4. Encoders

### `EventFormatter`

`pub trait EventFormatter: Send + Sync + 'static`. No configuration selects a custom implementation.

* `fn format_event(&self, event: &LogEvent) -> Result<Vec<u8>>`

### `PatternFormatter`

`pub struct PatternFormatter` at `encoders::pattern`. Implements `EventFormatter`.

* `pub fn new(pattern_string: &str) -> Self`

Converters: `%d` and `%d{strftime}`, `%p`, `%l`, `%t`, `%m`, `%T`, `%n`, `%X`, `%X{name}`, `%%`. An unknown converter is `InvalidConfigValue` at init. The default pattern is `[%d] %p %t - %m%n`.

### `JsonLinesFormatter`

`pub struct JsonLinesFormatter` at `encoders::json`. Implements `EventFormatter`.

* `pub fn new(config: JsonLinesEncoderInternal) -> Self`

### `encoders::util`

* `pub fn write_timestamp(buf: &mut String, timestamp: &DateTime<Utc>)`: RFC 3339 with milliseconds and `Z`.
* `pub fn write_timestamp_with_format(buf: &mut String, timestamp: &DateTime<Utc>, format_str: &str)`

## 5. Configuration Types

### `config::raw`

The YAML shape. Every struct has `deny_unknown_fields`, so an unknown key is `ConfigParse`.

* `ConfigRaw`: `version` (default 1), `appenders`, `loggers`, `internal_error_reporting`.
* `InternalErrorReportingRaw`: `enabled` (default false).
* `AppenderConfigRaw`, tagged by `kind`: `Console`, `File`, `RollingFile`, `Custom`, `DebugReport`.
* `ConsoleAppenderConfigRaw`: `encoder`, `channel_capacity` (default 1024), `overflow`.
* `FileAppenderConfigRaw`: `path`, `encoder`, `channel_capacity`, `overflow`.
* `RollingFileAppenderConfigRaw`: `directory`, `file_name_prefix` (default `fibre_logging.log`), `file_name_suffix` (default `.log`), `policy`, `encoder`, `channel_capacity`, `overflow`.
* `RollingPolicyRaw`: `time_granularity` (`minutely`, `hourly`, `daily` or `never`), `max_file_size`, `max_retained_sequences`, `compression`.
* `CompressionPolicyRaw`: `compressed_file_suffix`, `max_uncompressed_sequences` (default 0).
* `CustomAppenderConfigRaw`: `buffer_size` (default 256), `overflow`.
* `DebugReportAppenderConfigRaw`: `print_interval`.
* `OverflowPolicyRaw`: `drop` (the default) or `block`.
* `EncoderConfigRaw`, tagged by `kind`: `Pattern`, `JsonLines`.
* `PatternEncoderConfigRaw`: `pattern`.
* `JsonLinesEncoderConfigRaw`: `flatten_fields` (default false).
* `LoggerConfigRaw`: `level`, `appenders` (default empty), `additive` (default true).
* `pub fn AppenderConfigRaw::encoder_config_raw(&self) -> Option<EncoderConfigRaw>`

### `config::processed`

The validated form.

* `pub fn process_raw_config(raw_config: ConfigRaw) -> Result<ConfigInternal>`
* `ConfigInternal`, `AppenderInternal`, `AppenderKindInternal`, `OverflowPolicy` (`DropNewest`, `Block`), `ConsoleAppenderInternal`, `FileAppenderInternal`, `CustomAppenderInternal`, `DebugReportAppenderInternal`, `CompressionPolicyInternal`, `EncoderInternal`, `PatternEncoderInternal`, `JsonLinesEncoderInternal`, `LoggerInternal`.
* `RollingPolicyInternal`: `pub fn base_path(&self) -> PathBuf`, `pub fn format_period(&self, ts: DateTime<Utc>) -> String`, `pub fn rolled_path(&self, ts: DateTime<Utc>, sequence: u32) -> PathBuf`.

## 6. Type Aliases

* `pub type AppenderTaskHandle = std::thread::JoinHandle<()>;`
* `pub type CustomEventReceiver = fibre::mpsc::BoundedSyncReceiver<LogEvent>;`

## 7. Error Handling

### `Result`

* `pub type Result<T, E = Error> = std::result::Result<T, E>;`

### `Error`

`pub enum Error`. `Debug + std::error::Error`.

* `ConfigNotFound(String)`: `find_config_file` found no file. `init_from_file` on a missing path returns `ConfigRead`.
* `ConfigRead(std::io::Error)`: the file could not be read. `From<std::io::Error>`.
* `ConfigParse(String)`: the content did not parse.
* `LogBridgeInit(String)`: a `log` logger was already set.
* `GlobalSubscriberSet(String)`: a global subscriber was already set.
* `AppenderSetup { appender_name: String, reason: String }`
* `InvalidConfigValue { field: String, message: String }`
* `Internal(String)`

### `InternalErrorReport`

`pub struct InternalErrorReport`. `Debug`. A runtime failure after init, read through `InitResult::internal_error_rx`. Write and flush failures also go to stderr.

* `pub source: InternalErrorSource`
* `pub error_message: String`
* `pub context: Option<String>`
* `pub timestamp: chrono::DateTime<chrono::Utc>`

### `InternalErrorSource`

`pub enum InternalErrorSource`. `Debug + Display`.

* `AppenderWrite { appender_name: String }`, `EventFormatting { appender_name: String }`, `ConfigProcessing`, `CustomRollerIo { path: String }`.
