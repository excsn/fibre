// Full-pipeline test for custom-stream shutdown signaling. Lives in its own
// integration-test binary because init_from_file installs process-global
// state (tracing subscriber + log handler) and can only run once per process.

use std::time::Duration;

#[test]
fn custom_stream_receiver_sees_disconnect_on_shutdown() {
  let dir = tempfile::tempdir().unwrap();
  let config_path = dir.path().join("fibre_logging.yaml");
  std::fs::write(
    &config_path,
    r#"
version: 1
appenders:
  stream:
    kind: custom
    buffer_size: 64
loggers:
  root:
    level: info
    appenders: [stream]
"#,
  )
  .unwrap();

  let mut init = fibre_logging::init_from_file(&config_path).unwrap();
  let rx = init
    .custom_streams
    .remove("stream")
    .expect("custom appender must expose a stream");

  // Events are enqueued synchronously by the emitting thread.
  tracing::info!("first");
  tracing::info!("second");

  init.shutdown(Duration::from_secs(5));

  // Buffered events must still be deliverable after shutdown, after which
  // the receiver observes a real disconnect - previously it saw Empty
  // forever, indistinguishable from a quiet system.
  let mut received = 0;
  loop {
    match rx.try_recv() {
      Ok(event) => {
        assert_eq!(event.target, module_path!());
        received += 1;
      }
      Err(fibre::TryRecvError::Disconnected) => break,
      Err(fibre::TryRecvError::Empty) => {
        panic!("receiver must see Disconnected after shutdown, not Empty")
      }
    }
  }
  assert_eq!(received, 2);

  // Events emitted after shutdown are silently discarded; the stream stays
  // disconnected and nothing panics or blocks.
  tracing::info!("too late");
  assert!(matches!(
    rx.try_recv(),
    Err(fibre::TryRecvError::Disconnected)
  ));
}
