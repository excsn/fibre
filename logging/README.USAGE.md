# Usage Guide: fibre_logging

This guide provides a detailed overview of `fibre_logging`'s core concepts, configuration system, and practical examples for its key features.

## Table of Contents

*   [Core Concepts](#core-concepts)
    *   [Loggers](#loggers)
    *   [Appenders](#appenders)
    *   [Encoders](#encoders)
*   [Quick Start](#quick-start)
*   [The Configuration File](#the-configuration-file)
    *   [Top-Level Structure](#top-level-structure)
    *   [Configuring Appenders](#configuring-appenders)
    *   [Configuring Encoders](#configuring-encoders)
    *   [Configuring Loggers](#configuring-loggers)
*   [Initialization API](#initialization-api)
*   [Specialized Features](#specialized-features)
    *   [Tracing Integration](#tracing-integration)
    *   [In-App Event Consumption (`custom` appender)](#in-app-event-consumption-custom-appender)
    *   [Internal Error Reporting](#internal-error-reporting)
    *   [Debug Reports (`debug_report` appender)](#debug-reports-debug_report-appender)
*   [Error Handling](#error-handling)

## Core Concepts

`fibre_logging`'s behavior is defined by the interaction of three primary components: `Loggers`, `Appenders`, and `Encoders`.

### Loggers

**Loggers** are named filters that control which log events are processed. When a log event is emitted, its **target** (typically the module path, e.g., `my_app::database`) is matched against the configured loggers.

*   A logger has a `level` (e.g., `info`, `debug`, `trace`). An event is only processed if its level is at or above the logger's level.
*   Each logger is associated with one or more `appenders`.
*   The `root` logger is the fallback for any target that doesn't match a more specific logger.
*   The `additive` flag (default: `true`) controls whether an event handled by a specific logger is also passed to the `root` logger's appenders. Setting `additive: false` is useful for isolating a module's logs to a specific file.

### Appenders

**Appenders** are named destinations for log events. They define *where* logs go.

The following appender `kind`s are available:
*   `console`: Writes to standard output.
*   `file`: Writes to a single, non-rolling file.
*   `rolling_file`: Writes to a file that rolls over based on time or size policies.
*   `custom`: Sends structured log events to a channel for consumption within your application.
*   `debug_report`: A debug-only appender that collects events in memory for later inspection.

### Encoders

**Encoders** are responsible for formatting a log event into a stream of bytes before it is sent to an appender. They define *what logs look like*.

The following encoder `kind`s are available:
*   `pattern`: Formats logs using a Log4j-style pattern string (e.g., `[%d] %p - %m%n`).
*   `json_lines`: Formats logs as a stream of newline-delimited JSON objects, ideal for structured logging.

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
            tracing::debug!("Doing some internal work...");
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

## The Configuration File

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

### Configuring Encoders

#### `pattern`
Uses Log4j-style format specifiers.
*   `%d`: Timestamp (e.g., `2023-01-01T12:00:00.000Z`). Custom formats with `{%Y-%m-%d}`.
*   `%p` or `%l`: Log level (e.g., `INFO`).
*   `%t`: Target/module path.
*   `%m`: The log message.
*   `%T`: The thread name.
*   `%n`: A newline.
*   `%-5p`: Padding (left-align level to 5 characters).

```yaml
encoder:
  kind: pattern
  pattern: "[%d{%Y-%m-%d %H:%M:%S}] %-5p [%T] %t - %m%n"
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

  # Rules for any target starting with "my_app".
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

## Initialization API

The primary entry point is `init_from_file`, which sets up the entire logging system.

```rust
pub fn init_from_file(config_path: &Path) -> Result<InitResult>
```

It returns an `InitResult` struct, which is a guard that must be kept alive. When it is dropped, it signals all background threads to shut down and flush any buffered logs.

```rust
pub struct InitResult {
  // Handles to the background I/O threads.
  pub appender_task_handles: Vec<JoinHandle<()>>,

  // Channel to receive internal error reports.
  pub internal_error_rx: Option<fibre::mpsc::BoundedReceiver<InternalErrorReport>>,

  // Map of names to channels for `custom` appenders.
  pub custom_streams: HashMap<String, CustomEventReceiver>,
  
  // ... internal fields
}
```

## Specialized Features

### Tracing Integration

`fibre_logging` automatically captures `tracing` span information.

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
            info!(task_id = 123, "Fetching data for user.");
        });
    }
    ```
*   **Resulting JSON Log (`logs/tracing_example.log`):**
    ```json
    {"level":"INFO","message":"Fetching data for user.","name":"fetch_data","parent_id":"SpanId(0x...)","span_id":"SpanId(0x...)","target":"database","task_id":123,"timestamp":"..."}
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
                // e.g., A file appender failed due to permissions
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