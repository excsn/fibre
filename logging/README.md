# Fibre Telemetry: Logging

[![Crates.io](https://img.shields.io/crates/v/fibre_logging.svg)](https://crates.io/crates/fibre_logging)
[![Docs.rs](https://docs.rs/fibre_logging/badge.svg)](https://docs.rs/fibre_logging)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

A flexible, multimode sync/async logging library that unifies the `log` and `tracing` ecosystems, driven by external configuration and featuring powerful debug instrumentation.

`fibre_logging` solves the problem of hard-coded logging configurations in Rust applications. It allows operators to dynamically control log levels, outputs, and formats for different parts of an application at runtime, simply by editing a YAML file. This brings the power and flexibility of mature logging frameworks like Log4j or Logback to the modern Rust ecosystem.

## Key Features

### External YAML Configuration
Decouple your logging logic from your application code. All aspects—log levels, outputs, and formats—are defined in a `fibre_logging.yaml` file. Changes to logging behavior no longer require a recompile and redeploy.

### Unified `log` and `tracing` Ecosystems
Instrument your code with your preferred facade. Whether you use `log::info!` or `tracing::info!`, all events are routed through the same configurable pipeline. `fibre_logging` provides a seamless bridge, allowing you to leverage `tracing`'s powerful structured logging and spans even when using the `log` crate.

### High-Performance Async Architecture
Logging I/O is offloaded to dedicated background threads. Application threads submit log events to non-blocking, in-memory channels, ensuring that slow disk writes or network latency never impact your application's performance.

### Structured & Pattern-Based Logging
Choose the output format that fits your needs. Use the `json_lines` encoder for machine-readable, structured logs perfect for log analysis platforms. Or, use the classic `pattern` encoder with Log4j-style formatters (e.g., `[%d] %-5p %t - %m%n`) for human-readable console output.

### Advanced File Rolling
Keep log file sizes manageable with a powerful rolling file appender. Configure files to roll based on a time interval (`minutely`, `hourly`, `daily`), a size limit (`10MB`, `1GB`), or a combination of both. Automatic retention and compression policies are also supported.

### Powerful Debug Instrumentation
Diagnose complex application behavior during development with the `debug_report` appender. This debug-only tool collects targeted events and counters in memory, which can be printed to the console on demand or at a regular interval, providing a snapshot of your application's state.

### Introspection & Extensibility
Turn your logging system into an in-app message bus with the `custom` appender. Your application can subscribe to a stream of structured `LogEvent`s to drive metrics, internal monitoring, or other custom logic. Additionally, the library can report its own internal errors (e.g., file permission issues) to your application for robust error handling.

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
fibre_logging = "0.5.0"
log = "0.4"
tracing = "0.1"
```

## Getting Started

1.  Create a `fibre_logging.yaml` configuration file in your project's root directory.
2.  Initialize the library at the start of your application's `main` function.

```rust
// in main.rs
use std::path::Path;

fn main() {
  // Find and initialize fibre_logging from our config file.
  let config_path = Path::new("fibre_logging.yaml");
  let _guards = fibre_logging::init::init_from_file(config_path)
    .expect("Failed to initialize fibre_logging");

  // The _guards variable must be kept alive for the duration of your
  // application. When it is dropped, all buffered logs are flushed.

  log::info!("Application is running!");
  // ... your application logic
}
```

For a comprehensive guide on configuration, advanced features, and API usage, please see the **[Usage Guide (README.USAGE.md)](README.USAGE.md)**.

The project includes an extensive set of examples in the `examples/` directory that demonstrate nearly every feature, from basic usage to advanced file rolling and custom stream consumption.

The full API reference is available at **[docs.rs/fibre_logging](https://docs.rs/fibre_logging/)**.

## License

This library is distributed under the terms of the **Mozilla Public License Version 2.0 (MPL-2.0)**.
You can find a copy of the license in the [LICENSE](./LICENSE) file or at [https://opensource.org/licenses/MPL-2.0](https://opensource.org/licenses/MPL-2.0).
