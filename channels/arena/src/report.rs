use crate::measure::{Measurement, format_throughput};
use crate::spec::{Api, BatchSupport, Capacity, Cell, Flavor, Mode, Pairing, Stage};

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
  pub records: Vec<Record>,
}

type RowKey = (Stage, Capacity, Pairing);
type Rows = BTreeMap<RowKey, BTreeMap<&'static str, f64>>;

fn rows(records: &[Record], flavor: Flavor, mode: Mode, batched: bool) -> Rows {
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
      .insert(record.library, record.measurement.median());
  }
  out
}

/// The pages promise that 1×1 rows reproduce within a few percent and caveat
/// the contended rows wholesale. A 1×1 cell whose own samples spread this far
/// breaks that promise, so its median is not a comparable figure.
const SPREAD_LIMIT: f64 = 2.0;

/// A oneshot's capacity and pairing have one legal value each, so the stage is
/// the only thing worth naming on its rows.
fn row_head(flavor: Flavor, key: &RowKey) -> String {
  match flavor {
    Flavor::Oneshot => format!("| {} |", key.0),
    _ => format!("| {} | {} |", key.1, key.2.label()),
  }
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

  fn table(&self, flavor: Flavor, mode: Mode, batched: bool) -> Option<String> {
    let libs = self.libraries_for(flavor, mode, batched);
    if libs.is_empty() {
      return None;
    }
    let rows = rows(&self.records, flavor, mode, batched);
    if rows.is_empty() {
      return None;
    }
    let baseline = batched.then(|| self::rows(&self.records, flavor, mode, false));

    let heads: &[&str] = match flavor {
      Flavor::Oneshot => &["Stage"],
      _ => &["Capacity", "P×C"],
    };

    let mut out = String::new();
    let _ = write!(out, "|");
    for head in heads {
      let _ = write!(out, " {} |", head);
    }
    for lib in &libs {
      let _ = write!(out, " {} |", lib);
    }
    let _ = write!(out, "\n|");
    for _ in heads {
      let _ = write!(out, " :--- |");
    }
    for _ in &libs {
      let _ = write!(out, " ---: |");
    }
    out.push('\n');

    for (key, values) in &rows {
      // Marking a winner needs someone to win against.
      let best = if values.len() > 1 {
        values.values().cloned().fold(f64::NEG_INFINITY, f64::max)
      } else {
        f64::NAN
      };
      let _ = write!(out, "{}", row_head(flavor, key));
      for lib in &libs {
        match values.get(lib) {
          Some(value) => {
            let speedup = baseline
              .as_ref()
              .and_then(|b| b.get(key))
              .and_then(|b| b.get(lib))
              .filter(|single| **single > 0.0)
              .map(|single| format!(" ({:.1}x)", value / single))
              .unwrap_or_default();
            let rendered = format!("{}{}", format_throughput(*value), speedup);
            if *value == best {
              let _ = write!(out, " **{}** |", rendered);
            } else {
              let _ = write!(out, " {} |", rendered);
            }
          }
          None => {
            let _ = write!(out, " - |");
          }
        }
      }
      out.push('\n');
    }

    Some(out)
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
    let mut sections: Vec<(String, String, Option<String>)> = Vec::new();
    let mut any_batched = false;
    for mode in Mode::ALL {
      if let Some(table) = self.table(flavor, mode, false) {
        sections.push((mode.to_string(), table, self.spread_note(flavor, mode, false)));
      }
      if let Some(table) = self.table(flavor, mode, true) {
        any_batched = true;
        let title = match self.batch_size() {
          Some(size) => format!("{}, batched ({} items per call)", mode, size),
          None => format!("{}, batched", mode),
        };
        let notes: Vec<String> = [
          self.batch_support_note(flavor, mode),
          self.spread_note(flavor, mode, true),
        ]
        .into_iter()
        .flatten()
        .collect();
        sections.push((title, table, (!notes.is_empty()).then(|| notes.join(" "))));
      }
    }
    if sections.is_empty() {
      return None;
    }

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

    for (title, table, note) in sections {
      let _ = writeln!(out);
      let _ = writeln!(out, "## {}", title);
      let _ = writeln!(out);
      let _ = write!(out, "{}", table);
      if let Some(note) = note {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", note);
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
      "fibre against tokio, crossbeam, flume, kanal, async-channel, futures, the `oneshot` crate and std, through one workload driver. Re-run with `cargo run --release` in `channels/arena`."
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
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Each cell moves `u64` items through one channel with P producer threads (or tasks) and C consumers. Producers send an equal share each and drop their handle; consumers receive until the channel disconnects. Item count is calibrated per cell to a wall-clock target."
    );
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Timing starts once every worker has reached a barrier, excluding spawn and channel construction. Item accounting is verified per run; a cell whose counts do not balance is dropped. Sample rounds are interleaved across implementations."
    );
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Pairings are clamped to what each shape allows, so only MPMC shows all seven. A general-purpose MPMC library is measured under the SPSC, MPSC and MPMC shapes; fibre uses its specialized channel per shape."
    );
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Capacities are rendezvous, 1, 128, 1024 and unbounded, with a lower item ceiling on unbounded cells. Batched cells run at capacity 128 and above, for implementations with a batch API."
    );
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "Oneshot is the exception to all of that: its channel is spent by a single op, so capacity, pairing and batching have one legal value each and the axis that remains is whether channel construction sits inside the timed region. See that page for the two stages."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Reproducibility");
    let _ = writeln!(out);
    let _ = writeln!(
      out,
      "1×1 rows reproduce within a few percent. Rows at 16 and 64 threads oversubscribe a 14-core machine and do not: over four runs of MPSC/sync/cap-128/64×1, fibre held 0.579-0.601 Melem/s, kanal 0.359-0.640, crossbeam 8.43-11.9, with crossbeam's spread inside one run reaching 1.24-18.6. Per-cell min and max are in [raw/results.tsv](./raw/results.tsv)."
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
    Ok(Report { machine, records })
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

    Ok(written)
  }
}

/// Library names are `&'static str` throughout; a name read back from a file
/// has to be matched against the set the registry uses.
fn known_library(name: &str) -> &'static str {
  const LIBRARIES: [&str; 14] = [
    "fibre",
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
