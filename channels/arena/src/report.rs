use crate::measure::{Measurement, format_throughput};
use crate::spec::{Api, BatchSupport, Capacity, Cell, Flavor, Mode, Pairing, Stage};

use serde_json::{Value, json};

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

pub struct Record {
  pub library: &'static str,
  pub cell: Cell,
  pub batch_support: BatchSupport,
  pub measurement: Measurement,
}

pub struct Report {
  pub machine: String,
  pub run: RunInfo,
  pub records: Vec<Record>,
}

/// What a run knows about itself beyond the machine label. A report rebuilt
/// from a TSV has none of it unless a previous `results.json` carried it.
#[derive(Default)]
pub struct RunInfo {
  pub measured_at: Option<String>,
  pub power_mode: Option<String>,
  pub load_at_start: Option<f64>,
  pub rustc: Option<String>,
  /// Library name to the version of the crate it was measured at.
  pub versions: BTreeMap<String, String>,
}

type RowKey = (Stage, Capacity, Pairing);
type Rows<'a> = BTreeMap<RowKey, BTreeMap<&'static str, &'a Measurement>>;

struct Table {
  heads: &'static [&'static str],
  libraries: Vec<&'static str>,
  rows: Vec<TableRow>,
}

struct TableRow {
  head: Vec<String>,
  cells: Vec<Option<TableCell>>,
}

struct TableCell {
  median: f64,
  min: f64,
  max: f64,
  gain: Option<f64>,
  best: bool,
}

impl TableCell {
  fn display(&self) -> String {
    match self.gain {
      Some(gain) => format!("{} ({:.1}x)", format_throughput(self.median), gain),
      None => format_throughput(self.median),
    }
  }
}

struct Section {
  id: String,
  title: String,
  mode: Mode,
  batched: bool,
  table: Table,
  notes: Vec<String>,
}

const SUMMARY: &str = "fibre against tokio, crossbeam, crossfire, flume, kanal, async-channel, futures, the `oneshot` crate and std, through one workload driver.";

const METHOD: [&str; 5] = [
  "Each cell moves `u64` items through one channel with P producer threads (or tasks) and C consumers. Producers send an equal share each and drop their handle; consumers receive until the channel disconnects. Item count is calibrated per cell to a wall-clock target.",
  "Timing starts once every worker has reached a barrier, excluding spawn and channel construction. Item accounting is verified per run; a cell whose counts do not balance is dropped. Sample rounds are interleaved across implementations.",
  "Pairings are clamped to what each shape allows, so only MPMC shows all seven. A general-purpose MPMC library is measured under the SPSC, MPSC and MPMC shapes; fibre uses its specialized channel per shape.",
  "Capacities are rendezvous, 1, 128, 1024 and unbounded, with a lower item ceiling on unbounded cells. Batched cells run at capacity 128 and above, for implementations with a batch API.",
  "Oneshot is the exception to all of that: its channel is spent by a single op, so capacity, pairing and batching have one legal value each and the axis that remains is whether channel construction sits inside the timed region. See that page for the two stages.",
];

const REPRODUCIBILITY: &str = "1×1 rows reproduce within a few percent. Rows at 16 and 64 threads oversubscribe a 14-core machine and do not: over four runs of MPSC/sync/cap-128/64×1, fibre held 0.579-0.601 Melem/s, kanal 0.359-0.640, crossbeam 8.43-11.9, with crossbeam's spread inside one run reaching 1.24-18.6.";

fn rows(records: &[Record], flavor: Flavor, mode: Mode, batched: bool) -> Rows<'_> {
  let mut out: Rows = BTreeMap::new();
  for record in records {
    let cell = &record.cell;
    if cell.flavor != flavor || cell.mode != mode {
      continue;
    }
    if (cell.api != Api::Single) != batched {
      continue;
    }
    out
      .entry((cell.stage, cell.capacity, cell.pairing))
      .or_default()
      .insert(record.library, &record.measurement);
  }
  out
}

/// The pages promise that 1×1 rows reproduce within a few percent and caveat
/// the contended rows wholesale. A 1×1 cell whose own samples spread this far
/// breaks that promise, so its median is not a comparable figure.
const SPREAD_LIMIT: f64 = 2.0;

/// A oneshot's capacity and pairing have one legal value each, so the stage is
/// the only thing worth naming on its rows.
fn row_head(flavor: Flavor, key: &RowKey) -> Vec<String> {
  match flavor {
    Flavor::Oneshot => vec![key.0.to_string()],
    _ => vec![key.1.to_string(), key.2.label()],
  }
}

fn markdown_table(table: &Table) -> String {
  let mut out = String::new();
  let _ = write!(out, "|");
  for head in table.heads {
    let _ = write!(out, " {} |", head);
  }
  for lib in &table.libraries {
    let _ = write!(out, " {} |", lib);
  }
  let _ = write!(out, "\n|");
  for _ in table.heads {
    let _ = write!(out, " :--- |");
  }
  for _ in &table.libraries {
    let _ = write!(out, " ---: |");
  }
  out.push('\n');

  for row in &table.rows {
    let _ = write!(out, "|");
    for head in &row.head {
      let _ = write!(out, " {} |", head);
    }
    for cell in &row.cells {
      match cell {
        Some(cell) if cell.best => {
          let _ = write!(out, " **{}** |", cell.display());
        }
        Some(cell) => {
          let _ = write!(out, " {} |", cell.display());
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

fn cell_name(flavor: Flavor, cell: &Cell) -> String {
  match flavor {
    Flavor::Oneshot => cell.stage.to_string(),
    _ => format!("capacity {}", cell.capacity),
  }
}

impl Report {
  fn batch_size(&self) -> Option<usize> {
    self
      .records
      .iter()
      .find_map(|r| match r.cell.api {
        Api::Batch(n) => Some(n),
        Api::Single => None,
      })
  }

  fn libraries_for(&self, flavor: Flavor, mode: Mode, batched: bool) -> Vec<&'static str> {
    let mut libs: Vec<&'static str> = Vec::new();
    for r in &self.records {
      if r.cell.flavor != flavor || r.cell.mode != mode {
        continue;
      }
      if (r.cell.api != Api::Single) != batched {
        continue;
      }
      if !libs.contains(&r.library) {
        libs.push(r.library);
      }
    }
    libs.sort_unstable_by_key(|l| (!l.starts_with("fibre"), *l));
    libs
  }

  fn table(&self, flavor: Flavor, mode: Mode, batched: bool) -> Option<Table> {
    let libraries = self.libraries_for(flavor, mode, batched);
    if libraries.is_empty() {
      return None;
    }
    let rows = rows(&self.records, flavor, mode, batched);
    if rows.is_empty() {
      return None;
    }
    let baseline = batched.then(|| self::rows(&self.records, flavor, mode, false));

    let heads: &'static [&'static str] = match flavor {
      Flavor::Oneshot => &["Stage"],
      _ => &["Capacity", "P×C"],
    };

    let mut table_rows = Vec::new();
    for (key, values) in &rows {
      // Marking a winner needs someone to win against.
      let best = if values.len() > 1 {
        values.values().map(|m| m.median()).fold(f64::NEG_INFINITY, f64::max)
      } else {
        f64::NAN
      };
      let cells = libraries
        .iter()
        .map(|lib| {
          values.get(lib).map(|measurement| {
            let median = measurement.median();
            TableCell {
              median,
              min: measurement.min(),
              max: measurement.max(),
              gain: baseline
                .as_ref()
                .and_then(|b| b.get(key))
                .and_then(|b| b.get(lib))
                .map(|single| single.median())
                .filter(|single| *single > 0.0)
                .map(|single| median / single),
              best: median == best,
            }
          })
        })
        .collect();
      table_rows.push(TableRow {
        head: row_head(flavor, key),
        cells,
      });
    }

    Some(Table {
      heads,
      libraries,
      rows: table_rows,
    })
  }

  fn sections(&self, flavor: Flavor) -> Vec<Section> {
    let mut sections = Vec::new();
    for mode in Mode::ALL {
      if let Some(table) = self.table(flavor, mode, false) {
        sections.push(Section {
          id: mode.to_string(),
          title: mode.to_string(),
          mode,
          batched: false,
          table,
          notes: self.spread_note(flavor, mode, false).into_iter().collect(),
        });
      }
      if let Some(table) = self.table(flavor, mode, true) {
        let title = match self.batch_size() {
          Some(size) => format!("{}, batched ({} items per call)", mode, size),
          None => format!("{}, batched", mode),
        };
        sections.push(Section {
          id: format!("{}-batched", mode),
          title,
          mode,
          batched: true,
          table,
          notes: [
            self.batch_support_note(flavor, mode),
            self.spread_note(flavor, mode, true),
          ]
          .into_iter()
          .flatten()
          .collect(),
        });
      }
    }
    sections
  }

  fn spread_note(&self, flavor: Flavor, mode: Mode, batched: bool) -> Option<String> {
    let mut noted: Vec<String> = Vec::new();
    for r in &self.records {
      if r.cell.flavor != flavor || r.cell.mode != mode {
        continue;
      }
      if (r.cell.api != Api::Single) != batched {
        continue;
      }
      if r.cell.pairing.producers != 1 || r.cell.pairing.consumers != 1 {
        continue;
      }
      let (min, max) = (r.measurement.min(), r.measurement.max());
      if min <= 0.0 || max / min < SPREAD_LIMIT {
        continue;
      }
      noted.push(format!(
        "{} at {} ({:.1}x, {} to {})",
        r.library,
        cell_name(flavor, &r.cell),
        max / min,
        format_throughput(min),
        format_throughput(max),
      ));
    }
    if noted.is_empty() {
      return None;
    }
    Some(format!(
      "Unreliable, sample spread over {:.0}x on a 1×1 cell where the rest of the matrix holds within a few percent: {}. Those medians do not support a comparison.",
      SPREAD_LIMIT,
      noted.join(", ")
    ))
  }

  fn batch_support_note(&self, flavor: Flavor, mode: Mode) -> Option<String> {
    let mut described: Vec<String> = Vec::new();
    for lib in self.libraries_for(flavor, mode, true) {
      if let Some(record) = self
        .records
        .iter()
        .find(|r| r.library == lib && r.cell.flavor == flavor && r.cell.mode == mode)
      {
        described.push(format!("{} {}", lib, record.batch_support.label()));
      }
    }
    if described.is_empty() {
      return None;
    }
    Some(format!("Native batch: {}.", described.join(", ")))
  }

  fn flavor_doc(&self, flavor: Flavor) -> Option<String> {
    let sections = self.sections(flavor);
    if sections.is_empty() {
      return None;
    }
    let any_batched = sections.iter().any(|s| s.batched);

    let mut out = String::new();
    let _ = writeln!(out, "# Channel Arena: {}", flavor.as_str().to_uppercase());
    let _ = writeln!(out, "**Test Machine:** {}", self.machine);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", flavor_blurb(flavor));
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Melem/s per item sent, median of samples. Best per row in bold. `-` means unsupported.{}",
      if any_batched {
        " Bracketed figures in batched tables are the gain over the single-item row above."
      } else {
        ""
      }
    );
    if let Some(caveat) = flavor_caveat(flavor) {
      let _ = writeln!(out);
      let _ = writeln!(out, "{}", caveat);
    }

    for section in sections {
      let _ = writeln!(out);
      let _ = writeln!(out, "## {}", section.title);
      let _ = writeln!(out);
      let _ = write!(out, "{}", markdown_table(&section.table));
      if !section.notes.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", section.notes.join(" "));
      }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "Interpretation: [FINDINGS.md](./FINDINGS.md).");

    Some(out)
  }

  fn index(&self, flavors: &[Flavor]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Channel Arena");
    let _ = writeln!(out, "**Test Machine:** {}", self.machine);
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "{} Re-run with `cargo run --release` in `channels/arena`.",
      SUMMARY
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Results");
    let _ = writeln!(out);
    for flavor in flavors {
      let _ = writeln!(
        out,
        "- [{}](./{}.md) - {}",
        flavor.as_str().to_uppercase(),
        flavor.as_str(),
        flavor_blurb(*flavor)
      );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Interpretation: [FINDINGS.md](./FINDINGS.md).");
    let _ = writeln!(out);
    let _ = writeln!(out, "Raw per-cell data: [raw/results.tsv](./raw/results.tsv).");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Method");
    for paragraph in METHOD {
      let _ = writeln!(out);
      let _ = writeln!(out, "{}", paragraph);
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Reproducibility");
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "{} Per-cell min and max are in [raw/results.tsv](./raw/results.tsv).",
      REPRODUCIBILITY
    );
    out
  }

  fn tsv(&self) -> String {
    let mut out = String::new();
    let _ = writeln!(
      out,
      "library\tflavor\tmode\tcapacity\tproducers\tconsumers\tapi\tbatch_support\titems\tmedian_melem_s\tmin_melem_s\tmax_melem_s\tstage"
    );
    for r in &self.records {
      let _ = writeln!(
        out,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{}",
        r.library,
        r.cell.flavor,
        r.cell.mode,
        r.cell.capacity,
        r.cell.pairing.producers,
        r.cell.pairing.consumers,
        r.cell.api,
        r.batch_support.label(),
        r.measurement.items,
        r.measurement.median(),
        r.measurement.min(),
        r.measurement.max(),
        r.cell.stage,
      );
    }
    out
  }

  /// Rebuilds a report from a previous run's TSV so the pages can be reworded
  /// without re-measuring. Sample detail is not stored, so the reconstructed
  /// measurement carries only the min, median and max the TSV recorded, which
  /// is all the report reads back.
  pub fn from_tsv(machine: String, text: &str) -> Result<Report, String> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate().skip(1) {
      if line.trim().is_empty() {
        continue;
      }
      let f: Vec<&str> = line.split('\t').collect();
      if f.len() < 12 {
        return Err(format!("line {}: expected 12 columns, got {}", index + 1, f.len()));
      }
      let fail = |what: &str| format!("line {}: bad {}", index + 1, what);
      let cell = Cell {
        flavor: Flavor::parse(f[1]).ok_or_else(|| fail("flavor"))?,
        mode: Mode::parse(f[2]).ok_or_else(|| fail("mode"))?,
        capacity: Capacity::parse(f[3]).ok_or_else(|| fail("capacity"))?,
        pairing: Pairing {
          producers: f[4].parse().map_err(|_| fail("producers"))?,
          consumers: f[5].parse().map_err(|_| fail("consumers"))?,
        },
        api: Api::parse(f[6]).ok_or_else(|| fail("api"))?,
        stage: f
          .get(12)
          .map_or(Some(Stage::Stream), |raw| Stage::parse(raw))
          .ok_or_else(|| fail("stage"))?,
      };
      let median: f64 = f[9].parse().map_err(|_| fail("median"))?;
      let min: f64 = f[10].parse().map_err(|_| fail("min"))?;
      let max: f64 = f[11].parse().map_err(|_| fail("max"))?;
      records.push(Record {
        library: known_library(f[0]),
        cell,
        batch_support: BatchSupport::parse(f[7]).ok_or_else(|| fail("batch support"))?,
        measurement: Measurement {
          items: f[8].parse().map_err(|_| fail("items"))?,
          throughputs: vec![min, median, max],
        },
      });
    }
    Ok(Report {
      machine,
      run: RunInfo::default(),
      records,
    })
  }

  pub fn write(&self, dir: &Path) -> io::Result<Vec<String>> {
    fs::create_dir_all(dir)?;
    fs::create_dir_all(dir.join("raw"))?;

    let mut written = Vec::new();
    let mut flavors = Vec::new();
    for flavor in Flavor::ALL {
      if let Some(doc) = self.flavor_doc(flavor) {
        let name = format!("{}.md", flavor.as_str());
        fs::write(dir.join(&name), doc)?;
        written.push(name);
        flavors.push(flavor);
      }
    }

    fs::write(dir.join("README.md"), self.index(&flavors))?;
    written.push("README.md".to_string());
    fs::write(dir.join("raw/results.tsv"), self.tsv())?;
    written.push("raw/results.tsv".to_string());
    fs::write(dir.join("raw/results.json"), self.json())?;
    written.push("raw/results.json".to_string());

    Ok(written)
  }
}

impl Report {
  /// The whole report as data: the tables and caveats the pages show, plus
  /// every record for a reader that wants to slice the matrix itself.
  pub fn json(&self) -> String {
    let groups: Vec<Value> = Flavor::ALL
      .into_iter()
      .filter_map(|flavor| {
        let sections = self.sections(flavor);
        if sections.is_empty() {
          return None;
        }
        Some(json!({
          "id": flavor.as_str(),
          "title": match flavor {
            Flavor::Oneshot => "Oneshot".to_owned(),
            _ => flavor.as_str().to_uppercase(),
          },
          "blurb": flavor_blurb(flavor),
          "caveats": flavor_caveat(flavor).map_or(Vec::new(), |c| c.split("\n\n").collect()),
          "sections": sections.iter().map(section_json).collect::<Vec<_>>(),
        }))
      })
      .collect();

    let mut libraries: Vec<&'static str> = Vec::new();
    for record in &self.records {
      if !libraries.contains(&record.library) {
        libraries.push(record.library);
      }
    }
    libraries.sort_unstable_by_key(|l| (!l.starts_with("fibre"), *l));

    let records: Vec<Value> = self
      .records
      .iter()
      .map(|r| {
        json!({
          "library": r.library,
          "d": [
            r.cell.flavor.as_str(),
            r.cell.mode.as_str(),
            r.cell.capacity.to_string(),
            r.cell.pairing.label(),
            r.cell.api.to_string(),
            r.cell.stage.as_str(),
          ],
          "median": round3(r.measurement.median()),
          "min": round3(r.measurement.min()),
          "max": round3(r.measurement.max()),
        })
      })
      .collect();

    let dimension = |id: &str, label: &str, index: usize| {
      let mut values: Vec<String> = Vec::new();
      for record in &records {
        let value = record["d"][index].as_str().unwrap_or_default().to_string();
        if !values.contains(&value) {
          values.push(value);
        }
      }
      json!({ "id": id, "label": label, "values": values })
    };

    let report = json!({
      "format": 1,
      "id": "channels",
      "title": "Channel Arena",
      "summary": SUMMARY,
      "unit": { "label": "Melem/s", "description": "Million items per second per item sent, median of samples." },
      "run": {
        "machine": self.machine,
        "measured_at": self.run.measured_at,
        "power_mode": self.run.power_mode,
        "load_at_start": self.run.load_at_start,
        "rustc": self.run.rustc,
      },
      "libraries": libraries.iter().map(|name| json!({
        "name": name,
        "crate": library_crate(name),
        "version": self.run.versions.get(*name),
      })).collect::<Vec<_>>(),
      "method": METHOD,
      "caveats": [REPRODUCIBILITY],
      "groups": groups,
      "explore": {
        "dimensions": [
          dimension("flavor", "Shape", 0),
          dimension("mode", "Mode", 1),
          dimension("capacity", "Capacity", 2),
          dimension("pairing", "P×C", 3),
          dimension("api", "API", 4),
          dimension("stage", "Stage", 5),
        ],
        "records": records,
      },
    });
    let mut out = serde_json::to_string_pretty(&report).expect("report serializes");
    out.push('\n');
    out
  }
}

impl RunInfo {
  /// Reads the run fields back from an earlier `results.json`, so a rerender
  /// keeps what the measuring run recorded.
  pub fn from_json(text: &str) -> Option<RunInfo> {
    let value: Value = serde_json::from_str(text).ok()?;
    let run = value.get("run")?;
    let text = |key: &str| run.get(key).and_then(Value::as_str).map(str::to_owned);
    let versions = value
      .get("libraries")
      .and_then(Value::as_array)
      .map(|libs| {
        libs
          .iter()
          .filter_map(|lib| Some((lib.get("name")?.as_str()?.to_owned(), lib.get("version")?.as_str()?.to_owned())))
          .collect()
      })
      .unwrap_or_default();
    Some(RunInfo {
      measured_at: text("measured_at"),
      power_mode: text("power_mode"),
      load_at_start: run.get("load_at_start").and_then(Value::as_f64),
      rustc: text("rustc"),
      versions,
    })
  }
}

fn section_json(section: &Section) -> Value {
  json!({
    "id": section.id,
    "title": section.title,
    "mode": section.mode.as_str(),
    "batched": section.batched,
    "heads": section.table.heads,
    "libraries": section.table.libraries,
    "rows": section.table.rows.iter().map(|row| json!({
      "head": row.head,
      "cells": row.cells.iter().map(|cell| cell.as_ref().map(|c| json!({
        "display": c.display(),
        "median": round3(c.median),
        "min": round3(c.min),
        "max": round3(c.max),
        "best": c.best,
      }))).collect::<Vec<_>>(),
    })).collect::<Vec<_>>(),
    "notes": section.notes,
  })
}

fn round3(value: f64) -> f64 {
  (value * 1000.0).round() / 1000.0
}

/// The crate a library name was measured from; `std` is the toolchain's.
pub fn library_crate(name: &str) -> &'static str {
  match name {
    "fibre" | "fibre-exclusive" | "fibre-pool" | "fibre-pool-host" => "fibre",
    "crossbeam" => "crossbeam-channel",
    "futures" => "futures-channel",
    "std" => "std",
    other => known_library(other),
  }
}

/// Library names are `&'static str` throughout; a name read back from a file
/// has to be matched against the set the registry uses.
fn known_library(name: &str) -> &'static str {
  const LIBRARIES: [&str; 16] = [
    "fibre",
    "crossfire",
    "oneshot",
    "fibre-exclusive",
    "fibre-pool",
    "fibre-pool-host",
    "tokio",
    "crossbeam",
    "flume",
    "kanal",
    "async-channel",
    "std",
    "futures",
    "async-oneshot",
    "lite-sync",
    "sync-oneshot",
  ];
  LIBRARIES
    .into_iter()
    .find(|known| *known == name)
    .unwrap_or_else(|| Box::leak(name.to_string().into_boxed_str()))
}

fn flavor_caveat(flavor: Flavor) -> Option<&'static str> {
  match flavor {
    Flavor::Spmc => Some(
      "Only fibre appears here: no other library in the set has a comparable broadcast channel, and `tokio::sync::broadcast` drops items for lagging consumers rather than applying backpressure. Throughput is per item sent, so a 1×64 row does 64 times the receive work per unit shown.",
    ),
    Flavor::Oneshot => Some(
      "Two stages, because a oneshot's channel is consumed by the op rather than standing across the run. `full-cycle` creates, sends and receives on one thread, so construction is part of the op and allocation shows. `handoff` pre-creates every pair before the barrier and times a sending thread against a receiving one, which is the same accounting the other pages use.\n\nfibre appears four times: `fibre` is the clonable `oneshot()`, `fibre-exclusive` the single-sender `exclusive()`, and `fibre-pool` and `fibre-pool-host` take their slots from a standing pool instead of allocating. kanal, flume, crossbeam, async-channel and std have no oneshot and are absent rather than approximated with a `bounded(1)`, which is a standing channel that happens to hold one item. `futures` and `async-oneshot` have no blocking receive and `sync-oneshot` has no `Future`, so each of those runs one mode only.\n\nHandoff rows carry a lower item ceiling than the rest of the arena, since every op needs its own pre-created channel and the item count is also the resident channel count.",
    ),
    _ => None,
  }
}

fn flavor_blurb(flavor: Flavor) -> &'static str {
  match flavor {
    Flavor::Spsc => "one producer, one consumer",
    Flavor::Mpsc => "many producers, one consumer",
    Flavor::Spmc => "one producer broadcasting to many consumers (every consumer receives every item)",
    Flavor::Mpmc => "many producers, many consumers, work-sharing",
    Flavor::Oneshot => "one value, one time, a fresh channel per op",
  }
}
