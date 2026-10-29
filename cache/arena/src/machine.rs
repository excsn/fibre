//! The machine state a measuring run records beside its numbers.

use crate::report::RunInfo;

use std::collections::BTreeMap;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const LOCKFILE: &str = include_str!("../Cargo.lock");

/// Crates whose locked versions the docs record, by engine name.
const CRATES: [(&str, &str); 5] = [
  ("fibre_cache.s3fifo", "fibre_cache"),
  ("fibre_cache.tinylfu", "fibre_cache"),
  ("moka", "moka"),
  ("quick_cache", "quick_cache"),
  ("stretto", "stretto"),
];

pub fn capture() -> RunInfo {
  RunInfo {
    measured_at: Some(now_utc()),
    power_mode: power_mode(),
    load_at_start: load_average(),
    load_at_end: None,
    versions: versions(),
  }
}

pub fn versions() -> BTreeMap<String, String> {
  let locked = locked_versions();
  let mut out = BTreeMap::new();
  for (engine, krate) in CRATES {
    if let Some(v) = locked.get(krate) {
      out.insert(engine.to_owned(), v.clone());
    }
  }
  out
}

fn locked_versions() -> BTreeMap<String, String> {
  let mut out = BTreeMap::new();
  let mut name: Option<&str> = None;
  for line in LOCKFILE.lines() {
    if let Some(rest) = line.strip_prefix("name = ") {
      name = Some(rest.trim_matches('"'));
    } else if let (Some(n), Some(rest)) = (name, line.strip_prefix("version = ")) {
      out.entry(n.to_owned()).or_insert_with(|| rest.trim_matches('"').to_owned());
      name = None;
    }
  }
  out
}

pub fn power_mode() -> Option<String> {
  let out = Command::new("pmset").arg("-g").output().ok()?;
  let text = String::from_utf8_lossy(&out.stdout);
  let mode = text
    .lines()
    .find(|l| l.trim_start().starts_with("powermode"))?
    .split_whitespace()
    .nth(1)?;
  Some(
    match mode {
      "0" => "automatic",
      "1" => "low power",
      "2" => "high power",
      other => other,
    }
    .to_owned(),
  )
}

pub fn load_average() -> Option<f64> {
  if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
    return text.split_whitespace().next()?.parse().ok();
  }
  let out = Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
  String::from_utf8_lossy(&out.stdout)
    .split_whitespace()
    .find_map(|field| field.parse().ok())
}

fn now_utc() -> String {
  let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
  let (days, rem) = (secs / 86_400, secs % 86_400);
  let (year, month, day) = civil_from_days(days as i64);
  format!(
    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
    year,
    month,
    day,
    rem / 3600,
    rem % 3600 / 60,
    rem % 60
  )
}

/// Howard Hinnant's days-to-civil conversion for the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
  let z = days + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z.rem_euclid(146_097);
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
  let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
  let year = yoe + era * 400 + i64::from(month <= 2);
  (year, month, day)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn civil_dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(19_723), (2024, 1, 1));
  }

  #[test]
  fn versions_cover_every_engine() {
    let v = versions();
    for (engine, _) in CRATES {
      assert!(v.contains_key(engine), "{engine}");
    }
  }
}
