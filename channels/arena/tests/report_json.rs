use channels_arena::report::Report;
use serde_json::Value;

fn report() -> Value {
  let tsv = include_str!("../docs/raw/results.tsv");
  let report = Report::from_tsv("Test Machine".to_owned(), tsv).unwrap();
  serde_json::from_str(&report.json()).unwrap()
}

#[test]
fn the_json_carries_the_wording_the_site_shows_for_this_arena() {
  let report = report();
  assert_eq!(report["noun"], "channel");
  assert_eq!(report["peers"], "other Rust channel libraries");
  assert!(report["reading"].as_str().unwrap().contains("no channel for that cell"));
  assert!(report["explore"]["intro"].is_string());
  assert_eq!(report["explore"]["axes"], "shapes, modes, capacities and thread counts");
}

#[test]
fn explore_fixes_pairing_as_rows_with_readable_labels() {
  let report = report();
  let explore = &report["explore"];
  assert_eq!(explore["rows"], "pairing");
  assert_eq!(explore["defaults"]["flavor"], "mpsc");
  assert_eq!(explore["defaults"]["mode"], "async");
  assert_eq!(explore["defaults"]["capacity"], "128");
  assert_eq!(explore["defaults"]["api"], "single");
  assert_eq!(explore["defaults"]["stage"], "stream");

  let pairing = explore["dimensions"].as_array().unwrap().iter().find(|d| d["id"] == "pairing").unwrap();
  assert_eq!(pairing["labels"]["1x1"], "1 producer, 1 consumer");
  assert_eq!(pairing["labels"]["16x1"], "16 producers, 1 consumer");
  assert_eq!(pairing["labels"]["1x64"], "1 producer, 64 consumers");
  assert_eq!(pairing["labels"]["64x64"], "64 producers, 64 consumers");
}
