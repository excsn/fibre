// The debug_report appender only exists in debug builds.
#![cfg(debug_assertions)]

// Full-pipeline test for the debug-report collector. Lives in its own
// integration-test binary because init_from_file installs process-global
// state (tracing subscriber + log handler) and can only run once per process.

use std::time::{Duration, Instant};

#[test]
fn debug_collector_keeps_up_and_shuts_down_promptly() {
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

  let init = fibre_logging::init_from_file(&config_path).unwrap();

  // 2000 events fit inside the collector channel (capacity 2048), so none
  // can be dropped even if the collector momentarily stalls - making the
  // throughput assertion deterministic.
  const N: usize = 2000;
  for _ in 0..N {
    tracing::info!(counter = "throughput", "tick");
  }

  // The old collector loop slept 100ms per event (max 10 events/sec); 2000
  // events would have taken ~200 seconds. Require them all within 5s.
  let deadline = Instant::now() + Duration::from_secs(5);
  loop {
    let (_events, counters) = fibre_logging::debug_report::debug_report_totals();
    if counters >= N {
      break;
    }
    assert!(
      Instant::now() < deadline,
      "collector drained only {}/{} events in 5s",
      counters,
      N
    );
    std::thread::sleep(Duration::from_millis(10));
  }

  // The collector thread is now tracked in appender_task_handles; shutdown
  // must signal and join it promptly instead of leaking it.
  let start = Instant::now();
  drop(init);
  assert!(
    start.elapsed() < Duration::from_secs(5),
    "shutdown hung waiting for the collector thread"
  );
}
