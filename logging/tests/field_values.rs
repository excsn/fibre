//! A field keeps its type: an unsigned number, a 128-bit number and bytes arrive as themselves, not as text.

use std::path::Path;

use fibre_logging::{LogEvent, LogValue};
use tracing_subscriber::layer::SubscriberExt;

const CONFIG: &str = r#"
version: 1
appenders:
  stream:
    kind: custom
    buffer_size: 64
loggers:
  root:
    level: info
    appenders: [stream]
"#;

fn logged(body: impl FnOnce()) -> Vec<LogEvent> {
  let dir = tempfile::tempdir().unwrap();
  let config: &Path = &dir.path().join("fibre_logging.yaml");
  std::fs::write(config, CONFIG).unwrap();
  let (logging, mut init) = fibre_logging::init::layer_from_file(config).unwrap();
  let events = init.custom_streams.remove("stream").unwrap();
  tracing::subscriber::with_default(tracing_subscriber::registry().with(logging), body);
  let mut out = Vec::new();
  while let Ok(event) = events.try_recv() {
    out.push(event);
  }
  out
}

#[test]
fn unsigned_128_bit_and_byte_fields_keep_their_types() {
  let id: u128 = 0x0193_5a2b_7c1d_4e5f_8a9b_0c1d_2e3f_4a5b;
  let bytes: &[u8] = &[1, 2, 255];
  let events = logged(|| tracing::info!(big = u64::MAX, small = 7u64, id, signed = -5i128, huge = i128::MIN, payload = bytes, "typed"));
  let fields = &events[0].fields;
  assert_eq!(fields.get("big"), Some(&LogValue::UInt(u64::MAX)));
  assert_eq!(fields.get("small"), Some(&LogValue::Int(7)), "an unsigned value that fits a signed one stays as before");
  assert_eq!(fields.get("id"), Some(&LogValue::U128(id)));
  assert_eq!(fields.get("signed"), Some(&LogValue::Int(-5)));
  assert_eq!(fields.get("huge"), Some(&LogValue::String(i128::MIN.to_string())), "a signed 128-bit value past i64 has no number variant");
  assert_eq!(fields.get("payload"), Some(&LogValue::Bytes(vec![1, 2, 255])));
}
