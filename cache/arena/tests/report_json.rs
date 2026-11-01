use cache_arena::report::{Pages, RunInfo};
use serde_json::Value;

const HEAD: &str = "engine\top\tmode\tworkers\tcapacity\tsecs\ttotal\thits\thit_ratio\tmops\tpeak_len\tend_len\tsettled_len\n";

fn line(engine: &str, cell: &str, mops: f64, hits_per_thousand: u64) -> String {
  let total = (mops * 10_000_000.0) as u64;
  let hits = total / 1000 * hits_per_thousand;
  let cell = cell.replace(' ', "\t");
  format!("{engine}\t{cell}\t10000\t10.0\t{total}\t{hits}\t0.0000\t{mops:.3}\t9984\t9984\t9984\n")
}

fn report() -> Value {
  let mut tsv = HEAD.to_owned();
  for (engine, cell, mops, hits) in [
    ("fibre_cache.s3fifo", "zipf sync 64", 31.0, 777),
    ("fibre_cache.tinylfu", "zipf sync 64", 24.0, 774),
    ("moka", "zipf sync 64", 1.0, 785),
    ("quick_cache", "zipf sync 64", 21.0, 785),
    ("fibre_cache.s3fifo", "zipf sync 8", 30.0, 777),
    ("quick_cache", "zipf sync 8", 42.0, 785),
    ("fibre_cache.s3fifo", "zipf mixed 64", 26.0, 777),
    ("quick_cache", "zipf mixed 64", 21.0, 785),
    ("fibre_cache.s3fifo", "insert sync 1", 14.0, 0),
    ("moka", "insert sync 1", 3.0, 0),
  ] {
    tsv.push_str(&line(engine, cell, mops, hits));
  }
  let pages = Pages::from_tsv("Test Machine".to_owned(), &tsv).unwrap();
  serde_json::from_str(&pages.json()).unwrap()
}

fn group<'a>(report: &'a Value, id: &str) -> &'a Value {
  report["groups"].as_array().unwrap().iter().find(|g| g["id"] == id).unwrap()
}

fn section<'a>(group: &'a Value, id: &str) -> &'a Value {
  group["sections"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()
}

fn cell<'a>(section: &'a Value, workers: &str, library: &str) -> &'a Value {
  let column = section["libraries"].as_array().unwrap().iter().position(|l| l == library).unwrap();
  let row = section["rows"].as_array().unwrap().iter().find(|r| r["head"][0] == workers).unwrap();
  &row["cells"][column]
}

#[test]
fn the_json_has_every_field_the_site_reads() {
  let report = report();
  for key in ["title", "summary", "unit", "run", "libraries", "method", "caveats", "groups", "explore", "reading", "noun", "peers"] {
    assert!(!report[key].is_null(), "no `{key}`");
  }
  assert_eq!(report["id"], "cache");
  assert_eq!(report["run"]["machine"], "Test Machine");
}

#[test]
fn libraries_name_the_crate_each_engine_comes_from() {
  let report = report();
  let libraries = report["libraries"].as_array().unwrap();
  assert_eq!(libraries[0]["name"], "fibre_cache.s3fifo");
  assert_eq!(libraries[0]["crate"], "fibre_cache");
  assert_eq!(libraries[1]["crate"], "fibre_cache");
  assert!(libraries.iter().any(|l| l["name"] == "moka" && l["crate"] == "moka"));
}

#[test]
fn groups_follow_op_order_with_one_section_per_mode_and_a_detail_section() {
  let report = report();
  let ids: Vec<&str> = report["groups"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap()).collect();
  assert_eq!(ids, ["insert", "zipf"]);

  let zipf = group(&report, "zipf");
  assert_eq!(zipf["title"], "ZIPF");
  let sections: Vec<&str> = zipf["sections"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
  assert_eq!(sections, ["sync", "mixed", "detail"]);
  assert_eq!(section(zipf, "sync")["heads"], serde_json::json!(["Workers"]));
}

#[test]
fn a_section_lists_only_the_engines_measured_in_its_mode() {
  let report = report();
  let mixed = section(group(&report, "zipf"), "mixed");
  assert_eq!(mixed["libraries"], serde_json::json!(["fibre_cache.s3fifo", "quick_cache"]));
}

#[test]
fn the_yardstick_is_never_best_and_is_marked_when_above_the_best_ranked_engine() {
  let report = report();
  let sync = section(group(&report, "zipf"), "sync");

  assert_eq!(cell(sync, "8", "quick_cache")["best"], false);
  assert_eq!(cell(sync, "8", "quick_cache")["above_ranked"], true);
  assert_eq!(cell(sync, "8", "fibre_cache.s3fifo")["best"], false);

  assert_eq!(cell(sync, "64", "quick_cache")["best"], false);
  assert_eq!(cell(sync, "64", "quick_cache")["above_ranked"], false);
  assert_eq!(cell(sync, "64", "fibre_cache.s3fifo")["best"], true);
  assert_eq!(cell(sync, "64", "fibre_cache.s3fifo")["median"], 31.0);
  assert_eq!(cell(sync, "64", "fibre_cache.s3fifo")["display"], "31.00");
}

#[test]
fn the_detail_section_shows_hit_ratio_only_where_the_op_has_one() {
  let report = report();

  let zipf = section(group(&report, "zipf"), "detail");
  assert_eq!(zipf["title"], "Hit ratio and peak entries");
  assert_eq!(zipf["heads"], serde_json::json!(["Mode", "Workers"]));
  let row = &zipf["rows"][0];
  assert_eq!(row["head"], serde_json::json!(["sync", "8"]));
  assert_eq!(row["cells"][0]["display"], "0.777 / 9984");
  assert_eq!(row["cells"][0]["best"], false);

  let insert = section(group(&report, "insert"), "detail");
  assert_eq!(insert["title"], "Peak entries");
  assert_eq!(insert["rows"][0]["cells"][0]["display"], "9984");
}

#[test]
fn explore_fixes_workers_as_rows_and_has_no_spread_for_a_single_run() {
  let report = report();
  let explore = &report["explore"];
  assert_eq!(explore["rows"], "workers");
  assert_eq!(explore["defaults"]["op"], "zipf");
  assert_eq!(explore["defaults"]["mode"], "sync");
  assert!(explore["intro"].is_string());
  assert!(explore["axes"].is_string());

  let dimensions = explore["dimensions"].as_array().unwrap();
  let ids: Vec<&str> = dimensions.iter().map(|d| d["id"].as_str().unwrap()).collect();
  assert_eq!(ids, ["op", "mode", "workers"]);
  assert_eq!(dimensions[0]["values"], serde_json::json!(["insert", "zipf"]));
  assert_eq!(dimensions[1]["values"], serde_json::json!(["sync", "mixed"]));
  assert_eq!(dimensions[2]["values"], serde_json::json!(["1", "8", "64"]));
  assert_eq!(dimensions[2]["labels"]["1"], "1 worker");
  assert_eq!(dimensions[2]["labels"]["64"], "64 workers");

  let records = explore["records"].as_array().unwrap();
  assert_eq!(records.len(), 10);
  let record = &records[0];
  assert_eq!(record["library"], "fibre_cache.s3fifo");
  assert_eq!(record["d"], serde_json::json!(["zipf", "sync", "64"]));
  assert_eq!(record["median"], 31.0);
  assert!(record.get("min").is_none());
  assert!(record.get("max").is_none());
  assert_eq!(record["note"], "0.777 hit ratio");
  assert!(records.iter().filter(|r| r["d"][0] == "insert").all(|r| r.get("note").is_none()));
}

#[test]
fn run_info_survives_a_rerender() {
  let text = r#"{ "run": { "measured_at": "2026-10-31T04:12:05Z", "power_mode": "high power", "load_at_start": 6.18, "load_at_end": 20.08 },
    "libraries": [ { "name": "fibre_cache.s3fifo", "crate": "fibre_cache", "version": "0.4.13" } ] }"#;
  let run = RunInfo::from_json(text).unwrap();
  assert_eq!(run.measured_at.as_deref(), Some("2026-10-31T04:12:05Z"));
  assert_eq!(run.power_mode.as_deref(), Some("high power"));
  assert_eq!(run.load_at_start, Some(6.18));
  assert_eq!(run.load_at_end, Some(20.08));
  assert_eq!(run.versions.get("fibre_cache.s3fifo").map(String::as_str), Some("0.4.13"));
}
