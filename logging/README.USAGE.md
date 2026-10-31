# Usage Guide: fibre_logging

This guide provides a detailed overview of `fibre_logging`'s core concepts, configuration system and practical examples for its key features.

## Table of Contents

*   [Core Concepts](#core-concepts)
*   [Quick Start](#quick-start)
*   [Writing the Configuration File](#writing-the-configuration-file)
    *   [Top-Level Structure](#top-level-structure)
    *   [Configuring Appenders](#configuring-appenders)
    *   [Configuring Encoders](#configuring-encoders)
    *   [Configuring Loggers](#configuring-loggers)
*   [Initializing the Library](#initializing-the-library)
*   [Using Specialized Features](#using-specialized-features)
    *   [Tracing Integration](#tracing-integration)
    *   [In-App Event Consumption (`custom` appender)](#in-app-event-consumption-custom-appender)
    *   [Internal Error Reporting](#internal-error-reporting)
    *   [Debug Reports (`debug_report` appender)](#debug-reports-debug_report-appender)
    *   [Async Runtime Debugging (tokio-console)](#async-runtime-debugging-tokio-console)
*   [Error Handling](#error-handling)

## Core Concepts

*   **Target**: The name an event is matched on, typically the module path, e.g. `my_app::database`.
*   **Logger**: A named filter with a `level`, zero or more `appenders` and an `additive` flag. Its name matches a target at a `::` boundary.
*   **Level**: An event is only processed if its level is at or above the logger's `level` (e.g. `info`, `debug`, `trace`).
*   **Root Logger**: `root`, the fallback for any target that doesn't match a more specific logger.
*   **Additivity**: The `additive` flag (default: `true`) controls whether an event accepted by a specific logger is also passed to the appenders of its ancestor loggers and of `root`, at the specific logger's level. A logger with a `level` and no `appenders` therefore only changes the level for its targets: `orders::db: { level: debug }` sends `orders::db` debug events to root's appenders and `hyper: { level: warn }` keeps `hyper` info events out of them. Setting `additive: false` isolates a module's logs to a specific file.
*   **Appender**: A named destination for log events. An appender receives an event once, however many loggers list it, so listing `console` on both `root` and a module logger does not print the event twice.
*   **`console`**: An appender that writes to standard output.
*   **`file`**: An appender that writes to a single, non-rolling file.
*   **`rolling_file`**: An appender that writes to a file that rolls over based on time or size policies.
*   **`custom`**: An appender that sends structured log events to a channel for consumption within your application.
*   **`debug_report`**: A debug-only appender that collects events in memory for later inspection.
*   **Encoder**: Formats a log event into bytes before it is sent to an appender.
*   **`pattern`**: An encoder that formats logs using a Log4j-style pattern string (e.g. `[%d] %p - %m%n`).
*   **`json_lines`**: An encoder that formats logs as newline-delimited JSON objects.

## Quick Start

This example logs `INFO` level messages from the root of the application to both the console and a file, while logging more verbose `DEBUG` messages from the `my_app` module to the file only.

1.  **Create `fibre_logging.yaml`:**
    ```yaml
    # fibre_logging.yaml
    version: 1

    appenders:
      console:
        kind: console
        encoder:
          kind: pattern
          pattern: "[%d] %-5p - %m%n"
      main_log_file:
        kind: file
        path: "logs/my_app.log"
        encoder:
          kind: json_lines

    loggers:
      # Default rules for all modules
      root:
        level: info
        appenders: [console, main_log_file]
      
      # Specific rules for 'my_app' module
      my_app:
        level: debug
        appenders: [main_log_file]
        # Prevents my_app logs from also going to the console
        additive: false 
    ```

2.  **Add the code to `main.rs`:**
    ```rust
    use log::{info, debug};
    use std::path::Path;
    
    mod my_app {
        pub fn do_work() {
            tracing::debug!(target: "my_app", "Doing some internal work...");
        }
    }
    
    fn main() {
      // Create the log directory for the example.
      if !Path::new("logs").exists() {
        std::fs::create_dir("logs").unwrap();
      }

      // Initialize the library from the config file.
      // The `_guards` variable must be kept alive for logging to work.
      let _guards = fibre_logging::init::init_from_file(Path::new("fibre_logging.yaml"))
        .expect("Failed to initialize fibre_logging");

      info!("Application started. This goes to console and file.");
      debug!("This root-level debug message will be ignored.");
      
      my_app::do_work(); // "Doing some internal work..." goes to the file only.
      
      info!("Application shutting down.");
      // _guards is dropped here, flushing all logs to disk.
    }
    ```

## Writing the Configuration File

### Top-Level Structure

```yaml
version: 1

# Optional: Enable the internal error reporting channel
internal_error_reporting:
  enabled: true

# Define all possible outputs
appenders:
  #... appender definitions

# Define filtering and routing rules
loggers:
  #... logger definitions
```

### Configuring Appenders

#### `console`
```yaml
appenders:
  my_console:
    kind: console
    encoder:
      kind: pattern
      pattern: "[%d] %-5p %t - %m%n"
```

#### `file`
```yaml
appenders:
  main_log:
    kind: file
    path: "logs/application.log"
    encoder:
      kind: json_lines
```

#### `rolling_file`
```yaml
appenders:
  rolling_log:
    kind: rolling_file
    directory: "logs/archive"
    file_name_prefix: "app"
    file_name_suffix: ".log"
    encoder:
      kind: pattern
      pattern: "%m%n"
    policy:
      # Rolls every day. Other options: "minutely", "hourly", "never".
      time_granularity: daily
      # Rolls when the active file exceeds this size.
      max_file_size: "100MB"
      # Keeps the 10 most recent rolled files.
      max_retained_sequences: 10
```

Compress older rolled files by adding `compression` to the policy:
```yaml
    policy:
      time_granularity: daily
      max_retained_sequences: 10
      compression:
        compressed_file_suffix: ".gz"
        max_uncompressed_sequences: 1
```

#### Channel capacity and overflow

Every appender but `debug_report` has a bounded channel. `console`, `file` and `rolling_file` size it with `channel_capacity` (default 1024, zero is an error). `custom` sizes it with `buffer_size` (default 256, zero is an error). When it is full, `overflow: drop` (the default) drops the newest message and warns on stderr at most once a second. `overflow: block` blocks the logging call.
```yaml
appenders:
  audit:
    kind: file
    path: "logs/audit.log"
    channel_capacity: 4096
    overflow: block
```

### Configuring Encoders

#### `pattern`
Uses Log4j-style format specifiers.

*   `%d`: Timestamp (e.g., `2023-01-01T12:00:00.000Z`). Can be customized with a format string inside braces, like `%d{%Y-%m-%d %H:%M}`.
*   `%p` or `%l`: Log level (e.g., `INFO`).
*   `%t`: The event's target (typically the module path).
*   `%m`: The primary log message.
*   `%T`: The name of the thread on which the event occurred. Empty for `log` records and for unnamed threads.
*   `%n`: A newline (`\n` on every platform).
*   `%-5p`: Padding can be applied to specifiers (e.g., left-align the level to 5 characters).
*   `%%`: An escaped literal percent sign.

**Structured Fields with `%X`**

The `%X` specifier includes an event's structured fields in the log line. It has two forms:

1.  **`%X{field_name}` (Specific Field):** This is the recommended way to include specific, important context. If the field doesn't exist on a given event, nothing is printed.

    ```yaml
    # fibre_logging.yaml
    appenders:
      console:
        kind: console
        encoder:
          kind: pattern
          pattern: "[%d] %p - [peer=%X{peer_id}, ballot=%X{ballot}] - %m%n"
    ```
    With the `tracing` event `warn!(peer_id = 1, ballot = "B(1,2)", "Contention detected");`, the output would be:
    ```log
    [2025-11-01T10:00:00.123Z] WARN - [peer=1, ballot=B(1,2)] - Contention detected
    ```

2.  **`%X` (All Fields):** This will print all fields attached to the event in a `{key=value}` format, sorted alphabetically by key. This is useful for debugging but can be very verbose for production logs. A record from the `log` crate also has `module_path`, `file` and `line` in its fields, so bare `%X` prints them. Use `%X{name}` when `log` records reach the appender.

    ```yaml
    # fibre_logging.yaml
    appenders:
      console_debug:
        kind: console
        encoder:
          kind: pattern
          pattern: "[%d] %p - %m %X%n"
    ```
    The same `tracing` event would produce:
    ```log
    [2025-11-01T10:00:00.123Z] WARN - Contention detected {ballot=B(1,2), peer_id=1}
    ```

**Example Pattern:**

```yaml
encoder:
  kind: pattern
  pattern: "[%d{%Y-%m-%d %H:%M:%S}] %-5p [%T] %t - %m {peer=%X{peer_id}}%n"
```

#### `json_lines`
Produces newline-delimited JSON.
*   `flatten_fields`: If `true`, custom event fields are added to the top-level JSON object. If `false` (default), they are nested under a `fields` key.

```yaml
encoder:
  kind: json_lines
  flatten_fields: true
```

### Configuring Loggers

Loggers form a hierarchy based on the `::` separator in their names.

```yaml
loggers:
  # The default logger for any event that doesn't match a more specific logger.
  root:
    level: info
    appenders: [console]

  # Rules for the target "my_app" and anything under "my_app::".
  my_app:
    level: debug
    appenders: [main_log_file]

  # Rules for a noisy dependency.
  # "additive: false" is critical: it prevents hyper logs from also going to
  # the 'root' logger's appenders (i.e., the console).
  hyper:
    level: warn
    appenders: [main_log_file]
    additive: false
```

#### Using Prefixes to Target Modules

You don't need to list every single module path in your configuration. The keys under the `loggers:` section are module-path prefixes. A key matches a target equal to it or under it at a `::` boundary: `my_app` matches `my_app` and `my_app::db` but not `my_apple`.

**The Rule:** The most specific (longest) matching prefix wins. If no specific logger matches, the `root` logger's configuration is used.

```yaml
loggers:
  # Default for everything else
  root:
    level: info
    appenders: [console]

  # This matches "my_app", "my_app::api", "my_app::db", etc.
  my_app:
    level: debug
    appenders: [file_app]
    additive: false # Prevents my_app logs from also going to the console

  # This quiets a noisy dependency
  hyper:
    level: warn
```

**How it works:**
*   A log from `my_app::api` matches the `my_app` logger and is logged to `file_app` at `DEBUG` level.
*   A log from `hyper::client` matches the `hyper` logger; only `WARN` level logs and higher are processed.
*   A log from another crate like `serde` doesn't match a specific logger, so it uses the `root` configuration and is logged to the `console` at `INFO` level.

#### Reusing Settings with YAML Anchors

You often want to apply the same logging level and appender settings to multiple, unrelated libraries (e.g., to quiet them all down). While the library does not support an `OR` operator like `|` in the logger key, you can achieve this using a standard YAML feature: **Anchors and Aliases**.

1.  **Define an Anchor (`&`):** Put it on the first logger that uses the block.
2.  **Use an Alias (`*`):** Refer to that anchored block wherever you need it.

```yaml
loggers:
  root:
    level: info
    appenders: [console]

  my_app:
    level: debug
    # ...

  # The anchor goes on the first logger that uses the block. An unknown top-level key is a parse error.
  hyper: &quiet_dependency_config
    level: warn
    appenders: [console]
    additive: false
  mio: *quiet_dependency_config
  rustls: *quiet_dependency_config
```

To change the level for all dependencies, edit the one anchored block.

## Initializing the Library

The primary entry point is `init_from_file`, which sets up the entire logging system.

```rust
pub fn init_from_file(config_path: &Path) -> Result<InitResult>
```

It returns an `InitResult`, a guard that must be kept alive. Dropping it flushes the appenders, waiting at most 5 seconds before abandoning a stuck task. To pick the wait yourself:

```rust
use std::time::Duration;

guards.shutdown(Duration::from_secs(30));
```

To pick the file from the environment, `find_config_file` tries `fibre_logging.{env}.yaml` then `fibre_logging.yaml`:

```rust
let path = fibre_logging::find_config_file(None)?;
let guards = fibre_logging::init_from_file(&path)?;
```

To compose with other layers, `layer_from_file` builds the same pipeline and sets no global subscriber:

```rust
use tracing_subscriber::layer::SubscriberExt;

let (logging, guards) = fibre_logging::init::layer_from_file(&path)?;
let subscriber = tracing_subscriber::registry().with(logging).with(my_layer);
tracing::subscriber::set_global_default(subscriber)?;
```

The struct itself:

```rust
pub struct InitResult {
  // Handles to the background I/O threads.
  pub appender_task_handles: Vec<JoinHandle<()>>,

  // Channel to receive internal error reports.
  pub internal_error_rx: Option<fibre::mpsc::BoundedSyncReceiver<InternalErrorReport>>,

  // Map of names to channels for `custom` appenders.
  pub custom_streams: HashMap<String, CustomEventReceiver>,
  
  // ... internal fields
}
```

## Using Specialized Features

### Tracing Integration

`fibre_logging` records the current span's id and its parent's id on each `tracing` event. It does not copy span names or span fields onto the event.

*   **YAML Config (`fibre_logging.tracing.yaml`):**
    ```yaml
    appenders:
      json_log:
        kind: file
        path: "logs/tracing_example.log"
        encoder:
          kind: json_lines
    loggers:
      root:
        level: info
      database:
        level: debug
        appenders: [json_log]
        additive: false
    ```
*   **Rust Code:**
    ```rust
    use tracing::{info, instrument, info_span};

    #[instrument(target = "database")]
    fn process_user(user_id: u32) {
        info_span!("fetch_data").in_scope(|| {
            info!(target: "database", task_id = 123, "Fetching data for user.");
        });
    }
    ```
*   **Resulting JSON Log (`logs/tracing_example.log`):**
    ```json
    {"fields":{"task_id":123},"level":"INFO","message":"Fetching data for user.","name":"event src/main.rs:6","parent_id":"Id(...)","span_id":"Id(...)","target":"database","thread_id":"1","thread_name":"main","timestamp":"..."}
    ```

### In-App Event Consumption (`custom` appender)

Receive log events as structured data within your application.

*   **YAML Config:**
    ```yaml
    appenders:
      metrics_stream:
        kind: custom
        buffer_size: 100
    loggers:
      METRICS: # A custom target
        level: trace
        appenders: [metrics_stream]
    ```
*   **Rust Code:**
    ```rust
    // In main, after initialization...
    let mut init_result = fibre_logging::init::init_from_file(config_path)?;
    if let Some(metrics_rx) = init_result.custom_streams.remove("metrics_stream") {
      std::thread::spawn(move || {
        while let Ok(event) = metrics_rx.recv() {
          println!("Metric Received: {:?}", event.fields);
        }
      });
    }

    // Somewhere else in the code...
    tracing::event!(target: "METRICS", tracing::Level::INFO, query_time_ms = 55);
    ```

Nothing drains a `custom` stream for you. If the receiver is never read the buffer fills and every later event is dropped with the stderr warning.

### Internal Error Reporting

Monitor the health of the logging system itself.

*   **YAML Config:**
    ```yaml
    internal_error_reporting:
      enabled: true
    ```
*   **Rust Code:**
    ```rust
    let mut init_result = fibre_logging::init::init_from_file(config_path)?;
    if let Some(error_rx) = init_result.internal_error_rx.take() {
        std::thread::spawn(move || {
            if let Ok(report) = error_rx.recv() {
                // e.g., A write to a log file failed
                eprintln!("Internal logging error: {}", report.error_message);
            }
        });
    }
    ```

### Debug Reports (`debug_report` appender)

Get an in-memory snapshot of events for debugging. This feature is only available in debug builds (`#[cfg(debug_assertions)]`).

*   **YAML Config:**
    ```yaml
    appenders:
      in_memory_report:
        kind: debug_report
        # Optional: automatically print a report every 10 seconds
        print_interval: 10s
    loggers:
      my_app:
        level: debug
        appenders: [in_memory_report]
        additive: false
    ```
*   **Rust Code:**
    ```rust
    // ... after initialization and running some code ...

    // Manually print the report collected so far.
    fibre_logging::debug_report::print_debug_report();
    ```

An event with a string field named `counter` increments a per-target counter instead of being recorded: `tracing::debug!(counter = "cache_miss");`

### Async Runtime Debugging (tokio-console)

[tokio-console](https://github.com/tokio-rs/console) is an interactive debugger for async Rust programs. It streams live data about every task in your Tokio runtime - scheduling latency, wakeup counts, poll durations and resource contention - via a gRPC server that the `tokio-console` CLI reads from.

The `tokio-console` feature integrates this transparently: when enabled, `init_from_file` attaches the console subscriber layer alongside your normal `fibre_logging` pipeline and starts the gRPC server automatically. No YAML changes are needed.

#### Step 1 - Enable the feature

```toml
# Cargo.toml
[dependencies]
fibre_logging = { version = "0", features = ["tokio-console"] } # Replace with the latest version
tokio = { version = "1", features = ["full"] }
```

#### Step 2 - Set `tokio_unstable`

`console-subscriber` requires Tokio's unstable task instrumentation, which is gated behind a cfg flag. Add it to your project's Cargo config so it applies consistently:

```toml
# .cargo/config.toml  (create this file at the root of your workspace)
[build]
rustflags = ["--cfg", "tokio_unstable"]
```

Alternatively, set it per-session in your shell:

```bash
export RUSTFLAGS="--cfg tokio_unstable"
```

#### Step 3 - Initialize as normal

Your application code does not change. `init_from_file` detects the feature and starts the console server on `localhost:6669`:

```rust
use std::path::Path;

fn main() {
    let _guards = fibre_logging::init::init_from_file(Path::new("fibre_logging.yaml"))
        .expect("Failed to initialize fibre_logging");
    // The tokio-console gRPC server is now running on localhost:6669.
    // All your normal logging config works exactly as before.
}
```

If you compose `layer_from_file` into your own registry instead, add the console layer yourself:

```rust
use tracing_subscriber::layer::SubscriberExt;

let (logging, _guard) = fibre_logging::init::layer_from_file(Path::new("fibre_logging.yaml"))?;
let subscriber = tracing_subscriber::registry()
    .with(logging)
    .with(fibre_logging::init::console_layer());
tracing::subscriber::set_global_default(subscriber)?;
```

#### Step 4 - Install and run `tokio-console`

```bash
cargo install tokio-console
tokio-console           # connects to localhost:6669 by default
```

The console shows a live table of all async tasks: their state (`idle` / `running` / `scheduled`), total poll count, busy and idle time and wakeup sources. Press `Enter` on any task to see its details.

#### Configuration via environment variables

The console server can be tuned without recompiling:

| Variable | Default | Description |
|---|---|---|
| `TOKIO_CONSOLE_BIND` | `127.0.0.1:6669` | Address the gRPC server listens on |
| `TOKIO_CONSOLE_RETENTION` | `3600s` | How long completed task data is retained |
| `TOKIO_CONSOLE_RECORD_PATH` | *(unset)* | Write a recording to a file for replay |

#### Disabling for release builds

The feature is typically only enabled in development. Put it behind a feature of your own so production builds leave it out:

```toml
# Cargo.toml
[features]
debug-console = ["fibre_logging/tokio-console"]

[dependencies]
fibre_logging = "0"       # no tokio-console by default
```

Then build with `cargo run --features debug-console` during development.

## Error Handling

`fibre_logging` has two categories of errors.

1.  **Initialization Errors (`fibre_logging::Error`)**: These are fatal errors that occur during setup (e.g., config file not found, invalid YAML). They are returned from `init_from_file` in a `Result`.
    ```rust
    pub enum Error {
        ConfigNotFound(String),
        ConfigRead(std::io::Error),
        ConfigParse(String),
        // ... and others
    }
    pub type Result<T, E = Error> = std::result::Result<T, E>;
    ```

2.  **Runtime Errors (`InternalErrorReport`)**: These are non-fatal errors that occur after initialization (e.g., a log file cannot be written to). They are sent to the optional error channel if `internal_error_reporting` is enabled.
    ```rust
    pub struct InternalErrorReport {
      pub source: InternalErrorSource,
      pub error_message: String,
      // ... and others
    }
    ```

```rust
match fibre_logging::init_from_file(path) {
    Ok(guards) => run(guards),
    Err(fibre_logging::Error::ConfigRead(e)) => eprintln!("cannot read config: {e}"),
    Err(fibre_logging::Error::ConfigParse(e)) => eprintln!("bad YAML: {e}"),
    Err(e) => eprintln!("logging setup failed: {e}"),
}
```