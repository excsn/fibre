// src/encoders/util.rs
// Utility functions for encoders.

use chrono::{DateTime, Utc};
use std::fmt::Write;

/// Formats a timestamp into a string buffer using the default RFC3339 format.
/// Example format: "YYYY-MM-DDTHH:MM:SS.millisZ"
pub fn write_timestamp(buf: &mut String, timestamp: &DateTime<Utc>) {
  let _ = write!(
    buf,
    "{}",
    timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
  );
}

/// Formats a timestamp into a string buffer using a custom format string.
/// The format string must be compatible with `chrono::format::strftime`.
pub fn write_timestamp_with_format(buf: &mut String, timestamp: &DateTime<Utc>, format_str: &str) {
  let _ = write!(buf, "{}", timestamp.format(format_str));
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::{TimeZone, Timelike};

  #[test]
  fn test_write_timestamp() {
    let mut buf = String::new();
    let dt = Utc
      .with_ymd_and_hms(2023, 10, 26, 14, 30, 5)
      .unwrap()
      .with_nanosecond(123_456_000)
      .unwrap();
    write_timestamp(&mut buf, &dt);
    assert_eq!(buf, "2023-10-26T14:30:05.123Z");
  }

  #[test]
  fn test_write_timestamp_with_custom_format() {
    let mut buf = String::new();
    let dt = Utc
      .with_ymd_and_hms(2023, 10, 26, 14, 30, 5)
      .unwrap()
      .with_nanosecond(123_456_000)
      .unwrap();
    write_timestamp_with_format(&mut buf, &dt, "%Y-%m-%d %H:%M:%S");
    assert_eq!(buf, "2023-10-26 14:30:05");
  }
}