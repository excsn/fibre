//! A failed `init_from_file` leaves nothing global behind. Its own test binary, since it sets process globals.

struct Foreign;

impl log::Log for Foreign {
  fn enabled(&self, _: &log::Metadata<'_>) -> bool {
    false
  }
  fn log(&self, _: &log::Record<'_>) {}
  fn flush(&self) {}
}

#[test]
fn init_that_cannot_bridge_log_sets_no_global_subscriber() {
  log::set_boxed_logger(Box::new(Foreign)).unwrap();
  let dir = tempfile::tempdir().unwrap();
  let config = dir.path().join("fibre_logging.yaml");
  std::fs::write(&config, "version: 1\nappenders:\n  stream:\n    kind: custom\n    buffer_size: 8\nloggers:\n  root:\n    level: info\n    appenders: [stream]\n").unwrap();
  assert!(fibre_logging::init::init_from_file(&config).is_err(), "another log logger is already installed");
  assert!(!tracing::dispatcher::has_been_set(), "the global subscriber is set only when init succeeds");
}
