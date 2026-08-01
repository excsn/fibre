use channels_arena::bench::Bench;
use channels_arena::driver::RunError;
use channels_arena::matrix;
use channels_arena::measure::{Budget, Measurement, format_throughput};
use channels_arena::registry::registry;
use channels_arena::report::{Record, Report};
use channels_arena::spec::{Api, BatchSupport, Capacity, Flavor, Mode, Pairing};

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_BATCH_SIZE: usize = 512;

struct Args {
  libraries: Option<Vec<String>>,
  flavors: Option<Vec<Flavor>>,
  modes: Option<Vec<Mode>>,
  capacities: Option<Vec<Capacity>>,
  pairings: Option<Vec<(usize, usize)>>,
  batch_size: Option<usize>,
  single_only: bool,
  batch_only: bool,
  budget: Budget,
  machine: String,
  out: PathBuf,
  list: bool,
  rerender: Option<PathBuf>,
}

const USAGE: &str = "\
channels_arena - cross-implementation channel benchmarks

USAGE:
    cargo run --release -- [OPTIONS]

OPTIONS:
    --library <a,b>    only these libraries (fibre, tokio, crossbeam, flume, kanal, async-channel, std)
    --flavor <a,b>     only these shapes (spsc, mpsc, spmc, mpmc)
    --mode <a,b>       only these modes (sync, async)
    --cap <a,b>        only these capacities (rendezvous, 1, 128, 1024, unbounded)
    --pairing <a,b>    only these requested loads, as PxC (1x1, 16x16, 64x64, 16x1, 1x16, 64x1, 1x64)
    --batch-size <n>   items per batch call (default 512)
    --no-batch         skip the batched cells
    --only-batch       run only the batched cells
    --samples <n>      samples per cell after warmup (default 7)
    --target-ms <n>    wall-clock target per sample, drives item calibration (default 60)
    --machine <label>  machine label recorded in the docs
    --out <dir>        output directory (default docs)
    --list             list the matrix and exit
    --rerender <tsv>   rewrite the pages from a previous run's results.tsv, measuring nothing
    --help             this message

Sample rounds are interleaved across implementations within a cell, so drift in
machine state is shared rather than landing on whichever one ran last.
";

fn parse_list<T>(raw: &str, parse: impl Fn(&str) -> Option<T>, what: &str) -> Vec<T> {
  raw
    .split(',')
    .map(|part| {
      parse(part.trim()).unwrap_or_else(|| {
        eprintln!("unknown {}: {}", what, part.trim());
        std::process::exit(2);
      })
    })
    .collect()
}

fn parse_pairing(raw: &str) -> Option<(usize, usize)> {
  let (p, c) = raw.split_once('x')?;
  Some((p.trim().parse().ok()?, c.trim().parse().ok()?))
}

fn parse_args() -> Args {
  let mut args = Args {
    libraries: None,
    flavors: None,
    modes: None,
    capacities: None,
    pairings: None,
    batch_size: Some(DEFAULT_BATCH_SIZE),
    single_only: false,
    batch_only: false,
    budget: Budget::default(),
    machine: default_machine(),
    out: PathBuf::from("docs"),
    list: false,
    rerender: None,
  };

  let mut argv = std::env::args().skip(1);
  while let Some(flag) = argv.next() {
    let mut value = || {
      argv.next().unwrap_or_else(|| {
        eprintln!("{} needs a value", flag);
        std::process::exit(2);
      })
    };
    match flag.as_str() {
      "--library" => {
        args.libraries = Some(value().split(',').map(|s| s.trim().to_string()).collect())
      }
      "--flavor" => args.flavors = Some(parse_list(&value(), Flavor::parse, "flavor")),
      "--mode" => args.modes = Some(parse_list(&value(), Mode::parse, "mode")),
      "--cap" => args.capacities = Some(parse_list(&value(), Capacity::parse, "capacity")),
      "--pairing" => args.pairings = Some(parse_list(&value(), parse_pairing, "pairing")),
      "--batch-size" => {
        args.batch_size = Some(value().parse().expect("--batch-size must be a number"))
      }
      "--no-batch" => args.single_only = true,
      "--only-batch" => args.batch_only = true,
      "--samples" => args.budget.samples = value().parse().expect("--samples must be a number"),
      "--target-ms" => {
        args.budget.target = Duration::from_millis(value().parse().expect("--target-ms must be a number"))
      }
      "--machine" => args.machine = value(),
      "--out" => args.out = PathBuf::from(value()),
      "--list" => args.list = true,
      "--rerender" => args.rerender = Some(PathBuf::from(value())),
      "--help" | "-h" => {
        print!("{}", USAGE);
        std::process::exit(0);
      }
      other => {
        eprintln!("unknown flag: {}\n\n{}", other, USAGE);
        std::process::exit(2);
      }
    }
  }
  if args.single_only {
    args.batch_size = None;
  }
  args
}

fn default_machine() -> String {
  let cores = std::thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(0);
  format!("{} ({} cores)", std::env::consts::ARCH, cores)
}

/// Two concurrent runs would compete for the same cores and quietly corrupt
/// each other's timings, so a run claims the output directory for its duration.
struct RunLock(PathBuf);

impl RunLock {
  fn claim(dir: &Path) -> RunLock {
    if let Err(e) = std::fs::create_dir_all(dir) {
      eprintln!("cannot create {}: {}", dir.display(), e);
      std::process::exit(1);
    }
    let path = dir.join(".arena-running");
    match std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&path)
    {
      Ok(mut file) => {
        let _ = writeln!(file, "pid {}", std::process::id());
        RunLock(path)
      }
      Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
        eprintln!(
          "another run holds {}.\n\
           Benchmarks must not overlap: concurrent runs compete for cores and both sets of numbers become meaningless.\n\
           If no run is active, delete that file and retry.",
          path.display()
        );
        std::process::exit(1);
      }
      Err(e) => {
        eprintln!("cannot create {}: {}", path.display(), e);
        std::process::exit(1);
      }
    }
  }
}

impl Drop for RunLock {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

struct Active<'a> {
  entry: &'a dyn Bench,
  items: u64,
  samples: Vec<Duration>,
  failed: bool,
}

/// A batched cell only means something for an implementation that has a batch
/// API; the rest would just re-measure their single-item path under a new name.
fn runs_cell(entry: &dyn Bench, cell: &channels_arena::spec::Cell) -> bool {
  entry.flavor() == cell.flavor
    && entry.mode() == cell.mode
    && (cell.api == Api::Single || entry.batch_support() != BatchSupport::None)
}

fn write_report(report: &Report, out: &Path) {
  match report.write(out) {
    Ok(files) => {
      println!("wrote {} files to {}", files.len(), out.display());
      for f in files {
        println!("  {}", f);
      }
    }
    Err(e) => {
      eprintln!("failed to write report: {}", e);
      std::process::exit(1);
    }
  }
}

fn main() {
  let args = parse_args();

  if let Some(path) = &args.rerender {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
      eprintln!("cannot read {}: {}", path.display(), e);
      std::process::exit(1);
    });
    let report = Report::from_tsv(args.machine, &text).unwrap_or_else(|e| {
      eprintln!("cannot parse {}: {}", path.display(), e);
      std::process::exit(1);
    });
    write_report(&report, &args.out);
    return;
  }

  let cells: Vec<_> = matrix::cells(args.batch_size)
    .into_iter()
    .filter(|c| !args.batch_only || c.api != Api::Single)
    .filter(|c| args.flavors.as_ref().is_none_or(|f| f.contains(&c.flavor)))
    .filter(|c| args.modes.as_ref().is_none_or(|m| m.contains(&c.mode)))
    .filter(|c| args.capacities.as_ref().is_none_or(|v| v.contains(&c.capacity)))
    .filter(|c| {
      args.pairings.as_ref().is_none_or(|v| {
        v.iter()
          .any(|requested| Pairing::clamped(*requested, c.flavor) == c.pairing)
      })
    })
    .collect();

  let entries: Vec<_> = registry()
    .into_iter()
    .filter(|e| {
      args
        .libraries
        .as_ref()
        .is_none_or(|l| l.iter().any(|want| want == e.library()))
    })
    .collect();

  if args.list {
    for cell in &cells {
      let mut libs: Vec<_> = entries
        .iter()
        .filter(|e| runs_cell(&***e, cell))
        .map(|e| e.library())
        .collect();
      libs.dedup();
      println!("{:<48} {}", cell.to_string(), libs.join(", "));
    }
    println!("\n{} cells, {} registry entries", cells.len(), entries.len());
    return;
  }

  let _lock = RunLock::claim(&args.out);

  let runtime = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("tokio runtime");

  let started = Instant::now();
  let mut records: Vec<Record> = Vec::new();

  for (index, cell) in cells.iter().enumerate() {
    let mut active: Vec<Active> = Vec::new();
    for entry in &entries {
      if !runs_cell(&**entry, cell) {
        continue;
      }
      match entry.calibrate(&runtime, cell, &args.budget) {
        Ok(items) => active.push(Active {
          entry: &**entry,
          items,
          samples: Vec::with_capacity(args.budget.samples),
          failed: false,
        }),
        Err(RunError::Unsupported) => {}
        Err(RunError::Miscounted) => {
          eprintln!("  {:<16} MISCOUNTED at {}, dropped", entry.library(), cell);
        }
      }
    }
    if active.is_empty() {
      continue;
    }

    for _ in 0..args.budget.samples {
      for slot in active.iter_mut() {
        if slot.failed {
          continue;
        }
        match slot.entry.run_once(&runtime, cell, slot.items) {
          Ok(elapsed) => slot.samples.push(elapsed),
          Err(_) => slot.failed = true,
        }
      }
    }

    println!("[{}/{}] {}", index + 1, cells.len(), cell);
    for slot in active {
      if slot.failed || slot.samples.is_empty() {
        eprintln!("  {:<16} failed mid-sampling, dropped", slot.entry.library());
        continue;
      }
      let measurement = Measurement::from_samples(cell, slot.items, &slot.samples);
      println!(
        "  {:<16} {:>8} Melem/s  ({} items, {}-{})",
        slot.entry.library(),
        format_throughput(measurement.median()),
        measurement.items,
        format_throughput(measurement.min()),
        format_throughput(measurement.max()),
      );
      if records
        .iter()
        .any(|r| r.library == slot.entry.library() && r.cell == *cell)
      {
        eprintln!(
          "  warning: {} measured {} twice; keeping the first",
          slot.entry.library(),
          cell
        );
        continue;
      }
      records.push(Record {
        library: slot.entry.library(),
        cell: *cell,
        batch_support: slot.entry.batch_support(),
        measurement,
      });
    }
  }

  println!("\ncompleted in {:.1}s", started.elapsed().as_secs_f64());

  let report = Report {
    machine: args.machine,
    records,
  };
  write_report(&report, &args.out);
}
