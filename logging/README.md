# fibre_logging

[![Crates.io](https://img.shields.io/crates/v/fibre_logging.svg)](https://crates.io/crates/fibre_logging)
[![Docs.rs](https://docs.rs/fibre_logging/badge.svg)](https://docs.rs/fibre_logging)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

A logging library that sends `log` and `tracing` events through appenders and encoders configured in a YAML file, so levels, outputs and formats change by editing the file and restarting. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

```toml
[dependencies]
fibre_logging = "0" # Replace with the latest version
log = "0.4"
tracing = "0.1"
```

| Feature | Default | What it does |
|---|---|---|
| `tokio-console` | off | Attaches the tokio-console layer and starts its gRPC server. Needs the `tokio_unstable` cfg. |

## What to reach for

| Problem | Piece |
|---|---|
| Start logging from a config file | `init::init_from_file` |
| Pick a config file by environment | `init::find_config_file` |
| Compose logging with other `tracing` layers | `init::layer_from_file` |
| Write to stdout, a file or rolling files | `console`, `file` and `rolling_file` appenders |
| Roll by time or size, keep N files, compress old ones | `policy` on a `rolling_file` appender |
| Human-readable lines | `pattern` encoder |
| Machine-readable lines | `json_lines` encoder |
| Consume events inside the application | `custom` appender and `InitResult::custom_streams` |
| Learn that the logger itself failed | `internal_error_reporting` and `InitResult::internal_error_rx` |
| Collect events and counters in memory while debugging | `debug_report` appender |
| Inspect async tasks | `tokio-console` feature |

## Notable Users

[Hi Stakes Markets Game](https://www.histakesgame.com) -  The worlds most advanced financial simulator, available on iPhone and Android.
