use crate::config::processed::RollingPolicyInternal;
use crate::error::{Error, Result};
use crate::error_handling::{InternalErrorReport, InternalErrorSource};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use fibre::mpsc::BoundedSyncSender;
use flate2::write::GzEncoder;
use flate2::Compression;
use once_cell::sync::Lazy;
use regex::Regex;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const DEFAULT_COMPRESSED_SUFFIX: &str = ".gz";

// Regex to parse filenames like: "prefix.YYYY-MM-DD_HH-MM-SS.1.log"
// Captures: 1=timestamp, 2=sequence
static ROLLED_FILE_REGEX: Lazy<Regex> = Lazy::new(|| {
  Regex::new(r"\.((?:\d{4}-\d{2}-\d{2})|(?:\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}))\.(\d+)").unwrap()
});

/// A model representing a parsed rolled file, crucial for correct sorting.
#[derive(Debug, Eq, PartialEq, Clone)]
struct RolledFile {
  timestamp: DateTime<Utc>,
  sequence: u32,
  path: std::path::PathBuf,
  is_compressed: bool,
}

impl Ord for RolledFile {
  fn cmp(&self, other: &Self) -> std::cmp::Ordering {
    // Sort by timestamp DESCENDING (newest first), then by sequence DESCENDING.
    // This ensures retention keeps the most recent files.
    other
      .timestamp
      .cmp(&self.timestamp)
      .then_with(|| other.sequence.cmp(&self.sequence))
  }
}

impl PartialOrd for RolledFile {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

/// A custom Write implementation that handles advanced file rolling correctly.
pub struct CustomRoller {
  policy: RollingPolicyInternal,
  writer: BufWriter<File>,
  current_path: std::path::PathBuf,
  current_size: u64,
  current_period_start: DateTime<Utc>,
  /// Cleanup/compression failures don't fail the write; they are reported here.
  error_tx: Option<BoundedSyncSender<InternalErrorReport>>,
}

impl CustomRoller {
  pub fn new(
    policy: RollingPolicyInternal,
    error_tx: Option<BoundedSyncSender<InternalErrorReport>>,
  ) -> Result<Self> {
    Self::new_at_time(policy, Utc::now(), error_tx)
  }

  /// Testable constructor that allows injecting the current time.
  fn new_at_time(
    policy: RollingPolicyInternal,
    now: DateTime<Utc>,
    error_tx: Option<BoundedSyncSender<InternalErrorReport>>,
  ) -> Result<Self> {
    let current_path = policy.base_path();
    let (writer, current_size) = Self::open_file(&current_path)?;
    let current_period_start = Self::calculate_period_start(now, &policy);

    Ok(Self {
      policy,
      writer,
      current_path,
      current_size,
      current_period_start,
      error_tx,
    })
  }

  /// The suffix marking a rolled file as compressed, per policy.
  fn compressed_suffix(&self) -> &str {
    self
      .policy
      .compression
      .as_ref()
      .map(|c| c.compressed_file_suffix.as_str())
      .unwrap_or(DEFAULT_COMPRESSED_SUFFIX)
  }

  /// Reports a non-fatal roller I/O problem without failing the write path.
  fn report_io_error(&self, path: &Path, error: &io::Error, context: &str) {
    eprintln!(
      "[fibre_logging:WARN] {} {:?}: {}",
      context, path, error
    );
    if let Some(tx) = &self.error_tx {
      let report = InternalErrorReport::new(
        InternalErrorSource::CustomRollerIo {
          path: path.display().to_string(),
        },
        io::Error::new(error.kind(), error.to_string()),
        Some(context.to_string()),
      );
      let _ = tx.try_send(report);
    }
  }

  /// Testable write method that allows injecting the current time.
  #[cfg(test)]
  fn write_at_time(&mut self, buf: &[u8], now: DateTime<Utc>) -> std::io::Result<usize> {
    self.write_internal(buf, now)
  }

  /// Opens the active log file, creating it if necessary.
  fn open_file(path: &Path) -> Result<(BufWriter<File>, u64)> {
    let file = OpenOptions::new()
      .create(true)
      .append(true)
      .open(path)
      .map_err(|e| Error::AppenderSetup {
        appender_name: "rolling_file".to_string(),
        reason: format!("Failed to open log file {:?}: {}", path, e),
      })?;
    let current_size = file.metadata()?.len();
    Ok((BufWriter::new(file), current_size))
  }

  /// The core rolling logic. Made testable by accepting `now`.
  fn roll(&mut self, now: DateTime<Utc>) -> Result<()> {
    // 1. Safely close the current writer.
    #[cfg(unix)]
    let null_path = "/dev/null";
    #[cfg(windows)]
    let null_path = "NUL";
    #[cfg(not(any(unix, windows)))]
    let null_path = "nul";
    let temp_file = OpenOptions::new().write(true).open(null_path)?;
    let dummy_writer = BufWriter::new(temp_file);
    let old_writer = std::mem::replace(&mut self.writer, dummy_writer);
    drop(old_writer);

    // 2. Discover the state of the world.
    let rolled_files = self.find_rolled_files()?;
    let new_period_start = Self::calculate_period_start(now, &self.policy);

    // 3. The file being rolled always belongs to the CURRENT (about to become
    // previous) period, and always gets the next free sequence within that
    // period. Using a fixed sequence of 1 for time-based rolls would rename
    // over an earlier size-rolled file from the same period and destroy it.
    let last_sequence = rolled_files
      .iter()
      .filter(|rf| {
        self.policy.format_period(rf.timestamp)
          == self.policy.format_period(self.current_period_start)
      })
      .map(|rf| rf.sequence)
      .max()
      .unwrap_or(0);
    let (period_for_rolled_file, next_sequence) = (self.current_period_start, last_sequence + 1);

    // 4. Rename the old active file to its new rolled name using the correct period.
    let rolled_path = self
      .policy
      .rolled_path(period_for_rolled_file, next_sequence);
    if self.current_path.exists() {
      fs::rename(&self.current_path, &rolled_path)?;
    }

    // 5. Open a new writer for the active log file and reset state.
    let (new_writer, new_size) = Self::open_file(&self.current_path)?;
    self.writer = new_writer;
    self.current_size = new_size;
    self.current_period_start = new_period_start;

    // 6. Perform cleanup.
    let mut all_files = rolled_files;
    all_files.push(RolledFile {
      timestamp: period_for_rolled_file,
      sequence: next_sequence,
      path: rolled_path,
      is_compressed: false,
    });
    all_files.sort();
    self.cleanup(all_files)?;
    Ok(())
  }

  /// Finds, parses, and correctly sorts all rolled files.
  fn find_rolled_files(&self) -> Result<Vec<RolledFile>> {
    let mut files = Vec::new();
    if !self.policy.directory.exists() {
      return Ok(files);
    }

    for entry in fs::read_dir(&self.policy.directory)? {
      let path = entry?.path();
      if !path.is_file() {
        continue;
      }
      if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        if !file_name.starts_with(&self.policy.file_name_prefix) {
          continue;
        }

        // Check if it's a compressed file, using the configured suffix.
        let compressed_suffix = self.compressed_suffix();
        let is_compressed = file_name.ends_with(compressed_suffix);
        let name_to_parse = if is_compressed {
          // Strip the compressed suffix before parsing with regex
          file_name
            .strip_suffix(compressed_suffix)
            .unwrap_or(file_name)
        } else {
          file_name
        };

        if let Some(caps) = ROLLED_FILE_REGEX.captures(name_to_parse) {
          let ts_str = &caps[1];
          let seq_str = &caps[2];
          if let (Some(timestamp), Ok(sequence)) =
            (parse_datetime_from_str(ts_str), seq_str.parse::<u32>())
          {
            files.push(RolledFile {
              timestamp: Utc.from_utc_datetime(&timestamp),
              sequence,
              path,
              is_compressed,
            });
          }
        }
      }
    }
    files.sort();
    Ok(files)
  }

  /// Deletes old files and compresses recent ones based on policy.
  fn cleanup(&self, mut sorted_files: Vec<RolledFile>) -> Result<()> {
    // sorted_files is already sorted newest-first due to Ord impl

    // --- STEP 1: Apply Retention Policy FIRST ---
    // This ensures we don't waste time compressing files that will be deleted
    if let Some(max_retained) = self.policy.max_retained_sequences {
      // Delete files beyond the retention limit
      for old_file in sorted_files.iter().skip(max_retained as usize) {
        if let Err(e) = fs::remove_file(&old_file.path) {
          self.report_io_error(&old_file.path, &e, "Failed to delete old log file");
        }
      }

      // Keep only the retained files for compression
      sorted_files.truncate(max_retained as usize);
    }

    // --- STEP 2: Apply Compression Policy ---
    // Now compress the files we're actually keeping
    if let Some(compression_config) = &self.policy.compression {
      let uncompressed_to_keep = compression_config.max_uncompressed_sequences as usize;

      // Files to compress are those beyond the "keep uncompressed" threshold.
      // Eligibility is based on the configured file suffix, not a hardcoded
      // ".log" extension.
      for file in sorted_files.iter().skip(uncompressed_to_keep) {
        let has_log_suffix = file
          .path
          .file_name()
          .and_then(|n| n.to_str())
          .map_or(false, |n| n.ends_with(&self.policy.file_name_suffix));
        if !file.is_compressed && has_log_suffix {
          // Compress this file
          if let Err(e) = self.compress_file(&file.path, &compression_config.compressed_file_suffix)
          {
            self.report_io_error(&file.path, &e, "Failed to compress rolled log file");
          }
        }
      }
    }

    Ok(())
  }

  /// Compress a log file using gzip, streaming so large files don't get
  /// buffered in memory.
  fn compress_file(&self, source_path: &Path, compressed_suffix: &str) -> io::Result<()> {
    let compressed_path = PathBuf::from(format!("{}{}", source_path.display(), compressed_suffix));

    let source = File::open(source_path)?;
    let mut reader = BufReader::new(source);

    let output_file = File::create(&compressed_path)?;
    let mut encoder = GzEncoder::new(BufWriter::new(output_file), Compression::default());
    io::copy(&mut reader, &mut encoder)?;
    encoder.finish()?.flush()?;

    // Delete original uncompressed file
    fs::remove_file(source_path)?;

    Ok(())
  }

  fn calculate_period_start(now: DateTime<Utc>, policy: &RollingPolicyInternal) -> DateTime<Utc> {
    match policy.time_granularity.as_str() {
      "minutely" => now
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap(),
      "hourly" => now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap(),
      // "never" anchors every write to one fixed period so a time-based roll
      // can never trigger and size-roll sequences stay monotonic.
      "never" => DateTime::<Utc>::UNIX_EPOCH,
      _ => now
        .with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap(),
    }
  }

  fn write_internal(&mut self, buf: &[u8], now: DateTime<Utc>) -> std::io::Result<usize> {
    let to_io_error = |e: Error| std::io::Error::new(std::io::ErrorKind::Other, e.to_string());

    if Self::calculate_period_start(now, &self.policy) > self.current_period_start {
      self.roll(now).map_err(to_io_error)?;
    }

    let bytes_written = self.writer.write(buf)?;
    if bytes_written > 0 {
      self.current_size += bytes_written as u64;
      if let Some(max_size) = self.policy.max_file_size {
        if self.current_size >= max_size {
          self.roll(now).map_err(to_io_error)?;
        }
      }
    }
    Ok(bytes_written)
  }
}

fn parse_datetime_from_str(s: &str) -> Option<NaiveDateTime> {
  // First, try to parse the full datetime format e.g., "2023-01-01_10-30-15"
  if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d_%H-%M-%S") {
    return Some(dt);
  }

  // If that fails, try to parse the date-only format e.g., "2023-01-01"
  if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
    // and_hms_opt is a method on NaiveDate that returns an Option<NaiveDateTime>
    return date.and_hms_opt(0, 0, 0);
  }

  // If both parsing attempts fail, return None.
  None
}

impl Write for CustomRoller {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    self.write_internal(buf, Utc::now())
  }
  fn flush(&mut self) -> std::io::Result<()> {
    self.writer.flush()
  }
}

// ===================================================================================
//
//                              TESTS
//
// ===================================================================================
#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::{tempdir, TempDir};

  struct TestSetup {
    _temp_dir: TempDir,
    policy: RollingPolicyInternal,
  }

  fn setup(policy_fn: impl FnOnce(&mut RollingPolicyInternal)) -> TestSetup {
    let temp_dir = tempdir().unwrap();
    let mut policy = RollingPolicyInternal {
      directory: temp_dir.path().to_path_buf(),
      file_name_prefix: "test".to_string(),
      file_name_suffix: ".log".to_string(),
      time_granularity: "daily".to_string(),
      max_file_size: None,
      max_retained_sequences: None,
      compression: None,
    };
    policy_fn(&mut policy);
    TestSetup {
      _temp_dir: temp_dir,
      policy,
    }
  }

  fn list_files(policy: &RollingPolicyInternal) -> Vec<String> {
    let mut files = fs::read_dir(&policy.directory)
      .unwrap()
      .map(|res| res.unwrap().file_name().into_string().unwrap())
      .collect::<Vec<String>>();
    files.sort();
    files
  }

  #[test]
  fn test_size_based_roll() {
    let setup = setup(|p| p.max_file_size = Some(10));
    let now = Utc::now();
    let mut roller = CustomRoller::new_at_time(setup.policy, now, None).unwrap();

    // Write 8 bytes, no roll
    roller.write_all(b"12345678").unwrap();
    roller.flush().unwrap();
    let active_file_path = roller.current_path.clone();
    assert_eq!(fs::metadata(&active_file_path).unwrap().len(), 8);
    assert_eq!(list_files(&roller.policy).len(), 1);

    // Write 8 more bytes, pushing total to 16, which is > 10. Should roll.
    roller.write_all(b"abcdefgh").unwrap();
    roller.flush().unwrap();

    let files = list_files(&roller.policy);
    assert_eq!(
      files.len(),
      2,
      "Should be one rolled file and one active file"
    );

    let rolled_file_name = files.iter().find(|f| f.starts_with("test.")).unwrap();
    let active_file_name = files.iter().find(|f| f.starts_with("test.log")).unwrap();
    assert_eq!(active_file_name, "test.log");

    let rolled_path = roller.policy.directory.join(rolled_file_name);
    assert_eq!(
      fs::metadata(&rolled_path).unwrap().len(),
      16,
      "Rolled file should contain all 16 bytes"
    );
    assert_eq!(
      fs::metadata(&roller.current_path).unwrap().len(),
      0,
      "New active file should be empty"
    );
  }

  #[test]
  fn test_time_based_roll() {
    let setup = setup(|p| p.time_granularity = "minutely".to_string());
    let t1 = Utc.with_ymd_and_hms(2023, 1, 1, 10, 30, 15).unwrap();
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), t1, None).unwrap();

    // Write at 10:30:15
    roller.write_at_time(b"first write", t1).unwrap();
    roller.flush().unwrap();
    assert_eq!(list_files(&setup.policy).len(), 1);

    // Write at 10:30:45, same minute, no roll
    let t2 = Utc.with_ymd_and_hms(2023, 1, 1, 10, 30, 45).unwrap();
    roller.write_at_time(b"second write", t2).unwrap();
    roller.flush().unwrap();
    assert_eq!(list_files(&setup.policy).len(), 1);

    // Write at 10:31:05, new minute, should roll
    let t3 = Utc.with_ymd_and_hms(2023, 1, 1, 10, 31, 5).unwrap();
    roller.write_at_time(b"third write", t3).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);
    assert_eq!(files.len(), 2);

    let rolled_path = setup.policy.directory.join(&files[0]); // Alphabetical, so rolled file is first
    assert_eq!(fs::metadata(rolled_path).unwrap().len(), 23); // "first write" + "second write"

    let active_path = setup.policy.directory.join(&files[1]);
    assert_eq!(fs::metadata(active_path).unwrap().len(), 11); // "third write"
  }

  #[test]
  fn test_cleanup_and_retention() {
    let setup = setup(|p| {
      p.max_file_size = Some(10);
      p.max_retained_sequences = Some(2);
    });
    let now = Utc::now();
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), now, None).unwrap();

    // Force 4 rolls
    for i in 0..4 {
      let data = format!("Log message {}", i);
      roller.write_all(data.as_bytes()).unwrap();
      roller.flush().unwrap();
    }

    let files = list_files(&setup.policy);
    // We expect 2 rolled files + 1 active file = 3 total
    assert_eq!(
      files.len(),
      3,
      "Should retain 2 rolled files plus the active one"
    );

    let rolled_files_count = files.iter().filter(|f| !f.ends_with("test.log")).count();
    assert_eq!(rolled_files_count, 2);
  }

  #[test]
  fn test_low_volume_appender_flushes_correctly() {
    // This test specifically verifies the original bug: that a roller which
    // does not roll still gets its data written to disk, as is the case
    // for `app_time.log` in a short run. This is assured by the `writer.flush()`
    // call in the main `init.rs` writer loop, which this test simulates.

    // 1. Setup: A daily roller that will not roll during this test.
    let setup = setup(|p| p.time_granularity = "daily".to_string());
    let now = Utc::now();
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), now, None).unwrap();
    let active_log_path = roller.current_path.clone();

    // 2. Action: Write a small amount of data.
    let write_result = roller.write_all(b"single message");
    assert!(write_result.is_ok());

    // 3. Verification (Before Flush): The data is in the buffer, not yet on disk.
    // The file size should still be 0.
    let metadata_before_flush = fs::metadata(&active_log_path).unwrap();
    assert_eq!(
      metadata_before_flush.len(),
      0,
      "Data should be buffered and not on disk before flush"
    );

    // 4. Action: Explicitly flush the writer. This simulates the `writer.flush()`
    // call that happens in the `init.rs` background thread after every write.
    let flush_result = roller.flush();
    assert!(flush_result.is_ok());

    // 5. Verification (After Flush): The data MUST now be on disk.
    let metadata_after_flush = fs::metadata(&active_log_path).unwrap();
    assert_eq!(
      metadata_after_flush.len(),
      14,
      "Data must be written to disk after an explicit flush"
    );

    // Final check: ensure no rolling occurred.
    let files = list_files(&setup.policy);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], "test.log");
  }

  #[test]
  fn test_low_volume_appender_flushes_correctly_across_multiple_writes() {
    // This test verifies that multiple, separate writes are correctly flushed
    // to disk, simulating the behavior of the `init.rs` writer loop.

    // 1. Setup
    let setup = setup(|p| p.time_granularity = "daily".to_string());
    let now = Utc::now();
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), now, None).unwrap();
    let active_log_path = roller.current_path.clone();

    // --- FIRST WRITE AND FLUSH CYCLE ---

    // 2. Action: First write.
    roller.write_all(b"first message").unwrap();

    // 3. Verification: Data is buffered, file is empty.
    let metadata_before_flush1 = fs::metadata(&active_log_path).unwrap();
    assert_eq!(
      metadata_before_flush1.len(),
      0,
      "File should be empty before the first flush"
    );

    // 4. Action: First flush.
    roller.flush().unwrap();

    // 5. Verification: Data from the first write is now on disk.
    let metadata_after_flush1 = fs::metadata(&active_log_path).unwrap();
    assert_eq!(
      metadata_after_flush1.len(),
      13,
      "File size should match the first message after the first flush"
    );

    // --- SECOND WRITE AND FLUSH CYCLE ---

    // 6. Action: Second write.
    roller.write_all(b" and a second message").unwrap(); // 21 bytes

    // 7. Verification: The file on disk should NOT have changed yet. The new
    // data is in the buffer.
    let metadata_before_flush2 = fs::metadata(&active_log_path).unwrap();
    assert_eq!(
      metadata_before_flush2.len(),
      13,
      "File size should not change before the second flush"
    );

    // 8. Action: Second flush.
    roller.flush().unwrap();

    // 9. Verification: The data from BOTH writes is now on disk. The file has grown.
    let metadata_after_flush2 = fs::metadata(&active_log_path).unwrap();
    let expected_total_size = 13 + 21; // "first message" + " and a second message"
    assert_eq!(
      metadata_after_flush2.len(),
      expected_total_size as u64,
      "File size should reflect the total of both messages after the second flush"
    );

    // Final check: ensure no rolling occurred.
    let files = list_files(&setup.policy);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], "test.log");
  }

  #[test]
  fn test_scenario_for_simple_time_log() {
    // This test directly simulates the configuration and behavior expected
    // from the `simple_time_log` appender in the `rolling_usage.rs` example.

    // 1. Setup: A policy that matches the YAML configuration.
    let setup = setup(|p| {
      p.file_name_prefix = "app_time".to_string();
      p.time_granularity = "minutely".to_string();
    });

    // Define a sequence of timestamps for our test events.
    let t1_start = Utc.with_ymd_and_hms(2025, 6, 20, 10, 30, 15).unwrap();
    let t2_in_minute = Utc.with_ymd_and_hms(2025, 6, 20, 10, 30, 45).unwrap();
    let t3_next_minute = Utc.with_ymd_and_hms(2025, 6, 20, 10, 31, 5).unwrap();

    // Instantiate the roller at the start time.
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), t1_start, None).unwrap();

    // --- Writes within the same minute (10:30) ---

    // 2. Action: First write at 10:30:15. We simulate the full write/flush cycle.
    let msg1 = b"Log message at 10:30:15\n";
    roller.write_at_time(msg1, t1_start).unwrap();
    roller.flush().unwrap();

    // 3. Verification: One file exists, containing the first message.
    assert_eq!(
      list_files(&setup.policy).len(),
      1,
      "Should only have the active file"
    );
    let active_log_path = roller.current_path.clone();
    assert_eq!(
      fs::read_to_string(&active_log_path).unwrap(),
      String::from_utf8_lossy(msg1)
    );

    // 4. Action: Second write at 10:30:45. No roll should occur.
    let msg2 = b"Log message at 10:30:45\n";
    roller.write_at_time(msg2, t2_in_minute).unwrap();
    roller.flush().unwrap();

    // 5. Verification: Still only one file, now containing both messages.
    assert_eq!(
      list_files(&setup.policy).len(),
      1,
      "Should still only have the active file"
    );
    let expected_content1 = format!(
      "{}{}",
      String::from_utf8_lossy(msg1),
      String::from_utf8_lossy(msg2)
    );
    assert_eq!(
      fs::read_to_string(&active_log_path).unwrap(),
      expected_content1
    );

    // --- Write that crosses the minute boundary ---

    // 6. Action: Third write at 10:31:05. THIS MUST TRIGGER A ROLL.
    let msg3 = b"Log message at 10:31:05\n";
    roller.write_at_time(msg3, t3_next_minute).unwrap();
    roller.flush().unwrap();

    // 7. Verification: We now have two files: the rolled one and the new active one.
    let files = list_files(&setup.policy);
    assert_eq!(
      files.len(),
      2,
      "A roll should have occurred, resulting in two files"
    );

    // 8. Verification of the ROLLED file.
    // The file name is based on the START of the period (10:30:00).
    let expected_rolled_filename = format!(
      "app_time.{}.1.log",
      roller.policy.format_period(t1_start) // format_period for 10:30:15 gives "....10-30-00"
    );
    let rolled_file_path = setup.policy.directory.join(expected_rolled_filename);
    assert!(
      rolled_file_path.exists(),
      "The correctly named rolled file must exist"
    );

    // It must contain the first two messages.
    assert_eq!(
      fs::read_to_string(rolled_file_path).unwrap(),
      expected_content1,
      "Rolled file must contain all messages from the previous minute"
    );

    // 9. Verification of the NEW ACTIVE file.
    // The new active file must have the original base name.
    assert!(active_log_path.exists(), "The new active file must exist");
    // It must contain ONLY the third message.
    assert_eq!(
      fs::read_to_string(&active_log_path).unwrap(),
      String::from_utf8_lossy(msg3),
      "New active file should only contain messages from the new minute"
    );
  }

  #[test]
  fn test_retention_with_restart_scenario() {
    use crate::config::processed::CompressionPolicyInternal;

    // This test verifies your exact scenario: old files from October/December 2025
    // should be properly managed when the app restarts in February 2026
    let setup = setup(|p| {
      p.time_granularity = "daily".to_string();
      p.max_retained_sequences = Some(3);
      p.compression = Some(CompressionPolicyInternal {
        compressed_file_suffix: ".gz".to_string(),
        max_uncompressed_sequences: 1,
      });
    });

    // Simulate MULTIPLE old files from previous runs (like your October/December logs)
    // We need MORE than max_retained_sequences to trigger deletion
    let old_log1 = setup.policy.directory.join("test.2025-10-13.1.log");
    let old_log2 = setup.policy.directory.join("test.2025-11-20.1.log");
    let old_log3 = setup.policy.directory.join("test.2025-12-11.1.log");
    let old_log4 = setup.policy.directory.join("test.2025-12-25.1.log");
    fs::write(&old_log1, b"old data from october").unwrap();
    fs::write(&old_log2, b"old data from november").unwrap();
    fs::write(&old_log3, b"old data from december 11").unwrap();
    fs::write(&old_log4, b"old data from december 25").unwrap();

    // Now start the app "today" (Feb 6, 2026)
    let today = Utc.with_ymd_and_hms(2026, 2, 6, 10, 0, 0).unwrap();
    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), today, None).unwrap();

    // Write some data today
    roller.write_at_time(b"today's data", today).unwrap();
    roller.flush().unwrap();

    // Now simulate tomorrow - this should trigger a roll
    // After this roll, we'll have 5 rolled files total (4 old + 1 new)
    // With max_retained_sequences = 3, the 2 oldest should be deleted
    let tomorrow = Utc.with_ymd_and_hms(2026, 2, 7, 10, 0, 0).unwrap();
    roller.write_at_time(b"tomorrow's data", tomorrow).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);

    // Debug output
    println!("Files after roll: {:?}", files);

    // We should have:
    // - test.log (active file, NOT counted in retention)
    // - 3 rolled files (max_retained_sequences = 3)
    // Total = 4 files
    assert_eq!(files.len(), 4, "Should have 3 rolled files + 1 active file");

    // The active file should exist
    assert!(
      files.iter().any(|f| f == "test.log"),
      "Active file should exist"
    );

    // The 2 oldest files should be DELETED
    assert!(
      !files.iter().any(|f| f.contains("2025-10-13")),
      "October file should be deleted as it's beyond retention"
    );
    assert!(
      !files.iter().any(|f| f.contains("2025-11-20")),
      "November file should be deleted as it's beyond retention"
    );

    // The 3 most recent rolled files should exist
    assert!(
      files.iter().any(|f| f.contains("2025-12-25")),
      "December 25 file should still exist"
    );
    assert!(
      files.iter().any(|f| f.contains("2026-02-06")),
      "Yesterday's file should exist and not be deleted"
    );

    // Verify compression: only the most recent rolled file should be uncompressed
    let uncompressed_rolled_files: Vec<_> = files
      .iter()
      .filter(|f| f.ends_with(".log") && *f != "test.log")
      .collect();
    assert_eq!(
      uncompressed_rolled_files.len(),
      1,
      "Only 1 rolled file should be uncompressed (max_uncompressed_sequences = 1)"
    );
    assert!(
      uncompressed_rolled_files[0].contains("2026-02-06"),
      "The most recent rolled file should be uncompressed"
    );
  }

  #[test]
  fn test_never_granularity_does_not_roll_across_days() {
    let setup = setup(|p| p.time_granularity = "never".to_string());
    let day1 = Utc.with_ymd_and_hms(2026, 3, 1, 23, 59, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(2026, 3, 2, 0, 1, 0).unwrap();
    let much_later = Utc.with_ymd_and_hms(2027, 1, 1, 12, 0, 0).unwrap();

    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), day1, None).unwrap();
    roller.write_at_time(b"before midnight\n", day1).unwrap();
    roller.write_at_time(b"after midnight\n", day2).unwrap();
    roller.write_at_time(b"a year later\n", much_later).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);
    assert_eq!(files, vec!["test.log"], "'never' must not time-roll");
  }

  #[test]
  fn test_never_granularity_still_size_rolls_without_clobbering() {
    let setup = setup(|p| {
      p.time_granularity = "never".to_string();
      p.max_file_size = Some(10);
    });
    let day1 = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(2026, 3, 2, 12, 0, 0).unwrap();

    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), day1, None).unwrap();
    // Two size-based rolls on different days: sequences must keep increasing,
    // not restart (which would rename over the first rolled file).
    roller.write_at_time(b"0123456789ab", day1).unwrap();
    roller.write_at_time(b"cdefghijklmn", day2).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);
    assert_eq!(
      files.len(),
      3,
      "expected 2 rolled files + active file, got {:?}",
      files
    );
    let rolled: Vec<_> = files.iter().filter(|f| *f != "test.log").collect();
    assert_eq!(rolled.len(), 2, "both size rolls must survive: {:?}", files);
  }

  #[test]
  fn test_time_roll_does_not_clobber_size_rolled_file() {
    let setup = setup(|p| {
      p.time_granularity = "minutely".to_string();
      p.max_file_size = Some(10);
    });
    let t1 = Utc.with_ymd_and_hms(2026, 3, 1, 10, 30, 5).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 3, 1, 10, 30, 40).unwrap();
    let t3 = Utc.with_ymd_and_hms(2026, 3, 1, 10, 31, 5).unwrap();

    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), t1, None).unwrap();
    // Two size rolls inside minute 10:30 -> sequences 1 and 2.
    roller.write_at_time(b"first-roll--", t1).unwrap();
    roller.write_at_time(b"second-roll-", t2).unwrap();
    // Small write, then crossing into 10:31 rolls the remainder of 10:30.
    roller.write_at_time(b"tail", t2).unwrap();
    roller.write_at_time(b"next minute\n", t3).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);
    // 3 rolled files from minute 10:30 (seq 1, 2, 3) + files from the 10:31
    // write (which itself size-rolled, leaving an empty active file).
    let period = setup.policy.format_period(t1);
    for seq in 1..=3 {
      let expected = format!("test.{}.{}.log", period, seq);
      assert!(
        files.contains(&expected),
        "sequence {} from the 10:30 period must survive the time roll, files: {:?}",
        seq,
        files
      );
    }

    // All three rolled files from 10:30 must have distinct, non-empty content.
    let seq1 = fs::read_to_string(setup.policy.directory.join(format!("test.{}.1.log", period)))
      .unwrap();
    assert!(
      seq1.contains("first-roll"),
      "seq 1 content was clobbered: {:?}",
      seq1
    );
  }

  #[test]
  fn test_compression_with_custom_suffixes() {
    use crate::config::processed::CompressionPolicyInternal;
    use flate2::read::GzDecoder;
    use std::io::Read;

    let setup = setup(|p| {
      p.file_name_suffix = ".txt".to_string();
      p.time_granularity = "never".to_string();
      p.max_file_size = Some(10);
      p.compression = Some(CompressionPolicyInternal {
        compressed_file_suffix: ".gzip".to_string(),
        max_uncompressed_sequences: 0,
      });
    });
    let now = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();

    let mut roller = CustomRoller::new_at_time(setup.policy.clone(), now, None).unwrap();
    roller.write_at_time(b"first roll payload", now).unwrap();
    roller.flush().unwrap();

    let files = list_files(&setup.policy);
    let compressed: Vec<_> = files.iter().filter(|f| f.ends_with(".txt.gzip")).collect();
    assert_eq!(
      compressed.len(),
      1,
      "rolled .txt file must be compressed with the configured suffix, files: {:?}",
      files
    );
    assert!(
      !files.iter().any(|f| f.ends_with(".txt") && *f != "test.txt"),
      "uncompressed rolled original must be removed, files: {:?}",
      files
    );

    // The compressed content must round-trip.
    let compressed_path = setup.policy.directory.join(compressed[0]);
    let mut decoder = GzDecoder::new(File::open(&compressed_path).unwrap());
    let mut contents = String::new();
    decoder.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "first roll payload");

    // A second roll must re-discover the compressed file (configured suffix)
    // and pick the next sequence rather than re-using or re-compressing it.
    roller.write_at_time(b"second roll payload", now).unwrap();
    roller.flush().unwrap();
    let files = list_files(&setup.policy);
    let compressed_count = files.iter().filter(|f| f.ends_with(".txt.gzip")).count();
    assert_eq!(
      compressed_count, 2,
      "second roll must produce a second compressed sequence, files: {:?}",
      files
    );
  }

  #[test]
  fn test_streaming_compression_round_trips_large_file() {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let setup = setup(|p| p.time_granularity = "never".to_string());
    let now = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
    let roller = CustomRoller::new_at_time(setup.policy.clone(), now, None).unwrap();

    // ~3MB of patterned data.
    let payload: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let source_path = setup.policy.directory.join("test.2026-03-01.9.log");
    fs::write(&source_path, &payload).unwrap();

    roller.compress_file(&source_path, ".gz").unwrap();
    assert!(!source_path.exists(), "source must be removed after compression");

    let compressed_path = setup.policy.directory.join("test.2026-03-01.9.log.gz");
    let mut decoder = GzDecoder::new(File::open(&compressed_path).unwrap());
    let mut round_tripped = Vec::new();
    decoder.read_to_end(&mut round_tripped).unwrap();
    assert_eq!(round_tripped, payload, "gunzipped bytes must match source");
  }
}
