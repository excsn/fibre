// Contains the primary public initialization functions for fibre_logging.

use crate::{
  config::{
    processed::{process_raw_config, AppenderKindInternal, ConfigInternal, LoggerInternal},
    raw::ConfigRaw,
  },
  encoders,
  error::{Error, Result},
  error_handling::InternalErrorSource,
  roller::CustomRoller,
  subscriber::{
    actor::{ActorAction, AppenderActor, DropCounter, PerAppenderFilter},
    DispatchLayer, EventProcessor, LogHandler,
  },
  vlog, AppenderTaskHandle, InitResult, InternalErrorReport, LogEvent,
};

#[cfg(debug_assertions)]
use crate::debug_report::{print_report_logic, DebugEvent, DebugReportData, DEBUG_REPORT_COLLECTOR};
#[cfg(debug_assertions)]
use crate::LogValue;

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

use fibre::{mpsc, RecvErrorTimeout};
use log::LevelFilter as LogLevelFilter;
use tracing_core::metadata::LevelFilter;
use tracing_subscriber::prelude::*;

const DEFAULT_CONFIG_BASE_NAME: &str = "fibre_logging";
const DEFAULT_CONFIG_EXTENSION: &str = "yaml";

/// How long appender tasks block waiting for a message before re-checking
/// the shutdown signal.
const WRITER_RECV_TIMEOUT: Duration = Duration::from_millis(50);
/// Under sustained load, flush at least this often so a crash can't lose
/// more than this window of buffered output.
const MAX_FLUSH_INTERVAL: Duration = Duration::from_millis(250);
/// Bound on how many queued messages are drained per wakeup, so a firehose
/// producer can't starve the periodic flush.
const WRITER_DRAIN_BATCH_MAX: usize = 256;

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

/// Initializes `fibre_logging` from a configuration file path.
pub fn init_from_file(config_path: &Path) -> Result<InitResult> {
  vlog!(
    "[fibre_logging] Initializing from config file: {:?}",
    config_path
  );

  let file = StdFsFile::open(config_path)?;
  let reader = io::BufReader::new(file);
  let raw_config: ConfigRaw =
    serde_yaml::from_reader(reader).map_err(|e| Error::ConfigParse(e.to_string()))?;

  let internal_config: ConfigInternal = process_raw_config(raw_config)?;
  vlog!(
    "[fibre_logging] Processed Internal Config: {:?}",
    internal_config
  );

  let mut appender_task_handles = Vec::<AppenderTaskHandle>::new();
  let mut appender_task_names = Vec::<String>::new();
  let mut actors = Vec::<AppenderActor>::new();
  let mut custom_streams = HashMap::new();

  // --- Create the Shutdown Signal ---
  // This is shared between the final InitResult and all spawned writer threads.
  let shutdown_signal = Arc::new(AtomicBool::new(false));

  let (error_tx_channel, error_rx_channel) = if internal_config.error_reporting_enabled {
    vlog!("[fibre_logging] Internal error reporting is ENABLED.");
    let (tx, rx) = mpsc::bounded::<InternalErrorReport>(256);
    (Some(tx), Some(rx))
  } else {
    (None, None)
  };

  for (appender_name, appender_config) in &internal_config.appenders {
    vlog!("[fibre_logging] Setting up appender: {}", appender_name);

    let formatter = encoders::new_event_formatter(&appender_config.encoder);
    let filter = build_filter_for_appender(appender_name, &internal_config.loggers);

    let action = match &appender_config.kind {
      #[cfg(debug_assertions)]
      AppenderKindInternal::DebugReport(debug_config) => {
        let print_interval = debug_config.print_interval;
        // This appender receives raw LogEvents, not formatted bytes.
        let (tx, rx) = mpsc::bounded::<LogEvent>(appender_config.channel_capacity);
        let shutdown_clone = Arc::clone(&shutdown_signal);

        // Spawn the collector thread.
        let handle = thread::spawn(move || {
          fn collect(collector: &mut DebugReportData, event: LogEvent) {
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
              collector.events.push(DebugEvent {
                timestamp: Instant::now(), // Use time of collection for sorting
                level: event.level,
                target: event.target,
                message: event.message,
                fields: event.fields,
              });
            }
          }

          let mut last_print_time = Instant::now();

          loop {
            if shutdown_clone.load(Ordering::Relaxed) {
              break;
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
              Ok(event) => {
                // Hold the lock once per batch and drain everything queued.
                let mut collector = DEBUG_REPORT_COLLECTOR.lock();
                collect(&mut collector, event);
                while let Ok(event) = rx.try_recv() {
                  collect(&mut collector, event);
                }
              }
              Err(RecvErrorTimeout::Timeout) => {}
              Err(RecvErrorTimeout::Disconnected) => break,
            }

            // --- Ticker Logic ---
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
          }

          // Shutdown: drain stragglers, then emit a final interval report so
          // the last window isn't silently lost.
          {
            let mut collector = DEBUG_REPORT_COLLECTOR.lock();
            while let Ok(event) = rx.try_recv() {
              collect(&mut collector, event);
            }
            if print_interval.is_some() && collector.start_time.is_some() {
              print_report_logic(&mut collector);
            }
          }
        });

        appender_task_handles.push(handle);
        appender_task_names.push(appender_name.clone());

        // The action is to send the full LogEvent to our collector thread.
        ActorAction::SendEvent(tx)
      }
      // In release builds the debug_report module is stubbed out; the
      // appender is skipped instead of panicking, so one config can serve
      // both profiles.
      #[cfg(not(debug_assertions))]
      AppenderKindInternal::DebugReport(_) => {
        vlog!(
          "[fibre_logging] Appender '{}' (debug_report) is disabled in release builds; skipping.",
          appender_name
        );
        continue;
      }
      AppenderKindInternal::Custom(_) => {
        let (tx, rx) = mpsc::bounded::<LogEvent>(appender_config.channel_capacity);
        custom_streams.insert(appender_name.clone(), rx);
        ActorAction::SendEvent(tx)
      }
      kind => {
        let (tx, rx) = mpsc::bounded::<Vec<u8>>(appender_config.channel_capacity);

        let writer: Box<dyn Write + Send> = match kind {
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
            let roller = CustomRoller::new(policy.clone(), error_tx_channel.clone())?;
            Box::new(roller)
          }
          AppenderKindInternal::Custom(_) => unreachable!(),
          AppenderKindInternal::DebugReport(_) => unreachable!(),
        };

        let handle = thread::spawn({
          let appender_name = appender_name.clone();
          let shutdown = Arc::clone(&shutdown_signal);
          let error_tx = error_tx_channel.clone();
          move || {
            run_byte_appender_writer(
              rx,
              writer,
              &appender_name,
              &shutdown,
              &error_tx,
              MAX_FLUSH_INTERVAL,
            )
          }
        });

        appender_task_handles.push(handle);
        appender_task_names.push(appender_name.clone());
        ActorAction::SendBytes(tx)
      }
    };

    actors.push(AppenderActor {
      name: appender_name.clone(),
      filter,
      formatter,
      action,
      overflow: appender_config.overflow,
      drops: DropCounter::default(),
    });
  }

  let processor = Arc::new(EventProcessor::new(actors, error_tx_channel));
  let max_level = processor.max_level();

  let dispatch_layer = DispatchLayer::new(Arc::clone(&processor));

  #[cfg(feature = "tokio-console")]
  let subscriber = tracing_subscriber::registry()
    .with(dispatch_layer)
    .with(console_subscriber::spawn()); // Attaches the console layer and starts its server

  #[cfg(not(feature = "tokio-console"))]
  let subscriber = tracing_subscriber::registry().with(dispatch_layer);

  tracing::subscriber::set_global_default(subscriber)
    .map_err(|e| Error::GlobalSubscriberSet(e.to_string()))?;
  vlog!("[fibre_logging] Global tracing subscriber set.");

  let log_handler = LogHandler::new(Arc::clone(&processor));
  log::set_boxed_logger(Box::new(log_handler))
    .map(|()| log::set_max_level(tracing_filter_to_log_filter(max_level)))
    .map_err(|e| Error::LogBridgeInit(e.to_string()))?;
  vlog!("[fibre_logging] Custom log handler initialized.");

  vlog!("[fibre_logging] Initialization complete.");
  Ok(InitResult {
    appender_task_handles,
    appender_task_names,
    shutdown_signal, // Return the signal for the shutdown function
    processor: Some(processor),
    internal_error_rx: error_rx_channel,
    custom_streams,
  })
}

/// Maps the config-derived tracing max level to the `log` crate's filter so
/// the `log` bridge short-circuits records no appender could accept.
fn tracing_filter_to_log_filter(filter: LevelFilter) -> LogLevelFilter {
  if filter == LevelFilter::OFF {
    LogLevelFilter::Off
  } else if filter == LevelFilter::ERROR {
    LogLevelFilter::Error
  } else if filter == LevelFilter::WARN {
    LogLevelFilter::Warn
  } else if filter == LevelFilter::INFO {
    LogLevelFilter::Info
  } else if filter == LevelFilter::DEBUG {
    LogLevelFilter::Debug
  } else {
    LogLevelFilter::Trace
  }
}

/// Reports an appender I/O failure to stderr and, when enabled, the internal
/// error channel.
fn report_write_error(
  error_tx: &Option<mpsc::BoundedSyncSender<InternalErrorReport>>,
  appender_name: &str,
  error: &io::Error,
  context: &str,
) {
  eprintln!(
    "[fibre_logging:ERROR] {} for appender '{}': {}",
    context, appender_name, error
  );
  if let Some(tx) = error_tx {
    let report = InternalErrorReport::new(
      InternalErrorSource::AppenderWrite {
        appender_name: appender_name.to_string(),
      },
      io::Error::new(error.kind(), error.to_string()),
      Some(context.to_string()),
    );
    let _ = tx.try_send(report);
  }
}

/// The background loop for byte-oriented appenders (console, file, rolling).
///
/// Blocks on the channel with a short timeout (instead of polling with a
/// sleep), drains queued messages in bounded batches, flushes when idle, and
/// additionally flushes at least every `max_flush_interval` under sustained
/// load so buffered output is never more than that window behind.
fn run_byte_appender_writer(
  rx: mpsc::BoundedSyncReceiver<Vec<u8>>,
  mut writer: Box<dyn Write + Send>,
  appender_name: &str,
  shutdown: &AtomicBool,
  error_tx: &Option<mpsc::BoundedSyncSender<InternalErrorReport>>,
  max_flush_interval: Duration,
) {
  let mut is_dirty = false;
  let mut last_flush = Instant::now();

  fn write_one(
    writer: &mut (dyn Write + Send),
    bytes: &[u8],
    is_dirty: &mut bool,
    appender_name: &str,
    error_tx: &Option<mpsc::BoundedSyncSender<InternalErrorReport>>,
  ) {
    match writer.write_all(bytes) {
      Ok(()) => *is_dirty = true,
      Err(e) => report_write_error(error_tx, appender_name, &e, "Write failed"),
    }
  }

  fn flush(
    writer: &mut (dyn Write + Send),
    is_dirty: &mut bool,
    last_flush: &mut Instant,
    appender_name: &str,
    error_tx: &Option<mpsc::BoundedSyncSender<InternalErrorReport>>,
  ) {
    match writer.flush() {
      Ok(()) => {
        *is_dirty = false;
        *last_flush = Instant::now();
      }
      Err(e) => report_write_error(error_tx, appender_name, &e, "Flush failed"),
    }
  }

  loop {
    if shutdown.load(Ordering::Relaxed) {
      break;
    }

    match rx.recv_timeout(WRITER_RECV_TIMEOUT) {
      Ok(bytes) => {
        write_one(&mut *writer, &bytes, &mut is_dirty, appender_name, error_tx);
        for _ in 0..WRITER_DRAIN_BATCH_MAX {
          match rx.try_recv() {
            Ok(bytes) => {
              write_one(&mut *writer, &bytes, &mut is_dirty, appender_name, error_tx)
            }
            Err(_) => break,
          }
        }
        if is_dirty && last_flush.elapsed() >= max_flush_interval {
          flush(
            &mut *writer,
            &mut is_dirty,
            &mut last_flush,
            appender_name,
            error_tx,
          );
        }
      }
      Err(RecvErrorTimeout::Timeout) => {
        // Idle: get everything onto disk.
        if is_dirty {
          flush(
            &mut *writer,
            &mut is_dirty,
            &mut last_flush,
            appender_name,
            error_tx,
          );
        }
      }
      Err(RecvErrorTimeout::Disconnected) => break,
    }
  }

  // --- Final Flush on Shutdown ---
  // Drain any messages that arrived just before shutdown, then flush.
  while let Ok(bytes) = rx.try_recv() {
    write_one(&mut *writer, &bytes, &mut is_dirty, appender_name, error_tx);
  }
  if is_dirty {
    flush(
      &mut *writer,
      &mut is_dirty,
      &mut last_flush,
      appender_name,
      error_tx,
    );
  }
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
  use crate::config::processed::{EncoderInternal, LoggerInternal, OverflowPolicy};
  use serial_test::serial;
  use std::{collections::HashMap, fs, path::Path};
  use tempfile::NamedTempFile;
  use tracing_core::metadata::{LevelFilter, Metadata};
  use tracing_core::{callsite, field, subscriber, Kind, Level};

  #[test]
  #[serial]
  fn find_config_file_default() {
    let _ = fs::remove_file(format!(
      "{}.{}",
      DEFAULT_CONFIG_BASE_NAME, DEFAULT_CONFIG_EXTENSION
    ));
    let _ = fs::remove_file(format!(
      "{}.dev.{}",
      DEFAULT_CONFIG_BASE_NAME, DEFAULT_CONFIG_EXTENSION
    ));
    unsafe {
      env::remove_var("FIBRE_ENV");
      env::remove_var("APP_ENV");
    }

    let result = find_config_file(None);
    assert!(matches!(result, Err(Error::ConfigNotFound(_))));
  }

  #[test]
  #[serial]
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
  #[serial]
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

  // ---------------------------------------------------------------------
  // Additivity / dispatch semantics
  // ---------------------------------------------------------------------

  /// Builds an event-stream actor wired the same way init_from_file does.
  fn make_event_actor(
    name: &str,
    loggers: &HashMap<String, LoggerInternal>,
    capacity: usize,
    overflow: OverflowPolicy,
  ) -> (AppenderActor, mpsc::BoundedSyncReceiver<LogEvent>) {
    let (tx, rx) = mpsc::bounded::<LogEvent>(capacity);
    let actor = AppenderActor {
      name: name.to_string(),
      filter: build_filter_for_appender(name, loggers),
      formatter: encoders::new_event_formatter(&EncoderInternal::default()),
      action: ActorAction::SendEvent(tx),
      overflow,
      drops: DropCounter::default(),
    };
    (actor, rx)
  }

  fn matrix_loggers() -> HashMap<String, LoggerInternal> {
    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::INFO,
        appender_names: vec!["app3".to_string()],
        additive: true,
      },
    );
    loggers.insert(
      "a".to_string(),
      LoggerInternal {
        name: "a".to_string(),
        min_level: LevelFilter::DEBUG,
        appender_names: vec!["app1".to_string()],
        additive: false,
      },
    );
    loggers.insert(
      "a::b".to_string(),
      LoggerInternal {
        name: "a::b".to_string(),
        min_level: LevelFilter::DEBUG,
        appender_names: vec!["app2".to_string()],
        additive: true,
      },
    );
    loggers
  }

  fn drain_count(rx: &mpsc::BoundedSyncReceiver<LogEvent>) -> usize {
    let mut n = 0;
    while rx.try_recv().is_ok() {
      n += 1;
    }
    n
  }

  #[test]
  fn additivity_matrix_pins_dispatch_semantics() {
    let loggers = matrix_loggers();
    let (actor1, rx1) = make_event_actor("app1", &loggers, 16, OverflowPolicy::DropNewest);
    let (actor2, rx2) = make_event_actor("app2", &loggers, 16, OverflowPolicy::DropNewest);
    let (actor3, rx3) = make_event_actor("app3", &loggers, 16, OverflowPolicy::DropNewest);
    let processor = EventProcessor::new(vec![actor1, actor2, actor3], None);

    let send = |level: Level, target: &'static str| {
      let md = mock_metadata(level, target);
      processor.process_event(LogEvent::new(level, target, "test", None), &md);
    };

    // Non-additive logger "a" is the most specific match: only its appender
    // receives, even though app3 would match via root.
    send(Level::DEBUG, "a::x");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (1, 0, 0));
    send(Level::INFO, "a::x");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (1, 0, 0));

    // Additive logger "a::b" is the most specific match: every appender
    // evaluates independently. Before the fix, app2 was wrongly suppressed
    // because app1's rule for the same event was non-additive.
    send(Level::INFO, "a::b::x");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (1, 1, 1));

    // DEBUG passes the "a"/"a::b" rules but not root's INFO default.
    send(Level::DEBUG, "a::b::x");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (1, 1, 0));

    // Unmatched target: only root's appender.
    send(Level::INFO, "other");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (0, 0, 1));

    // Boundary check flows through dispatch: "aa" must not match logger "a".
    send(Level::DEBUG, "aa::x");
    assert_eq!((drain_count(&rx1), drain_count(&rx2), drain_count(&rx3)), (0, 0, 0));
  }

  #[test]
  fn processor_fast_path_and_max_level_follow_config() {
    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::WARN,
        appender_names: vec!["app1".to_string()],
        additive: true,
      },
    );
    let (actor, _rx) = make_event_actor("app1", &loggers, 16, OverflowPolicy::DropNewest);
    let processor = EventProcessor::new(vec![actor], None);

    assert_eq!(processor.max_level(), LevelFilter::WARN);
    assert!(processor.event_enabled(&mock_metadata(Level::WARN, "anything")));
    assert!(processor.event_enabled(&mock_metadata(Level::ERROR, "anything")));
    assert!(!processor.event_enabled(&mock_metadata(Level::INFO, "anything")));
    assert!(!processor.event_enabled(&mock_metadata(Level::DEBUG, "anything")));
  }

  #[test]
  fn tracing_filter_maps_to_log_filter() {
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::OFF), LogLevelFilter::Off);
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::ERROR), LogLevelFilter::Error);
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::WARN), LogLevelFilter::Warn);
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::INFO), LogLevelFilter::Info);
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::DEBUG), LogLevelFilter::Debug);
    assert_eq!(tracing_filter_to_log_filter(LevelFilter::TRACE), LogLevelFilter::Trace);
  }

  // ---------------------------------------------------------------------
  // Overflow policies
  // ---------------------------------------------------------------------

  #[test]
  fn overflow_block_blocks_until_consumer_drains() {
    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::INFO,
        appender_names: vec!["app1".to_string()],
        additive: true,
      },
    );
    let (actor, rx) = make_event_actor("app1", &loggers, 1, OverflowPolicy::Block);
    let processor = EventProcessor::new(vec![actor], None);

    let md = mock_metadata(Level::INFO, "t");
    processor.process_event(LogEvent::new(Level::INFO, "t", "e1", None), &md);

    let sender_thread = thread::spawn(move || {
      let md = mock_metadata(Level::INFO, "t");
      processor.process_event(LogEvent::new(Level::INFO, "t", "e2", None), &md);
    });

    thread::sleep(Duration::from_millis(100));
    assert!(
      !sender_thread.is_finished(),
      "block policy must block while the channel is full"
    );

    assert!(rx.try_recv().is_ok(), "first event should be in the channel");
    sender_thread.join().unwrap();
    assert!(rx.try_recv().is_ok(), "second event delivered after drain");
  }

  #[test]
  fn close_channels_disconnects_streams_after_draining_buffered_events() {
    use fibre::TryRecvError;

    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::INFO,
        appender_names: vec!["app1".to_string()],
        additive: true,
      },
    );
    let (actor, rx) = make_event_actor("app1", &loggers, 16, OverflowPolicy::DropNewest);
    let processor = EventProcessor::new(vec![actor], None);

    let md = mock_metadata(Level::INFO, "t");
    processor.process_event(LogEvent::new(Level::INFO, "t", "e1", None), &md);
    processor.process_event(LogEvent::new(Level::INFO, "t", "e2", None), &md);

    processor.close_channels();

    // Buffered events survive the close and are still deliverable...
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    // ...after which the receiver sees a real disconnect, not Empty.
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));

    // Events processed after close are silently discarded, without panicking
    // and without resurrecting the stream.
    processor.process_event(LogEvent::new(Level::INFO, "t", "late", None), &md);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
  }

  #[test]
  fn overflow_drop_newest_never_blocks() {
    let mut loggers = HashMap::new();
    loggers.insert(
      "root".to_string(),
      LoggerInternal {
        name: "root".to_string(),
        min_level: LevelFilter::INFO,
        appender_names: vec!["app1".to_string()],
        additive: true,
      },
    );
    let (actor, rx) = make_event_actor("app1", &loggers, 1, OverflowPolicy::DropNewest);
    let processor = EventProcessor::new(vec![actor], None);

    let md = mock_metadata(Level::INFO, "t");
    for _ in 0..10 {
      processor.process_event(LogEvent::new(Level::INFO, "t", "e", None), &md);
    }
    assert_eq!(drain_count(&rx), 1, "capacity-1 channel keeps exactly one event");
  }

  // ---------------------------------------------------------------------
  // Byte-appender writer loop
  // ---------------------------------------------------------------------

  #[derive(Clone, Default)]
  struct SharedWriter {
    data: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    flushes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
  }

  impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
      self.data.lock().unwrap().extend_from_slice(buf);
      Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
      self
        .flushes
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
      Ok(())
    }
  }

  struct FailingWriter;
  impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
      Err(io::Error::new(io::ErrorKind::Other, "disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
      Err(io::Error::new(io::ErrorKind::Other, "disk full"))
    }
  }

  #[test]
  fn writer_loop_writes_everything_and_flushes_on_shutdown() {
    let (tx, rx) = mpsc::bounded::<Vec<u8>>(1024);
    let shutdown = Arc::new(AtomicBool::new(false));
    let writer = SharedWriter::default();
    let data = Arc::clone(&writer.data);
    let flushes = Arc::clone(&writer.flushes);

    let handle = thread::spawn({
      let shutdown = Arc::clone(&shutdown);
      move || {
        let error_tx: Option<mpsc::BoundedSyncSender<InternalErrorReport>> = None;
        run_byte_appender_writer(
          rx,
          Box::new(writer),
          "test_writer",
          &shutdown,
          &error_tx,
          MAX_FLUSH_INTERVAL,
        )
      }
    });

    for i in 0..500 {
      tx.try_send(format!("line {}\n", i).into_bytes())
        .expect("channel should not fill in this test");
    }
    shutdown.store(true, Ordering::SeqCst);
    handle.join().unwrap();

    let written = String::from_utf8(data.lock().unwrap().clone()).unwrap();
    for i in 0..500 {
      assert!(
        written.contains(&format!("line {}\n", i)),
        "message {} lost (shutdown drain must capture queued messages)",
        i
      );
    }
    assert!(
      flushes.load(std::sync::atomic::Ordering::Relaxed) >= 1,
      "final flush must run on shutdown"
    );
  }

  #[test]
  fn writer_loop_flushes_within_interval_under_sustained_load() {
    let (tx, rx) = mpsc::bounded::<Vec<u8>>(1024);
    let shutdown = Arc::new(AtomicBool::new(false));
    let writer = SharedWriter::default();
    let flushes = Arc::clone(&writer.flushes);

    let handle = thread::spawn({
      let shutdown = Arc::clone(&shutdown);
      move || {
        let error_tx: Option<mpsc::BoundedSyncSender<InternalErrorReport>> = None;
        run_byte_appender_writer(
          rx,
          Box::new(writer),
          "busy_writer",
          &shutdown,
          &error_tx,
          Duration::from_millis(50),
        )
      }
    });

    // Keep the writer continuously busy for ~400ms.
    let feed_until = Instant::now() + Duration::from_millis(400);
    while Instant::now() < feed_until {
      let _ = tx.try_send(b"x".to_vec());
      thread::sleep(Duration::from_millis(2));
    }

    // While still feeding (no shutdown yet), flushes must already have
    // happened via the max-flush-interval path.
    let flushes_before_shutdown = flushes.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
      flushes_before_shutdown >= 1,
      "under sustained load, data must be flushed at least every interval"
    );

    shutdown.store(true, Ordering::SeqCst);
    handle.join().unwrap();
  }

  #[test]
  fn writer_loop_reports_write_failures_to_error_channel() {
    let (tx, rx) = mpsc::bounded::<Vec<u8>>(16);
    let (err_tx, err_rx) = mpsc::bounded::<InternalErrorReport>(16);
    let shutdown = Arc::new(AtomicBool::new(false));

    let handle = thread::spawn({
      let shutdown = Arc::clone(&shutdown);
      move || {
        let error_tx = Some(err_tx);
        run_byte_appender_writer(
          rx,
          Box::new(FailingWriter),
          "bad_writer",
          &shutdown,
          &error_tx,
          MAX_FLUSH_INTERVAL,
        )
      }
    });

    tx.try_send(b"payload".to_vec()).unwrap();

    let report = err_rx
      .recv_timeout(Duration::from_secs(2))
      .expect("write failure must produce an internal error report");
    match &report.source {
      InternalErrorSource::AppenderWrite { appender_name } => {
        assert_eq!(appender_name, "bad_writer");
      }
      other => panic!("expected AppenderWrite source, got {}", other),
    }
    assert!(report.error_message.contains("disk full"));

    shutdown.store(true, Ordering::SeqCst);
    handle.join().unwrap();
  }
}
