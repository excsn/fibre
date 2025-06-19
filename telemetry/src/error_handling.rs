use std::fmt;

#[derive(Debug)]
pub enum InternalErrorSource {
  AppenderWrite { appender_name: String },
  EventFormatting { appender_name: String },
  ConfigProcessing,
  CustomRollerIo { path: String },
}

// Implement Display for user-friendly printing in tests or logs if needed
impl fmt::Display for InternalErrorSource {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      InternalErrorSource::AppenderWrite { appender_name } => {
        write!(
          f,
          "AppenderWrite {{ appender_name: \"{}\" }}",
          appender_name
        )
      }
      InternalErrorSource::EventFormatting { appender_name } => {
        write!(
          f,
          "EventFormatting {{ appender_name: \"{}\" }}",
          appender_name
        )
      }
      InternalErrorSource::ConfigProcessing => write!(f, "ConfigProcessing"),
      InternalErrorSource::CustomRollerIo { path } => {
        write!(f, "CustomRollerIo {{ path: \"{}\" }}", path)
      }
    }
  }
}

#[derive(Debug)]
pub struct InternalErrorReport {
  pub source: InternalErrorSource,
  pub error_message: String, // Store the error message directly as a String
  pub context: Option<String>,
  pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl InternalErrorReport {
  // Constructor now takes any type that implements std::error::Error
  pub(crate) fn new<E: std::error::Error + 'static>(
    source: InternalErrorSource,
    error: E, // The original error
    context: Option<String>,
  ) -> Self {
    Self {
      source,
      error_message: error.to_string(), // Convert the error to its string representation
      context,
      timestamp: chrono::Utc::now(),
    }
  }
}
