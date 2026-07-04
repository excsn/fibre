// Only meaningful in release builds (`cargo test --release`), where the
// debug_report module is stubbed out: a config that declares a debug_report
// appender must initialize cleanly with the appender skipped, instead of
// panicking on an unreachable!() arm.
#![cfg(not(debug_assertions))]

#[test]
fn debug_report_appender_is_skipped_in_release_builds() {
  let dir = tempfile::tempdir().unwrap();
  let config_path = dir.path().join("fibre_logging.yaml");
  std::fs::write(
    &config_path,
    r#"
version: 1
appenders:
  report:
    kind: debug_report
loggers:
  root:
    level: info
    appenders: [report]
"#,
  )
  .unwrap();

  let init = fibre_logging::init_from_file(&config_path)
    .expect("release init must not panic on a debug_report appender");
  assert!(
    init.appender_task_handles.is_empty(),
    "skipped appender must not spawn a task"
  );

  // Events route nowhere but must not panic.
  tracing::info!("into the void");
  drop(init);
}
