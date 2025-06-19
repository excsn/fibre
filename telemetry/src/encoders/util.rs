// src/encoders/util.rs
// Utility functions for encoders.

use chrono::{DateTime, Utc};

/// Formats a timestamp into a string buffer.
/// Example format: "YYYY-MM-DDTHH:MM:SS.microsZ" (ISO 8601 like)
/// For MVP, we'll use a fixed format. Later, this could take a format string.
pub fn write_timestamp(buf: &mut String, timestamp: &DateTime<Utc>) {
  // Using rfc3339 for a standard, sortable format.
  // .to_rfc3339_opts(chrono::SecondsFormat::Micros, true) is precise.
  // For slightly simpler MVP default, let's use a fixed format.
  use std::fmt::Write;
  // Format: YYYY-MM-DD HH:MM:SS.mmmZ (milliseconds, UTC Z)
  // chrono's default Debug format is close to ISO 8601.
  // let _ = write!(buf, "{}", timestamp.format("%Y-%m-%d %H:%M:%S%.3fZ")); // Using chrono's formatting
  let _ = write!(
    buf,
    "{}",
    timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
  );
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
    // Example: "2023-10-26T14:30:05.123Z" (if Millis is used)
    // Check for key components rather than exact string if format is complex
    assert!(buf.contains("2023-10-26"));
    assert!(buf.contains("14:30:05"));
    assert!(buf.contains(".123Z")); // Assuming Millis format
    println!("Formatted timestamp: {}", buf);
  }
}
