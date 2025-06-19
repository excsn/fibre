use thiserror::Error;

/// The main error type for the `fibre_telemetry` library.
#[derive(Debug, Error)]
pub enum Error {
  #[error("Configuration file not found: {0}")]
  ConfigNotFound(String),

  #[error("Failed to read configuration file: {0}")]
  ConfigRead(#[from] std::io::Error), // Allows easy conversion from io::Error

  #[error("Failed to parse configuration: {0}")]
  ConfigParse(String), // Specific parsing errors might come from serde_yaml, etc.

  #[error("Failed to initialize tracing_log bridge: {0}")]
  LogBridgeInit(String),

  #[error("Failed to set global tracing subscriber: {0}")]
  GlobalSubscriberSet(String),

  #[error("Appender setup failed for '{appender_name}': {reason}")]
  AppenderSetup {
    appender_name: String,
    reason: String,
  },

  #[error("Invalid configuration value for '{field}': {message}")]
  InvalidConfigValue { field: String, message: String },

  #[error("Internal library error: {0}")]
  Internal(String), // For unexpected situations
}

/// A specialized `Result` type for `fibre_telemetry` operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;
