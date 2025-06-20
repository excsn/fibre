// telemetry/src/config/processed.rs

use crate::config::raw::{AppenderConfigRaw, EncoderConfigRaw, LoggerConfigRaw};
use crate::error::{Error, Result};

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::Level;
use tracing_core::metadata::LevelFilter;

// --- Processed Top Level Config ---
#[derive(Debug, Clone)]
pub struct ConfigInternal {
  pub appenders: HashMap<String, AppenderInternal>,
  pub loggers: HashMap<String, LoggerInternal>,
  pub error_reporting_enabled: bool,
}

// --- Processed Appender Config ---
#[derive(Debug, Clone)]
pub struct AppenderInternal {
  pub name: String,
  pub kind: AppenderKindInternal,
  pub encoder: EncoderInternal,
}

#[derive(Debug, Clone)]
pub enum AppenderKindInternal {
  Console(ConsoleAppenderInternal),
  File(FileAppenderInternal),
  RollingFile(RollingPolicyInternal),
  Custom(CustomAppenderInternal),
  DebugReport(DebugReportAppenderInternal),
}

#[derive(Debug, Clone)]
pub struct DebugReportAppenderInternal {
  pub print_interval: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct ConsoleAppenderInternal {
  // No specific fields needed for MVP
}

#[derive(Debug, Clone)]
pub struct FileAppenderInternal {
  pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RollingPolicyInternal {
  pub directory: PathBuf,
  pub file_name_prefix: String,
  pub file_name_suffix: String,
  pub time_granularity: String,
  pub max_file_size: Option<u64>,
  pub max_retained_sequences: Option<u32>,
  pub compression: Option<CompressionPolicyInternal>,
}

impl RollingPolicyInternal {
  // Helpers to generate paths
  pub fn base_path(&self) -> PathBuf {
    self.directory.join(format!(
      "{}{}",
      self.file_name_prefix, self.file_name_suffix
    ))
  }
  pub fn format_period(&self, ts: DateTime<Utc>) -> String {
    match self.time_granularity.as_str() {
      "minutely" => ts.format("%Y-%m-%d_%H-%M-00").to_string(),
      "hourly" => ts.format("%Y-%m-%d_%H-00-00").to_string(),
      "daily" => ts.format("%Y-%m-%d").to_string(),
      // The "never" case will just use the daily format, but the time check will fail.
      _ => ts.format("%Y-%m-%d").to_string(),
    }
  }
  pub fn rolled_path(&self, ts: DateTime<Utc>, sequence: u32) -> PathBuf {
    self.directory.join(format!(
      "{}.{}.{}{}",
      self.file_name_prefix,
      self.format_period(ts),
      sequence,
      self.file_name_suffix
    ))
  }
}

#[derive(Debug, Clone)]
pub struct CompressionPolicyInternal {
  pub compressed_file_suffix: String,
  pub max_uncompressed_sequences: u32,
}

#[derive(Debug, Clone)]
pub struct CustomAppenderInternal {
  pub buffer_size: usize,
}

// --- Processed Encoder Config ---
#[derive(Debug, Clone)]
pub enum EncoderInternal {
  Pattern(PatternEncoderInternal),
  JsonLines(JsonLinesEncoderInternal),
}

#[derive(Debug, Clone)]
pub struct PatternEncoderInternal {
  pub pattern_string: String,
}

#[derive(Debug, Clone)]
pub struct JsonLinesEncoderInternal {
  pub flatten_fields: bool,
}

// --- Processed Logger Config ---
#[derive(Debug, Clone)]
pub struct LoggerInternal {
  pub name: String,
  pub min_level: LevelFilter,
  pub appender_names: Vec<String>,
  pub additive: bool,
}

// --- Helper: Default Encoder ---
impl Default for EncoderInternal {
  fn default() -> Self {
    EncoderInternal::Pattern(PatternEncoderInternal {
      pattern_string: "[%d] %p %t - %m%n".to_string(),
    })
  }
}

// --- Conversion and Validation Logic ---

/// Processes the raw, deserialized configuration into a validated internal representation.
pub fn process_raw_config(raw_config: crate::config::raw::ConfigRaw) -> Result<ConfigInternal> {
  let mut processed_appenders = HashMap::new();
  let mut processed_loggers = HashMap::new();

  // 1. Process Appenders
  for (name, raw_appender) in raw_config.appenders {
    let encoder_internal = match raw_appender.encoder_config_raw() {
      Some(raw_encoder) => process_encoder_config_raw(raw_encoder)?,
      None => EncoderInternal::default(),
    };

    let appender_internal_kind = match raw_appender {
      AppenderConfigRaw::Console(_raw_console) => {
        AppenderKindInternal::Console(ConsoleAppenderInternal {})
      }
      AppenderConfigRaw::File(raw_file) => {
        if raw_file.path.is_empty() {
          return Err(Error::InvalidConfigValue {
            field: format!("appenders.{}.path", name),
            message: "File appender path cannot be empty.".to_string(),
          });
        }
        AppenderKindInternal::File(FileAppenderInternal {
          path: PathBuf::from(raw_file.path),
        })
      }
      AppenderConfigRaw::RollingFile(raw_rolling) => {
        let max_file_size = if let Some(s) = raw_rolling.policy.max_file_size {
          Some(parse_size_str(&s).map_err(|msg| Error::InvalidConfigValue {
            field: format!("appenders.{}.policy.max_file_size", name),
            message: msg,
          })?)
        } else {
          None
        };

        let compression = if let Some(raw_comp) = raw_rolling.policy.compression {
          Some(CompressionPolicyInternal {
            compressed_file_suffix: raw_comp.compressed_file_suffix,
            max_uncompressed_sequences: raw_comp.max_uncompressed_sequences,
          })
        } else {
          None
        };

        match raw_rolling.policy.time_granularity.to_lowercase().as_str() {
          "minutely" | "hourly" | "daily" | "never" => (),
          other => {
            return Err(Error::InvalidConfigValue {
              field: format!("appenders.{}.policy.time_granularity", name),
              message: format!(
              "Unknown time_granularity '{}'. Expected 'minutely', 'hourly', 'daily', or 'never'.",
              other
            ),
            })
          }
        };

        let policy = RollingPolicyInternal {
          directory: PathBuf::from(&raw_rolling.directory),
          file_name_prefix: raw_rolling.file_name_prefix,
          file_name_suffix: raw_rolling.file_name_suffix,
          time_granularity: raw_rolling.policy.time_granularity.to_lowercase(),
          max_file_size,
          max_retained_sequences: raw_rolling.policy.max_retained_sequences,
          compression,
        };
        AppenderKindInternal::RollingFile(policy)
      }
      AppenderConfigRaw::Custom(raw_custom) => {
        if raw_custom.buffer_size == 0 {
          return Err(Error::InvalidConfigValue {
            field: format!("appenders.{}.buffer_size", name),
            message: "Custom appender buffer_size cannot be zero.".to_string(),
          });
        }
        AppenderKindInternal::Custom(CustomAppenderInternal {
          buffer_size: raw_custom.buffer_size,
        })
      }
      AppenderConfigRaw::DebugReport(raw_debug) => {
        let print_interval = raw_debug
          .print_interval
          .map(|s| {
            humantime::parse_duration(&s).map_err(|e| Error::InvalidConfigValue {
              field: format!("appenders.{}.print_interval", name),
              message: format!("Invalid duration string '{}': {}", s, e),
            })
          })
          .transpose()?; // This turns Option<Result<T, E>> into Result<Option<T>, E>

        AppenderKindInternal::DebugReport(DebugReportAppenderInternal { print_interval })
      }
    };

    processed_appenders.insert(
      name.clone(),
      AppenderInternal {
        name,
        kind: appender_internal_kind,
        encoder: encoder_internal,
      },
    );
  }

  // 2. Process Loggers
  // Ensure there's a "root" logger, providing a default if not.
  let mut raw_loggers_mut = raw_config.loggers;
  if !raw_loggers_mut.contains_key("root") {
    println!("[fibre_telemetry::config] No 'root' logger defined, adding a default (level: INFO, appenders: none, additive: true).");
    raw_loggers_mut.insert(
      "root".to_string(),
      LoggerConfigRaw {
        level: "info".to_string(),
        appenders: Vec::new(),
        additive: true,
      },
    );
  }

  for (name, raw_logger) in raw_loggers_mut {
    let min_level = parse_level_filter(&raw_logger.level, &name)?;

    // Validate that specified appenders actually exist
    for appender_name in &raw_logger.appenders {
      if !processed_appenders.contains_key(appender_name) {
        return Err(Error::InvalidConfigValue {
          field: format!("loggers.{}.appenders", name),
          message: format!(
            "Logger '{}' refers to undefined appender '{}'. Available appenders: {:?}",
            name,
            appender_name,
            processed_appenders.keys()
          ),
        });
      }
    }

    processed_loggers.insert(
      name.clone(),
      LoggerInternal {
        name,
        min_level,
        appender_names: raw_logger.appenders,
        additive: raw_logger.additive,
      },
    );
  }

  let error_reporting_enabled = raw_config.internal_error_reporting.enabled;

  Ok(ConfigInternal {
    appenders: processed_appenders,
    loggers: processed_loggers,
    error_reporting_enabled,
  })
}

fn process_encoder_config_raw(raw_encoder: EncoderConfigRaw) -> Result<EncoderInternal> {
  match raw_encoder {
    EncoderConfigRaw::Pattern(raw_pattern) => {
      Ok(EncoderInternal::Pattern(PatternEncoderInternal {
        pattern_string: raw_pattern
          .pattern
          .unwrap_or_else(|| "[%d] %p %t - %m%n".to_string()),
      }))
    }
    EncoderConfigRaw::JsonLines(raw_json) => {
      Ok(EncoderInternal::JsonLines(JsonLinesEncoderInternal {
        flatten_fields: raw_json.flatten_fields,
      }))
    }
  }
}

fn parse_level_filter(level_str: &str, logger_name: &str) -> Result<LevelFilter> {
  // Special case for "OFF" which is not a `tracing::Level`
  if level_str.to_uppercase() == "OFF" {
    return Ok(LevelFilter::OFF);
  }

  level_str
    .to_uppercase()
    .parse::<Level>()
    .map(LevelFilter::from_level)
    .map_err(|_| Error::InvalidConfigValue {
      field: format!("loggers.{}.level", logger_name),
      message: format!(
        "Invalid log level string '{}'. Expected TRACE, DEBUG, INFO, WARN, ERROR, or OFF.",
        level_str
      ),
    })
}

// Add new helper function to parse size strings
fn parse_size_str(size_str: &str) -> std::result::Result<u64, String> {
  let lower = size_str.to_lowercase();
  let (num_str, suffix) = lower.split_at(lower.trim_end_matches(|c: char| c.is_alphabetic()).len());
  let num = num_str
    .trim()
    .parse::<u64>()
    .map_err(|_| format!("Invalid number in size string: '{}'", num_str))?;

  match suffix.trim() {
    "kb" => Ok(num * 1024),
    "mb" => Ok(num * 1024 * 1024),
    "gb" => Ok(num * 1024 * 1024 * 1024),
    "b" | "" => Ok(num),
    _ => Err(format!("Unknown size suffix: '{}'", suffix)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_level_filter_valid_levels() {
    assert_eq!(
      parse_level_filter("TRACE", "test").unwrap(),
      LevelFilter::TRACE
    );
    assert_eq!(
      parse_level_filter("debug", "test").unwrap(),
      LevelFilter::DEBUG
    );
    assert_eq!(
      parse_level_filter("InFo", "test").unwrap(),
      LevelFilter::INFO
    );
    assert_eq!(
      parse_level_filter("warn", "test").unwrap(),
      LevelFilter::WARN
    );
    assert_eq!(
      parse_level_filter("ERROR", "test").unwrap(),
      LevelFilter::ERROR
    );
  }

  #[test]
  fn parse_level_filter_off_level() {
    assert_eq!(parse_level_filter("off", "test").unwrap(), LevelFilter::OFF);
    assert_eq!(parse_level_filter("OFF", "test").unwrap(), LevelFilter::OFF);
  }

  #[test]
  fn parse_level_filter_invalid_level() {
    let result = parse_level_filter("INVALID_LEVEL", "test_logger");
    assert!(result.is_err());
    if let Err(Error::InvalidConfigValue { field, message }) = result {
      assert_eq!(field, "loggers.test_logger.level");
      assert!(message.contains("Invalid log level string 'INVALID_LEVEL'"));
    } else {
      panic!("Expected InvalidConfigValue error");
    }
  }

  #[test]
  fn process_raw_config_adds_default_root_if_missing() {
    let raw_config = crate::config::raw::ConfigRaw {
      version: 1,
      appenders: HashMap::new(),
      loggers: HashMap::new(), // No loggers specified
      internal_error_reporting: Default::default(),
    };
    let internal_config = process_raw_config(raw_config).unwrap();

    assert!(internal_config.loggers.contains_key("root"));
    let root_logger = internal_config.loggers.get("root").unwrap();
    assert_eq!(root_logger.min_level, LevelFilter::INFO);
  }

  #[test]
  fn process_raw_config_validates_appender_existence() {
    let mut loggers = HashMap::new();
    loggers.insert(
      "my_logger".to_string(),
      crate::config::raw::LoggerConfigRaw {
        level: "info".to_string(),
        appenders: vec!["non_existent_appender".to_string()],
        additive: true,
      },
    );
    let raw_config = crate::config::raw::ConfigRaw {
      version: 1,
      appenders: HashMap::new(),
      loggers,
      internal_error_reporting: Default::default(),
    };

    let result = process_raw_config(raw_config);
    assert!(result.is_err());
    if let Err(Error::InvalidConfigValue { field, .. }) = result {
      assert_eq!(field, "loggers.my_logger.appenders");
    } else {
      panic!("Expected InvalidConfigValue error for undefined appender");
    }
  }
}
