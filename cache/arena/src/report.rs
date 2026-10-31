//! The pages a run writes: one per op, an index, a raw TSV and a JSON copy of
//! everything. `FINDINGS.md` beside them is written by hand.

use crate::engines::DOCS;
use crate::harness::{Cell, Mode, Op, Report, zipf_ceiling};

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

pub struct Record {
  pub engine: String,
  pub cell: Cell,
  pub capacity: u64,
  pub secs: f64,
  pub report: Report,
}

/// What a run knows about itself beyond the machine label.
#[derive(Default, Clone)]
pub struct RunInfo {
  pub measured_at: Option<String>,
  pub power_mode: Option<String>,
  pub load_at_start: Option<f64>,
  pub load_at_end: Option<f64>,
  /// Engine name to the version of the crate it was measured at.
  pub versions: BTreeMap<String, String>,
}

pub struct Pages {
  pub machine: String,
  pub run: RunInfo,
  pub records: Vec<Record>,
}

const SUMMARY: &str = "fibre_cache against moka, quick_cache and stretto through one fixed-duration workload harness.";

const METHOD: [&str; 4] = [
  "Each cell runs every engine in turn for a fixed number of seconds and reports how many operations its workers completed. Workers stay alive for the whole run, keys are shuffled and dealt round-robin and each worker checks the stop flag every 256 operations. Sync workers are OS threads; async workers are tasks on a multi-thread tokio runtime with one worker thread per task; mixed puts half the workers on each side of the same instance.",
  "Keys and values are `u64` and every entry costs 1, so the capacity is a number of entries. `hit`, `hot` and `insert` fill the cache to capacity before timing starts. `hit` reads resident keys, `hot` has every worker read the key filled last, `insert` adds keys outside the filled range and `zipf` draws keys from a Zipf distribution (exponent 0.99) over ten times the capacity starting from an empty cache, inserting on a miss.",
  "A sampler thread reads the entry count every millisecond; the peak it saw is reported against the capacity, which is how an implementation's capacity bound is checked under load.",
  "Timing starts once every worker has reached a barrier. The power mode and load average are recorded at the start and the end of the run.",
];

fn op_name(op: Op) -> &'static str {
  match op {
    Op::Hit => "hit",
    Op::Hot => "hot",
    Op::Insert => "insert",
    Op::Zipf => "zipf",
  }
}

fn op_blurb(op: Op) -> &'static str {
  match op {
    Op::Hit => "every operation reads a resident key",
    Op::Hot => "every worker reads the same resident key",
    Op::Insert => "every operation inserts a key not yet seen, into a full cache",
    Op::Zipf => "Zipf-distributed reads over ten times the capacity, inserting on a miss",
  }
}

/// The expected hit ratio and peak for an op, and where they turn abnormal.
fn expected(op: Op, capacity: Option<u64>) -> String {
  let cap = capacity.map_or("capacity".to_owned(), |c| c.to_string());
  match op {
    Op::Hit => format!("Hit ratio 1.0, or 0.95 and up for a cache with per-shard budgets. Peak ≤ {cap}.\n"),
    Op::Hot => format!("Hit ratio exactly 1.0. Peak ≤ {cap}.\n"),
    Op::Insert => format!("No gets, so no hit ratio. Peak ≤ {cap}.\n"),
    Op::Zipf => match capacity {
      Some(c) => {
        let ceiling = zipf_ceiling(c);
        format!(
          "| | Hit ratio | Peak |\n| :--- | :--- | :--- |\n| Ceiling | {ceiling:.3} (the {cap} most frequent keys always resident) | {cap} |\n| Expected | {:.2} to {ceiling:.3} | ≤ {cap} |\n| Abnormal | above {ceiling:.3} or below {:.2} | above {cap} |\n",
          ceiling - 0.03,
          ceiling - 0.03
        )
      }
      None => "Hit ratio near the ceiling for the capacity. Peak ≤ capacity.\n".to_owned(),
    },
  }
}

fn mode_name(mode: Mode) -> &'static str {
  match mode {
    Mode::Sync => "sync",
    Mode::Async => "async",
    Mode::Mixed => "mixed",
  }
}

const OPS: [Op; 4] = [Op::Hit, Op::Hot, Op::Insert, Op::Zipf];
const MODES: [Mode; 3] = [Mode::Sync, Mode::Async, Mode::Mixed];

fn mode_rank(mode: Mode) -> usize {
  MODES.iter().position(|m| *m == mode).unwrap_or(0)
}

fn fmt3(v: f64) -> String {
  format!("{:.2}", v)
}

/// Shown in every table but never ranked: it has no expiry and no cache-level loader.
const YARDSTICK: &str = "quick_cache";

impl Pages {
  fn engines(&self) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in &self.records {
      if !out.contains(&r.engine) {
        out.push(r.engine.clone());
      }
    }
    out.sort_by_key(|e| (!e.starts_with("fibre"), e.clone()));
    out
  }

  fn engines_for(&self, op: Op) -> Vec<String> {
    self
      .engines()
      .into_iter()
      .filter(|e| self.records.iter().any(|r| r.engine == *e && r.cell.op == op))
      .collect()
  }

  fn rows(&self, op: Op) -> Vec<(Mode, usize)> {
    let mut rows: Vec<(Mode, usize)> = Vec::new();
    for r in &self.records {
      if r.cell.op == op && !rows.contains(&(r.cell.mode, r.cell.workers)) {
        rows.push((r.cell.mode, r.cell.workers));
      }
    }
    rows.sort_by_key(|(m, w)| (mode_rank(*m), *w));
    rows
  }

  fn record(&self, engine: &str, op: Op, mode: Mode, workers: usize) -> Option<&Record> {
    self
      .records
      .iter()
      .find(|r| r.engine == engine && r.cell.op == op && r.cell.mode == mode && r.cell.workers == workers)
  }

  fn capacity(&self) -> Option<u64> {
    self.records.first().map(|r| r.capacity)
  }

  fn secs(&self) -> Option<f64> {
    self.records.first().map(|r| r.secs)
  }

  /// Mops/s table with the best engine per row in bold, leaving out the yardstick.
  /// The yardstick is in italics where it is above that best.
  fn throughput_table(&self, op: Op) -> String {
    let engines = self.engines_for(op);
    let mut out = String::new();
    let _ = write!(out, "| Mode | Workers |");
    for e in &engines {
      let _ = write!(out, " {} |", e);
    }
    let _ = write!(out, "\n| :--- | :--- |");
    for _ in &engines {
      let _ = write!(out, " ---: |");
    }
    out.push('\n');
    for (mode, workers) in self.rows(op) {
      let values: Vec<Option<f64>> = engines
        .iter()
        .map(|e| self.record(e, op, mode, workers).map(|r| r.report.mops()))
        .collect();
      let ranked: Vec<f64> = engines
        .iter()
        .zip(&values)
        .filter(|(e, _)| e.as_str() != YARDSTICK)
        .filter_map(|(_, v)| *v)
        .collect();
      let present = ranked.len();
      let best = ranked.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b));
      let _ = write!(out, "| {} | {} |", mode_name(mode), workers);
      for (e, v) in engines.iter().zip(values) {
        match v {
          Some(v) if present > 1 && v == best && e.as_str() != YARDSTICK => {
            let _ = write!(out, " **{}** |", fmt3(v));
          }
          Some(v) if present > 0 && v > best && e.as_str() == YARDSTICK => {
            let _ = write!(out, " *{}* |", fmt3(v));
          }
          Some(v) => {
            let _ = write!(out, " {} |", fmt3(v));
          }
          None => {
            let _ = write!(out, " - |");
          }
        }
      }
      out.push('\n');
    }
    out
  }

  /// Hit ratio and peak entry count per cell, the fidelity side of each number.
  fn detail_table(&self, op: Op) -> String {
    let engines = self.engines_for(op);
    let with_ratio = self.records.iter().any(|r| r.cell.op == op && r.report.hits > 0 && r.report.hits < r.report.total);
    let mut out = String::new();
    let _ = write!(out, "| Mode | Workers |");
    for e in &engines {
      let _ = write!(out, " {} |", e);
    }
    let _ = write!(out, "\n| :--- | :--- |");
    for _ in &engines {
      let _ = write!(out, " ---: |");
    }
    out.push('\n');
    for (mode, workers) in self.rows(op) {
      let _ = write!(out, "| {} | {} |", mode_name(mode), workers);
      for e in &engines {
        match self.record(e, op, mode, workers) {
          Some(r) if with_ratio => {
            let _ = write!(out, " {:.3} / {} |", r.report.hit_ratio(), r.report.peak_len);
          }
          Some(r) => {
            let _ = write!(out, " {} |", r.report.peak_len);
          }
          None => {
            let _ = write!(out, " - |");
          }
        }
      }
      out.push('\n');
    }
    (out, with_ratio).0
  }

  fn op_doc(&self, op: Op) -> Option<String> {
    if self.engines_for(op).is_empty() {
      return None;
    }
    let with_ratio = self.records.iter().any(|r| r.cell.op == op && r.report.hits > 0 && r.report.hits < r.report.total);
    let mut out = String::new();
    let _ = writeln!(out, "# Cache Arena: {}", op_name(op).to_uppercase());
    let _ = writeln!(out, "**Test Machine:** {}", self.machine);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}.", op_blurb(op));
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Mops/s: operations completed per second across all workers over a {} s run at capacity {}. Best per row in bold, leaving out quick_cache, which is a yardstick and is not ranked. Italics mean quick_cache is above the best ranked engine. `-` means the engine has no API for that mode.",
      self.secs().map_or("?".to_owned(), |s| format!("{:.0}", s)),
      self.capacity().map_or("?".to_owned(), |c| c.to_string())
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Throughput");
    let _ = writeln!(out);
    let _ = write!(out, "{}", self.throughput_table(op));
    let _ = writeln!(out);
    if with_ratio {
      let _ = writeln!(out, "## Hit ratio and peak entries");
      let _ = writeln!(out);
      let _ = writeln!(out, "Hit ratio over the run, then the highest entry count the sampler saw against the capacity.");
    } else {
      let _ = writeln!(out, "## Peak entries");
      let _ = writeln!(out);
      let _ = writeln!(out, "The highest entry count the sampler saw during the run, against the capacity.");
    }
    let _ = writeln!(out);
    let _ = write!(out, "{}", self.detail_table(op));
    let _ = writeln!(out);
    let _ = writeln!(out, "## Expected");
    let _ = writeln!(out);
    let _ = write!(out, "{}", expected(op, self.capacity()));
    let _ = writeln!(out);
    let _ = writeln!(out, "Interpretation: [FINDINGS.md](./FINDINGS.md).");
    Some(out)
  }

  fn index(&self, ops: &[Op]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Cache Arena");
    let _ = writeln!(out, "**Test Machine:** {}", self.machine);
    let _ = writeln!(out);
    let _ = writeln!(out, "{} Re-run with `cargo run --release` in `cache/arena`.", SUMMARY);
    let _ = writeln!(out);
    let _ = writeln!(out, "## Results");
    let _ = writeln!(out);
    for op in ops {
      let _ = writeln!(out, "- [{}](./{}.md) - {}", op_name(*op).to_uppercase(), op_name(*op), op_blurb(*op));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Interpretation: [FINDINGS.md](./FINDINGS.md).");
    let _ = writeln!(out);
    let _ = writeln!(out, "Raw per-cell data: [raw/results.tsv](./raw/results.tsv).");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Run");
    let _ = writeln!(out);
    if let Some(t) = &self.run.measured_at {
      let _ = writeln!(out, "- Measured at {}.", t);
    }
    if let Some(p) = &self.run.power_mode {
      let _ = writeln!(out, "- Power mode: {}.", p);
    }
    match (self.run.load_at_start, self.run.load_at_end) {
      (Some(a), Some(b)) => {
        let _ = writeln!(out, "- Load average {:.2} at the start, {:.2} at the end (the end figure is the run's own workers).", a, b);
      }
      (Some(a), None) => {
        let _ = writeln!(out, "- Load average {:.2} at the start.", a);
      }
      _ => {}
    }
    if !self.run.versions.is_empty() {
      let versions: Vec<String> = self.run.versions.iter().map(|(e, v)| format!("{} {}", e, v)).collect();
      let _ = writeln!(out, "- Versions: {}.", versions.join(", "));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Engines");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Engine | Eviction policy | Built with |");
    let _ = writeln!(out, "| :--- | :--- | :--- |");
    for name in self.engines() {
      match DOCS.iter().find(|d| d.name == name) {
        Some(d) => {
          let _ = writeln!(out, "| `{}` | {} | {} |", d.name, d.policy, d.built_with);
        }
        None => {
          let _ = writeln!(out, "| `{}` | - | - |", name);
        }
      }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Method");
    for paragraph in METHOD {
      let _ = writeln!(out);
      let _ = writeln!(out, "{}", paragraph);
    }
    out
  }

  fn tsv(&self) -> String {
    let mut out = String::new();
    let _ = writeln!(
      out,
      "engine\top\tmode\tworkers\tcapacity\tsecs\ttotal\thits\thit_ratio\tmops\tpeak_len\tend_len\tsettled_len"
    );
    for r in &self.records {
      let _ = writeln!(
        out,
        "{}\t{}\t{}\t{}\t{}\t{:.1}\t{}\t{}\t{:.4}\t{:.3}\t{}\t{}\t{}",
        r.engine,
        op_name(r.cell.op),
        mode_name(r.cell.mode),
        r.cell.workers,
        r.capacity,
        r.secs,
        r.report.total,
        r.report.hits,
        r.report.hit_ratio(),
        r.report.mops(),
        r.report.peak_len,
        r.report.end_len,
        r.report.settled_len,
      );
    }
    out
  }

  fn json(&self) -> String {
    fn esc(s: &str) -> String {
      s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    fn opt_str(v: &Option<String>) -> String {
      v.as_ref().map_or("null".to_owned(), |s| format!("\"{}\"", esc(s)))
    }
    fn opt_f64(v: Option<f64>) -> String {
      v.map_or("null".to_owned(), |f| format!("{:.2}", f))
    }
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"format\": 1,");
    let _ = writeln!(out, "  \"id\": \"cache\",");
    let _ = writeln!(out, "  \"title\": \"Cache Arena\",");
    let _ = writeln!(out, "  \"summary\": \"{}\",", esc(SUMMARY));
    let _ = writeln!(
      out,
      "  \"unit\": {{ \"label\": \"Mops/s\", \"description\": \"Operations completed per second across all workers over a fixed-duration run.\" }},"
    );
    let _ = writeln!(out, "  \"run\": {{");
    let _ = writeln!(out, "    \"machine\": \"{}\",", esc(&self.machine));
    let _ = writeln!(out, "    \"measured_at\": {},", opt_str(&self.run.measured_at));
    let _ = writeln!(out, "    \"power_mode\": {},", opt_str(&self.run.power_mode));
    let _ = writeln!(out, "    \"load_at_start\": {},", opt_f64(self.run.load_at_start));
    let _ = writeln!(out, "    \"load_at_end\": {},", opt_f64(self.run.load_at_end));
    let _ = writeln!(out, "    \"capacity\": {},", self.capacity().map_or("null".to_owned(), |c| c.to_string()));
    let _ = writeln!(out, "    \"secs\": {}", opt_f64(self.secs()));
    let _ = writeln!(out, "  }},");
    let _ = writeln!(out, "  \"engines\": [");
    let engines = self.engines();
    for (i, e) in engines.iter().enumerate() {
      let version = self.run.versions.get(e).map_or("null".to_owned(), |v| format!("\"{}\"", esc(v)));
      let _ = writeln!(
        out,
        "    {{ \"name\": \"{}\", \"version\": {} }}{}",
        esc(e),
        version,
        if i + 1 < engines.len() { "," } else { "" }
      );
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"method\": [");
    for (i, m) in METHOD.iter().enumerate() {
      let _ = writeln!(out, "    \"{}\"{}", esc(m), if i + 1 < METHOD.len() { "," } else { "" });
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"records\": [");
    for (i, r) in self.records.iter().enumerate() {
      let _ = writeln!(
        out,
        "    {{ \"engine\": \"{}\", \"op\": \"{}\", \"mode\": \"{}\", \"workers\": {}, \"capacity\": {}, \"secs\": {:.1}, \"total\": {}, \"hits\": {}, \"hit_ratio\": {:.4}, \"mops\": {:.3}, \"peak_len\": {}, \"end_len\": {}, \"settled_len\": {} }}{}",
        esc(&r.engine),
        op_name(r.cell.op),
        mode_name(r.cell.mode),
        r.cell.workers,
        r.capacity,
        r.secs,
        r.report.total,
        r.report.hits,
        r.report.hit_ratio(),
        r.report.mops(),
        r.report.peak_len,
        r.report.end_len,
        r.report.settled_len,
        if i + 1 < self.records.len() { "," } else { "" }
      );
    }
    let _ = writeln!(out, "  ]");
    let _ = writeln!(out, "}}");
    out
  }

  /// Rebuilds the pages from a previous run's TSV so they can be reworded
  /// without re-measuring.
  pub fn from_tsv(machine: String, text: &str) -> Result<Pages, String> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate().skip(1) {
      if line.trim().is_empty() {
        continue;
      }
      let f: Vec<&str> = line.split('\t').collect();
      if f.len() < 13 {
        return Err(format!("line {}: expected 13 columns, got {}", index + 1, f.len()));
      }
      let fail = |what: &str| format!("line {}: bad {}", index + 1, what);
      let cell = Cell::parse(&format!("{} {} {}", f[1], f[2], f[3])).map_err(|_| fail("cell"))?;
      let total: u64 = f[6].parse().map_err(|_| fail("total"))?;
      let mops: f64 = f[9].parse().map_err(|_| fail("mops"))?;
      let secs: f64 = f[5].parse().map_err(|_| fail("secs"))?;
      let elapsed = if mops > 0.0 { total as f64 / (mops * 1e6) } else { secs };
      records.push(Record {
        engine: f[0].to_owned(),
        cell,
        capacity: f[4].parse().map_err(|_| fail("capacity"))?,
        secs,
        report: Report {
          total,
          hits: f[7].parse().map_err(|_| fail("hits"))?,
          elapsed,
          peak_len: f[10].parse().map_err(|_| fail("peak_len"))?,
          end_len: f[11].parse().map_err(|_| fail("end_len"))?,
          settled_len: f[12].parse().map_err(|_| fail("settled_len"))?,
        },
      });
    }
    Ok(Pages { machine, run: RunInfo::default(), records })
  }

  pub fn write(&self, dir: &Path) -> io::Result<Vec<String>> {
    fs::create_dir_all(dir.join("raw"))?;
    let mut written = Vec::new();
    let mut ops = Vec::new();
    for op in OPS {
      if let Some(doc) = self.op_doc(op) {
        let name = format!("{}.md", op_name(op));
        fs::write(dir.join(&name), doc)?;
        written.push(name);
        ops.push(op);
      }
    }
    fs::write(dir.join("README.md"), self.index(&ops))?;
    written.push("README.md".to_owned());
    fs::write(dir.join("raw/results.tsv"), self.tsv())?;
    written.push("raw/results.tsv".to_owned());
    fs::write(dir.join("raw/results.json"), self.json())?;
    written.push("raw/results.json".to_owned());
    Ok(written)
  }
}

impl RunInfo {
  /// Reads the run fields back from an earlier `results.json`, so a rerender
  /// keeps what the measuring run recorded.
  pub fn from_json(text: &str) -> Option<RunInfo> {
    fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
      let start = text.find(&format!("\"{}\": ", key))? + key.len() + 4;
      let rest = &text[start..];
      let end = rest.find([',', '\n', '}']).unwrap_or(rest.len());
      Some(rest[..end].trim())
    }
    fn string(text: &str, key: &str) -> Option<String> {
      let v = field(text, key)?;
      (v != "null").then(|| v.trim_matches('"').to_owned())
    }
    fn number(text: &str, key: &str) -> Option<f64> {
      field(text, key)?.parse().ok()
    }
    let run_start = text.find("\"run\": {")?;
    let run = &text[run_start..];
    let mut versions = BTreeMap::new();
    for line in text.lines() {
      let t = line.trim();
      if let Some(rest) = t.strip_prefix("{ \"name\": \"") {
        let name_end = rest.find('"')?;
        let name = &rest[..name_end];
        if let Some(v) = rest.find("\"version\": \"") {
          let v = &rest[v + 12..];
          let v_end = v.find('"')?;
          versions.insert(name.to_owned(), v[..v_end].to_owned());
        }
      }
    }
    Some(RunInfo {
      measured_at: string(run, "measured_at"),
      power_mode: string(run, "power_mode"),
      load_at_start: number(run, "load_at_start"),
      load_at_end: number(run, "load_at_end"),
      versions,
    })
  }
}
