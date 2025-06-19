// src/config/processed.rs
use crate::config::raw::{
  AppenderConfigRaw, ConsoleAppenderConfigRaw, EncoderConfigRaw, FileAppenderConfigRaw,
  JsonLinesEncoderConfigRaw, LoggerConfigRaw, PatternEncoderConfigRaw,
};
use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::Level;
use tracing_appender::rolling::Rotation;
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
  RollingFile(RollingFileAppenderInternal),
  Custom(CustomAppenderInternal),
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
pub struct RollingFileAppenderInternal {
  pub directory: PathBuf,
  pub file_name_prefix: String,
  pub rotation_policy: Rotation,
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
  // No specific options for MVP
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
        let rotation_policy = match raw_rolling.policy.time_granularity.to_lowercase().as_str() {
          "minutely" => Rotation::MINUTELY,
          "hourly" => Rotation::HOURLY,
          "daily" => Rotation::DAILY,
          "never" => Rotation::NEVER,
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

        AppenderKindInternal::RollingFile(RollingFileAppenderInternal {
          directory: PathBuf::from(raw_rolling.directory),
          file_name_prefix: raw_rolling.file_name_prefix,
          rotation_policy,
        })
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
    EncoderConfigRaw::JsonLines(_raw_json) => {
      Ok(EncoderInternal::JsonLines(JsonLinesEncoderInternal {}))
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
