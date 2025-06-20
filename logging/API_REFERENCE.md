# API REFERENCE: `fibre_logging`

This document provides a detailed API reference for the `fibre_logging` library.

### 1. Introduction & Core Concepts

`fibre_logging` is a comprehensive logging solution designed to bridge the gap between Rust's `log` and `tracing` ecosystems. It provides a single, unified sink that seamlessly captures events from both library dependencies using `log` and modern application code instrumented with `tracing`.

*   **Configuration-Driven:** The library's primary architectural principle is its reliance on external YAML configuration. This allows developers to define and modify complex logging behavior (filtering, formatting, destinations) without recompiling the application.
*   **Asynchronous I/O:** All I/O operations (like writing to files) are offloaded to dedicated background threads to prevent blocking application code, making the library suitable for high-performance systems.
*   **Lifecycle Management:** The library is initialized via a single function call which returns a "guard" struct. The logging system remains active as long as this guard is in scope. When the guard is dropped (typically at the end of `main`), it triggers a graceful shutdown, ensuring all buffered logs are flushed.
*   **Unified Event Model:** All log and trace events are converted into a standard internal format, `LogEvent`, allowing for consistent processing and formatting.

#### Primary Entry Points & Handles

*   The main entry point is the `init::init_from_file` function.
*   The primary handle for managing the library's lifecycle is the `InitResult` struct, which is returned by the initialization function.

#### Pervasive Types

*   **`Result<T, Error>`**: Most initialization functions return this custom `Result` type.
*   **`LogEvent`**: The internal representation for all captured log and trace events. It is the type received by custom appender streams.

### 2. Initialization & Lifecycle

The functions and types in this section are used to start, manage, and shut down the logging system.

#### **Struct `InitResult`**

The primary handle for the log system, returned upon successful initialization. Its `Drop` implementation ensures a graceful shutdown of all background logging tasks, flushing any buffered messages. **This struct must be kept in scope for the duration of the application's life.**

**Public Fields:**

*   `appender_task_handles: Vec<AppenderTaskHandle>`
    *   A vector of join handles for the spawned background appender threads.
*   `internal_error_rx: Option<fibre::mpsc::BoundedReceiver<InternalErrorReport>>`
    *   An optional channel receiver that can be used to consume internal error reports from the log system itself (e.g., file write errors). This is only populated if `internal_error_reporting` is enabled in the configuration.
*   `custom_streams: HashMap<String, CustomEventReceiver>`
    *   A map of channel receivers for any appenders configured with `kind: custom`. The key is the appender's name from the configuration file.

#### **Module `init`**

Contains the primary public functions for initializing the library.

**Public Functions:**

*   `pub fn find_config_file(environment_suffix: Option<&str>) -> Result<std::path::PathBuf>`
    *   Finds a configuration file in the current directory. It searches for `fibre_logging.yaml` and `fibre_logging.{env}.yaml` based on the `FIBRE_ENV` or `APP_ENV` environment variables, or the provided suffix.
*   `pub fn init_from_file(config_path: &std::path::Path) -> Result<InitResult>`
    *   Initializes the log system from a specific configuration file path. This is the main entry point for the library.

### 3. Core Data Types

These are the primary data structures that represent log events and their fields.

#### **Struct `LogEvent`**

The internal structured representation of a log or trace event.

**Public Fields:**

*   `timestamp: chrono::DateTime<chrono::Utc>`
*   `level: tracing::Level`
*   `target: String`
*   `name: String`
*   `message: Option<String>`
*   `fields: std::collections::HashMap<String, LogValue>`
*   `span_id: Option<String>`
*   `parent_id: Option<String>`
*   `thread_id: Option<String>`
*   `thread_name: Option<String>`

#### **Enum `LogValue`**

Represents a value associated with a key in a `LogEvent`'s fields.

**Variants:**

*   `String(String)`
*   `Int(i64)`
*   `Float(f64)`
*   `Bool(bool)`
*   `Debug(String)`

### 4. Error Handling

This section details the error types returned by the library.

#### **Type Alias `Result`**

A specialized `Result` type for `fibre_logging` operations.

*   `pub type Result<T, E = Error> = std::result::Result<T, E>;`

#### **Enum `Error`**

The primary error type for initialization failures.

**Variants:**

*   `ConfigNotFound(String)`: Configuration file could not be found.
*   `ConfigRead(#[from] std::io::Error)`: Failed to read the configuration file.
*   `ConfigParse(String)`: Failed to parse the configuration file content (e.g., invalid YAML).
*   `LogBridgeInit(String)`: Failed to initialize the bridge with the `log` crate.
*   `GlobalSubscriberSet(String)`: Failed to set the global `tracing` subscriber.
*   `AppenderSetup { appender_name: String, reason: String }`: An appender failed to initialize (e.g., couldn't open a file).
*   `InvalidConfigValue { field: String, message: String }`: A value in the configuration was invalid.
*   `Internal(String)`: An unexpected internal library error.

#### **Struct `InternalErrorReport`**

Represents a runtime error that occurred within the log system *after* initialization. These are consumed via the `internal_error_rx` field on `InitResult`.

**Public Fields:**

*   `source: InternalErrorSource`
*   `error_message: String`
*   `context: Option<String>`
*   `timestamp: chrono::DateTime<chrono::Utc>`

#### **Enum `InternalErrorSource`**

Describes the origin of an `InternalErrorReport`.

**Variants:**

*   `AppenderWrite { appender_name: String }`
*   `EventFormatting { appender_name: String }`
*   `ConfigProcessing`
*   `CustomRollerIo { path: String }`

### 5. Debug Instrumentation

This module provides functions for interacting with the special `debug_report` appender. These functions are only active in debug builds; in release builds, they are no-ops.

#### **Module `debug_report`**

**Public Functions:**

*   `pub fn print_debug_report()`
    *   Prints a detailed, sorted report of all events and counters captured by any `debug_report` appenders.
*   `pub fn clear_debug_report()`
    *   Clears all data from the in-memory debug report collector.

### 6. Public Type Aliases

*   `pub type AppenderTaskHandle = std::thread::JoinHandle<()>;`
    *   A handle to a spawned appender background task.
*   `pub type CustomEventReceiver = fibre::mpsc::BoundedReceiver<LogEvent>;`
    *   The receiving end of a channel for a `custom` appender, which yields full `LogEvent` structs.