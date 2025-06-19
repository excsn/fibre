use serde::Deserialize;
use std::collections::HashMap; // Using HashMap for config keys as order doesn't matter much here

#[derive(Debug, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct InternalErrorReportingRaw {
  #[serde(default)] // Defaults to false if not present
  pub enabled: bool,
  // Later we could add buffer_size here.
}

// --- Top Level Config ---
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
  #[serde(default = "default_version")]
  pub version: u32,
  #[serde(default)] // Appenders can be empty
  pub appenders: HashMap<String, AppenderConfigRaw>,
  #[serde(default)] // Loggers can be empty, will imply a default root
  pub loggers: HashMap<String, LoggerConfigRaw>,
  #[serde(default)]
  pub internal_error_reporting: InternalErrorReportingRaw,
}

fn default_version() -> u32 {
  1
}

// --- Appender Config ---
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)] // "kind" determines the enum variant
pub enum AppenderConfigRaw {
  Console(ConsoleAppenderConfigRaw),
  File(FileAppenderConfigRaw),
  RollingFile(RollingFileAppenderConfigRaw),
  Custom(CustomAppenderConfigRaw),
}

// --- Console Appender ---

#[derive(Debug, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ConsoleAppenderConfigRaw {
  #[serde(default)]
  pub encoder: Option<EncoderConfigRaw>,
}

// --- File Appender ---

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileAppenderConfigRaw {
  pub path: String,
  #[serde(default)]
  pub encoder: Option<EncoderConfigRaw>,
}

// --- Rolling Appender ---

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RollingFileAppenderConfigRaw {
  pub directory: String,
  #[serde(default = "default_file_name_prefix")]
  pub file_name_prefix: String,
  pub policy: RollingPolicyRaw,
  #[serde(default)]
  pub encoder: Option<EncoderConfigRaw>,
}

fn default_file_name_prefix() -> String {
  "fibre_telemetry.log".to_string()
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RollingPolicyRaw {
  /// Determines the time-based rotation frequency.
  /// Expected values: "minutely", "hourly", "daily", or "never".
  pub time_granularity: String,
}

// --- Custom Appender ---

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomAppenderConfigRaw {
  /// The capacity of the underlying `fibre` channel.
  #[serde(default = "default_buffer_size")]
  pub buffer_size: usize,
}

fn default_buffer_size() -> usize {
  256 // A sensible default buffer size
}

// --- Encoder Config ---
#[derive(Debug, Deserialize, PartialEq, Clone)] // Clone because it might be defaulted and copied
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EncoderConfigRaw {
  Pattern(PatternEncoderConfigRaw),
  JsonLines(JsonLinesEncoderConfigRaw),
}

#[derive(Debug, Deserialize, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct PatternEncoderConfigRaw {
  // Default pattern will be applied if this is None or empty
  pub pattern: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct JsonLinesEncoderConfigRaw {
  // For MVP, no specific options. Later: pretty_print, fields_to_include/exclude
  // pub pretty: Option<bool>,
}

// --- Logger Config ---
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoggerConfigRaw {
  pub level: String,
  #[serde(default)]
  pub appenders: Vec<String>, // Names of appenders
  #[serde(default = "default_additive")]
  pub additive: bool,
}

fn default_additive() -> bool {
  true
} // Log events are typically additive by default

// --- Default implementations for testing/convenience ---
impl Default for ConfigRaw {
  fn default() -> Self {
    Self {
      version: default_version(),
      appenders: HashMap::new(),
      loggers: HashMap::new(),
      internal_error_reporting: Default::default(),
    }
  }
}

impl AppenderConfigRaw {
  /// Helper to get the optional encoder configuration from any appender variant.
  pub fn encoder_config_raw(&self) -> Option<EncoderConfigRaw> {
    match self {
      AppenderConfigRaw::Console(c) => c.encoder.clone(),
      AppenderConfigRaw::File(f) => f.encoder.clone(),
      AppenderConfigRaw::RollingFile(r) => r.encoder.clone(),
      AppenderConfigRaw::Custom(_) => None,
    }
  }
}
