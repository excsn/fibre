// telemetry/src/init.rs

// Contains the primary public initialization functions for fibre_telemetry.

use crate::{
  config::{
    processed::{process_raw_config, AppenderKindInternal, ConfigInternal, LoggerInternal},
    raw::ConfigRaw,
  },
  debug::{print_report_logic, DebugEvent, DEBUG_REPORT_COLLECTOR},
  encoders,
  error::{Error, Result},
  roller::CustomRoller,
  subscriber::{
    actor::{ActorAction, AppenderActor, PerAppenderFilter},
    DispatchLayer, EventProcessor, LogHandler,
  },
  AppenderTaskHandle, CustomEventReceiver, InitResult, InternalErrorReport, LogValue,
  TelemetryEvent,
};

use std::{
  collections::HashMap,
  env,
  fs::{File as StdFsFile, OpenOptions},
  io::{self, Write},
  path::{Path, PathBuf},
  sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
  },
  thread,
  time::{Duration, Instant},
};

use fibre::{mpsc, TryRecvError};
use log::LevelFilter as LogLevelFilter;
use tracing_core::metadata::LevelFilter;
use tracing_subscriber::prelude::*;

const DEFAULT_CONFIG_BASE_NAME: &str = "fibre_telemetry";
const DEFAULT_CONFIG_EXTENSION: &str = "yaml";
const DEFAULT_CHANNEL_SIZE: usize = 1024;

/// Finds the configuration file based on common patterns and an optional environment suffix.
pub fn find_config_file(environment_suffix: Option<&str>) -> Result<PathBuf> {
  let base_name = DEFAULT_CONFIG_BASE_NAME;
  let extension = DEFAULT_CONFIG_EXTENSION;

  let env_from_var = environment_suffix
    .map(|s| s.to_string())
    .or_else(|| env::var("FIBRE_ENV").ok())
    .or_else(|| env::var("APP_ENV").ok());

  let mut files_to_check: Vec<String> = Vec::new();

  if let Some(env_str) = &env_from_var {
    if !env_str.is_empty() {
      files_to_check.push(format!("{}.{}.{}", base_name, env_str, extension));
    }
  }
  files_to_check.push(format!("{}.{}", base_name, extension));

  let search_dirs = [PathBuf::from(".")];

  for dir in &search_dirs {
    for file_name in &files_to_check {
      let path = dir.join(file_name);
      if path.exists() && path.is_file() {
        return Ok(path);
      }
    }
  }

  Err(Error::ConfigNotFound(format!(
    "Searched for: {:?} in {:?}. Provide a config file or check FIBRE_ENV/APP_ENV.",
    files_to_check, search_dirs
  )))
}

/// Initializes `fibre_telemetry` from a configuration file path.
pub fn init_from_file(config_path: &Path) -> Result<InitResult> {
  println!(
    "[fibre_telemetry] Initializing from config file: {:?}",
    config_path
  );

  let file = StdFsFile::open(config_path)?;
  let reader = io::BufReader::new(file);
  let raw_config: ConfigRaw =
    serde_yaml::from_reader(reader).map_err(|e| Error::ConfigParse(e.to_string()))?;

  let internal_config: ConfigInternal = process_raw_config(raw_config)?;
  println!(
    "[fibre_telemetry] Processed Internal Config: {:?}",
    internal_config
  );

  let mut appender_task_handles = Vec::<AppenderTaskHandle>::new();
  let mut actors = Vec::<AppenderActor>::new();
  let mut custom_streams = HashMap::new();

  // --- Create the Shutdown Signal ---
  // This is shared between the final InitResult and all spawned writer threads.
  let shutdown_signal = Arc::new(AtomicBool::new(false));

  let (error_tx_channel, error_rx_channel) = if internal_config.error_reporting_enabled {
    println!("[fibre_telemetry] Internal error reporting is ENABLED.");
    let (tx, rx) = mpsc::bounded::<InternalErrorReport>(256);
    (Some(tx), Some(rx))
  } else {
    (None, None)
  };

  for (appender_name, appender_config) in &internal_config.appenders {
    println!("[fibre_telemetry] Setting up appender: {}", appender_name);

    let formatter = encoders::new_event_formatter(&appender_config.encoder);
    let filter = build_filter_for_appender(appender_name, &internal_config.loggers);

    let action = match &appender_config.kind {
      AppenderKindInternal::DebugReport(debug_config) => {
        let print_interval = debug_config.print_interval;
        // This appender receives raw TelemetryEvents, not formatted bytes.
        let (tx, rx) = mpsc::bounded::<TelemetryEvent>(2048); // A generous buffer

        // Spawn the collector thread.
        thread::spawn(move || {
          let mut last_print_time = Instant::now();

          loop {
            // We can use a simple blocking recv() here because this thread does nothing
            // else. Its shutdown is triggered by the channel disconnecting when the
            // EventProcessor is dropped.
            match rx.try_recv() {
              Ok(event) => {
                let mut collector = DEBUG_REPORT_COLLECTOR.lock();
                // Set the start time on the very first event received.
                if collector.start_time.is_none() {
                  collector.start_time = Some(Instant::now());
                }

                // Check for a special "counter" field.
                if let Some(LogValue::String(counter_name)) = event.fields.get("counter") {
                  let key = (event.target.clone(), counter_name.clone());
                  *collector.counters.entry(key).or_insert(0) += 1;
                } else {
                  // Otherwise, record it as a regular event.
                  let debug_event = DebugEvent {
                    timestamp: Instant::now(), // Use time of collection for sorting
                    level: event.level,
                    target: event.target,
                    message: event.message,
                    fields: event.fields,
                  };
                  collector.events.push(debug_event);
                }
              }
              Err(TryRecvError::Empty) => {
                // Channel is empty. This is fine. We'll check the timer and sleep.
              }
              Err(TryRecvError::Disconnected) => {
                // Senders are gone, shutdown is happening. Exit the loop.
                break;
              }
            }

            // --- Ticker Logic ---
            // After checking for an event, check if it's time to print.
            if let Some(interval) = print_interval {
              if last_print_time.elapsed() >= interval {
                let mut collector = DEBUG_REPORT_COLLECTOR.lock();
                // We check if there's anything to print to avoid empty reports.
                if collector.start_time.is_some() {
                  print_report_logic(&mut collector);
                  // Reset timer ONLY after a successful print.
                  last_print_time = Instant::now();
                }
              }
            }

            // Sleep to prevent busy-waiting if the channel is empty.
            thread::sleep(Duration::from_millis(100));
          }
        });

        // The action is to send the full TelemetryEvent to our collector thread.
        ActorAction::SendEvent(tx)
      }
      AppenderKindInternal::Custom(custom_config) => {
        let (tx, rx) = mpsc::bounded(custom_config.buffer_size);
        custom_streams.insert(appender_name.clone(), rx);
        ActorAction::SendEvent(tx)
      }
      kind => {
        let (tx, rx) = mpsc::bounded::<Vec<u8>>(DEFAULT_CHANNEL_SIZE);

        let mut writer: Box<dyn Write + Send> = match kind {
          AppenderKindInternal::Console(_) => Box::new(io::stdout()),
          AppenderKindInternal::File(file_config) => {
            if let Some(parent_dir) = file_config.path.parent() {
              if !parent_dir.exists() {
                std::fs::create_dir_all(parent_dir).map_err(|e| Error::AppenderSetup {
                  appender_name: appender_name.clone(),
                  reason: format!("Failed to create directory {:?}: {}", parent_dir, e),
                })?;
              }
            }
            let file = OpenOptions::new()
              .create(true)
              .append(true)
              .open(&file_config.path)
              .map_err(|e| Error::AppenderSetup {
                appender_name: appender_name.clone(),
                reason: format!("Failed to open file {:?}: {}", file_config.path, e),
              })?;

            Box::new(io::BufWriter::new(file))
          }
          AppenderKindInternal::RollingFile(policy) => {
            let directory = &policy.directory;
            if !directory.exists() {
              std::fs::create_dir_all(&directory).map_err(|e| Error::AppenderSetup {
                appender_name: appender_name.clone(),
                reason: format!("Failed to create directory {:?}: {}", directory, e),
              })?;
            }
            let roller = CustomRoller::new(policy.clone())?;
            Box::new(roller)
          }
          AppenderKindInternal::Custom(_) => unreachable!(),
          AppenderKindInternal::DebugReport(_) => unreachable!(),
        };

        let appender_name_clone = appender_name.clone();
        let shutdown_clone = Arc::clone(&shutdown_signal); // Clone signal for the thread

        // This is the corrected, non-blocking polling loop.
        let handle = thread::spawn(move || {
          // This flag tracks if the BufWriter has data that hasn't been flushed to disk yet.
          let mut is_dirty = false;

          loop {
            // Check for the shutdown signal first for a fast exit.
            if shutdown_clone.load(Ordering::Relaxed) {
              break;
            }

            match rx.try_recv() {
              Ok(bytes) => {
                // We received a message. Write it to the buffer.
                if writer.write_all(&bytes).is_ok() {
                  // Mark the buffer as "dirty" since it now contains new, unflushed data.
                  is_dirty = true;
                } else {
                  eprintln!(
                    "[fibre_telemetry:ERROR] Appender write failed for '{}'",
                    appender_name_clone
                  );
                }
              }
              Err(TryRecvError::Empty) => {
                // The channel is empty. This is our moment of "downtime".
                // If the buffer is dirty, flush it to disk now.
                if is_dirty {
                  if writer.flush().is_ok() {
                    // The buffer is now clean.
                    is_dirty = false;
                  } else {
                    eprintln!(
                      "[fibre_telemetry:ERROR] Appender flush failed for '{}'",
                      appender_name_clone
                    );
                  }
                }
                // Sleep to yield the CPU and prevent a tight spin loop.
                thread::sleep(Duration::from_millis(50));
              }
              Err(TryRecvError::Disconnected) => {
                // The channel has been closed by the senders, so we can exit.
                break;
              }
            }
          }

          // --- Final Flush on Shutdown ---
          // The loop has exited. Before the thread terminates, we must perform
          // one last check to flush any remaining data.

          // First, drain any messages that might have arrived just before shutdown.
          while let Ok(bytes) = rx.try_recv() {
            if writer.write_all(&bytes).is_ok() {
              is_dirty = true;
            }
          }

          // Now, if the buffer is dirty from either the main loop or the drain loop,
          // perform one final, guaranteed flush.
          if is_dirty {
            if let Err(e) = writer.flush() {
              eprintln!(
                "[fibre_telemetry:ERROR] Final appender flush failed for '{}': {}",
                appender_name_clone, e
              );
            }
          }
        });

        appender_task_handles.push(handle);
        ActorAction::SendBytes(tx)
      }
    };

    actors.push(AppenderActor {
      name: appender_name.clone(),
      filter,
      formatter,
      action,
    });
  }

  let processor = Arc::new(EventProcessor::new(actors, error_tx_channel));

  let dispatch_layer = DispatchLayer::new(Arc::clone(&processor));
  let subscriber = tracing_subscriber::registry().with(dispatch_layer);
  tracing::subscriber::set_global_default(subscriber)
    .map_err(|e| Error::GlobalSubscriberSet(e.to_string()))?;
  println!("[fibre_telemetry] Global tracing subscriber set.");

  let log_handler = LogHandler::new(processor);
  log::set_boxed_logger(Box::new(log_handler))
    .map(|()| log::set_max_level(LogLevelFilter::Trace))
    .map_err(|e| Error::LogBridgeInit(e.to_string()))?;
  println!("[fibre_telemetry] Custom log handler initialized.");

  println!("[fibre_telemetry] Initialization complete.");
  Ok(InitResult {
    appender_task_handles,
    shutdown_signal, // Return the signal for the shutdown function
    internal_error_rx: error_rx_channel,
    custom_streams,
  })
}

fn build_filter_for_appender(
  appender_name_to_build: &str,
  loggers: &HashMap<String, LoggerInternal>,
) -> PerAppenderFilter {
  let mut rules = HashMap::new();
  let mut default_level = LevelFilter::OFF;

  if let Some(root_logger) = loggers.get("root") {
    if root_logger
      .appender_names
      .contains(&appender_name_to_build.to_string())
    {
      default_level = root_logger.min_level;
    }
  }

  for (logger_name, logger_config) in loggers {
    if logger_name == "root" {
      continue;
    }

    if logger_config
      .appender_names
      .contains(&appender_name_to_build.to_string())
    {
      rules.insert(
        logger_config.name.clone(),
        (logger_config.min_level, logger_config.additive),
      );
    }
  }

  PerAppenderFilter::new(rules, default_level)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::processed::LoggerInternal;
  use std::{collections::HashMap, fs, path::Path};
  use tempfile::NamedTempFile;
  use tracing_core::metadata::{LevelFilter, Metadata};
  use tracing_core::{callsite, field, subscriber, Kind, Level};

  fn create_dummy_config_file_for_find(path: &Path) {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
      fs::create_dir_all(parent).expect("Failed to create parent directory for dummy config");
    }
    fs::write(path, "version: 1").expect("Failed to write dummy config");
  }

  #[test]
  fn find_config_file_default() {
    let _ = fs::remove_file(format!(
      "{}.{}",
      DEFAULT_CONFIG_BASE_NAME, DEFAULT_CONFIG_EXTENSION
    ));
    let _ = fs::remove_file(format!(
      "{}.dev.{}",
      DEFAULT_CONFIG_BASE_NAME, DEFAULT_CONFIG_EXTENSION
    ));
    env::remove_var("FIBRE_ENV");
    env::remove_var("APP_ENV");

    let result = find_config_file(None);
    assert!(matches!(result, Err(Error::ConfigNotFound(_))));
  }

  #[test]
  fn find_config_file_not_found() {
    let current_dir_default = PathBuf::from(format!(
      "./{}.{}",
      DEFAULT_CONFIG_BASE_NAME, DEFAULT_CONFIG_EXTENSION
    ));
    let _ = fs::remove_file(&current_dir_default);
    let result = find_config_file(None);
    assert!(matches!(result, Err(Error::ConfigNotFound(_))));
  }

  struct MockCallsite;
  impl callsite::Callsite for MockCallsite {
    fn set_interest(&self, _interest: subscriber::Interest) {}
    fn metadata(&self) -> &Metadata<'_> {
      unimplemented!()
    }
  }
  static MOCK_CALLSITE: MockCallsite = MockCallsite;

  fn mock_metadata(level: Level, target: &'static str) -> Metadata<'static> {
    Metadata::new(
      "test_event",
      target,
      level,
      None,
      None,
      None,
      field::FieldSet::new(&[], callsite::Identifier(&MOCK_CALLSITE)),
      Kind::EVENT,
    )
  }

  #[test]
  fn build_filter_for_appender_correctly_builds_rules() {
    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::INFO,
        appender_names: vec!["console".to_string(), "file_main".to_string()],
        additive: true,
      },
    );
    loggers.insert(
      "my_app".to_string(),
      LoggerInternal {
        name: "my_app".to_string(),
        min_level: LevelFilter::DEBUG,
        appender_names: vec!["file_main".to_string()],
        additive: false,
      },
    );
    loggers.insert(
      "hyper".to_string(),
      LoggerInternal {
        name: "hyper".to_string(),
        min_level: LevelFilter::WARN,
        appender_names: vec!["console".to_string()],
        additive: false,
      },
    );

    let console_filter = build_filter_for_appender("console", &loggers);
    assert!(console_filter.enabled(&mock_metadata(Level::WARN, "hyper::client")));
    assert!(!console_filter.enabled(&mock_metadata(Level::INFO, "hyper::client")));
    assert!(console_filter.enabled(&mock_metadata(Level::INFO, "another_crate")));
    assert!(!console_filter.enabled(&mock_metadata(Level::DEBUG, "another_crate")));

    let file_filter = build_filter_for_appender("file_main", &loggers);
    assert!(file_filter.enabled(&mock_metadata(Level::DEBUG, "my_app::db::queries")));
    assert!(file_filter.enabled(&mock_metadata(Level::INFO, "my_app::db::queries")));
    assert!(!file_filter.enabled(&mock_metadata(Level::TRACE, "my_app::db::queries")));
    assert!(file_filter.enabled(&mock_metadata(Level::INFO, "hyper::server")));
    assert!(!file_filter.enabled(&mock_metadata(Level::DEBUG, "hyper::server")));
  }

  #[test]
  fn init_from_file_basic_stub_still_works() {
    let temp_file = NamedTempFile::new_in(".").expect("Failed to create temp file for init test");
    fs::write(temp_file.path(), "version: 1\nappenders: {}\nloggers: {}")
      .expect("Failed to write to temp file for init test");
    let result = init_from_file(temp_file.path());
    assert!(result.is_ok(), "init_from_file failed: {:?}", result.err());
    if let Ok(init_res) = result {
      assert!(init_res.appender_task_handles.is_empty());
    }
  }
}
