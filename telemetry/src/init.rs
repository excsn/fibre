// Contains the primary public initialization functions for fibre_telemetry.

use crate::{
  config::{
    processed::{process_raw_config, AppenderKindInternal, ConfigInternal, LoggerInternal},
    raw::ConfigRaw,
  },
  encoders,
  error::{Error, Result},
  guards::WorkerGuardCollection,
  subscriber::{
    actor::{ActorAction, AppenderActor, PerAppenderFilter},
    DispatchLayer,
  },
  CustomEventReceiver, InitResult, InternalErrorReport, TelemetryEvent,
};

use std::{
  collections::HashMap,
  env,
  fs::{File as StdFsFile, OpenOptions},
  io::{self, Write},
  path::{Path, PathBuf},
  sync::Arc,
};

use fibre::mpsc;
use tracing_appender::rolling;
use tracing_core::metadata::LevelFilter;
use tracing_subscriber::prelude::*;

const DEFAULT_CONFIG_BASE_NAME: &str = "fibre_telemetry";
const DEFAULT_CONFIG_EXTENSION: &str = "yaml";

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

  tracing_log::LogTracer::init().map_err(|e| Error::LogBridgeInit(e.to_string()))?;
  println!("[fibre_telemetry] tracing-log bridge initialized.");

  let mut guard_collection = WorkerGuardCollection::new();
  let mut actors: Vec<AppenderActor> = Vec::new();
  let mut custom_streams = HashMap::new();

  let (error_tx_channel, error_rx_channel) = if internal_config.error_reporting_enabled {
    println!("[fibre_telemetry] Internal error reporting is ENABLED.");
    let (tx, rx) = mpsc::bounded::<InternalErrorReport>(256);
    (Some(tx), Some(rx))
  } else {
    (None, None)
  };

  for (appender_name, appender_config) in &internal_config.appenders {
    println!("[fibre_telemetry] Setting up appender: {}", appender_name);

    let action = match &appender_config.kind {
      AppenderKindInternal::Console(_console_config) => {
        ActorAction::Write(Arc::new(parking_lot::Mutex::new(Box::new(io::stdout()))))
      }
      AppenderKindInternal::File(file_config) => {
        if let Some(parent_dir) = file_config.path.parent() {
          if !parent_dir.exists() {
            std::fs::create_dir_all(parent_dir).map_err(|e| Error::AppenderSetup {
              appender_name: appender_name.clone(),
              reason: format!("Failed to create directory {:?}: {}", parent_dir, e),
            })?;
          }
        }

        let file_writer = OpenOptions::new()
          .create(true)
          .append(true)
          .open(&file_config.path)
          .map_err(|e| Error::AppenderSetup {
            appender_name: appender_name.clone(),
            reason: format!("Failed to open file {:?}: {}", file_config.path, e),
          })?;

        let (non_blocking_writer, guard) = tracing_appender::non_blocking(file_writer);

        guard_collection.add(guard);
        ActorAction::Write(Arc::new(parking_lot::Mutex::new(Box::new(
          non_blocking_writer,
        ))))
      }
      AppenderKindInternal::RollingFile(rolling_config) => {
        if !rolling_config.directory.exists() {
          std::fs::create_dir_all(&rolling_config.directory).map_err(|e| Error::AppenderSetup {
            appender_name: appender_name.clone(),
            reason: format!(
              "Failed to create directory {:?}: {}",
              rolling_config.directory, e
            ),
          })?;
        }

        let file_appender = rolling::RollingFileAppender::new(
          rolling_config.rotation_policy.clone(),
          &rolling_config.directory,
          &rolling_config.file_name_prefix,
        );

        let (non_blocking_writer, guard) = tracing_appender::non_blocking(file_appender);
        guard_collection.add(guard);
        ActorAction::Write(Arc::new(parking_lot::Mutex::new(Box::new(
          non_blocking_writer,
        ))))
      }

      AppenderKindInternal::Custom(custom_config) => {
        let (tx, rx): (mpsc::BoundedSender<TelemetryEvent>, CustomEventReceiver) =
          mpsc::bounded(custom_config.buffer_size);

        // Store the receiver for the user to retrieve.
        custom_streams.insert(appender_name.clone(), rx);

        // The action is to send to the channel.
        ActorAction::Send(tx)
      }
    };

    let formatter = encoders::new_event_formatter(&appender_config.encoder);
    let filter = build_filter_for_appender(appender_name, &internal_config.loggers);

    actors.push(AppenderActor {
      name: appender_name.clone(),
      filter,
      formatter,
      action,
    });
  }

  let dispatch_layer = DispatchLayer::new(actors, error_tx_channel);
  let subscriber = tracing_subscriber::registry().with(dispatch_layer);

  tracing::subscriber::set_global_default(subscriber)
    .map_err(|e| Error::GlobalSubscriberSet(e.to_string()))?;
  println!("[fibre_telemetry] Global tracing subscriber set.");

  println!("[fibre_telemetry] Initialization complete.");
  Ok(InitResult {
    guard_collection,
    internal_error_rx: error_rx_channel,
    custom_streams,
  })
}

/// Constructs a `PerAppenderFilter` for a specific appender by inspecting all logger configurations.
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

    // CORRECTED: Using `appender_names` field.
    if logger_config
      .appender_names
      .contains(&appender_name_to_build.to_string())
    {
      rules.insert(logger_config.name.clone(), logger_config.min_level);
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

  fn safe_canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|e| {
      eprintln!("Warning: could not canonicalize path {:?}: {}", path, e);
      path.to_path_buf()
    })
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

    // Test filter for the "console" appender
    let console_filter = build_filter_for_appender("console", &loggers);
    assert!(console_filter.enabled(&mock_metadata(Level::WARN, "hyper::client")));
    assert!(!console_filter.enabled(&mock_metadata(Level::INFO, "hyper::client")));
    assert!(console_filter.enabled(&mock_metadata(Level::INFO, "another_crate")));
    assert!(!console_filter.enabled(&mock_metadata(Level::DEBUG, "another_crate")));

    // Test filter for the "file_main" appender
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
      assert!(init_res.guard_collection.is_empty());
    }
  }
}
