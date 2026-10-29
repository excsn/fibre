//! The command line, shared by this crate's binary and any binary that adds
//! engines of its own.

use crate::engines::Runner;
use crate::harness::{Cell, Cfg, Report};
use crate::machine;
use crate::report::{Pages, Record, RunInfo};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_CELLS: &[&str] = &[
  "zipf sync 8",
  "zipf sync 64",
  "zipf async 8",
  "zipf async 64",
  "zipf mixed 64",
  "insert sync 1",
  "insert sync 64",
  "insert async 64",
  "hit sync 1",
  "hit sync 8",
  "hit async 64",
  "hot sync 8",
  "hot async 8",
];

const USAGE: &str = "usage: [--engine a,b] [--cell \"<op> <mode> <workers>\"]... [--secs S] [--capacity N] [--out FILE] [--docs DIR] [--machine LABEL] [--rerender TSV] [--list]
  op: hit | insert | zipf | hot    mode: sync | async | mixed
  --engine defaults to every engine; --list prints the cells and engines
  --out appends the raw result lines to FILE as they are produced
  --docs writes the pages (one per op, an index, raw/results.tsv and raw/results.json) into DIR
  --machine labels the pages; defaults to the architecture and core count
  --rerender rebuilds the pages from an earlier raw/results.tsv and measures nothing";

struct Args {
  engines: Vec<String>,
  cells: Vec<Cell>,
  cell_names: Vec<String>,
  secs: f64,
  capacity: u64,
  out: Option<String>,
  docs: Option<PathBuf>,
  machine: String,
  rerender: Option<PathBuf>,
  list: bool,
}

fn default_machine() -> String {
  let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
  format!("{} ({} cores)", std::env::consts::ARCH, cores)
}

fn parse(names: &[&str], default_docs: Option<PathBuf>) -> Result<Args, String> {
  let mut a = Args {
    engines: names.iter().map(|s| s.to_string()).collect(),
    cells: Vec::new(),
    cell_names: Vec::new(),
    secs: 10.0,
    capacity: 10_000,
    out: None,
    docs: default_docs,
    machine: default_machine(),
    rerender: None,
    list: false,
  };
  let mut it = std::env::args().skip(1);
  while let Some(flag) = it.next() {
    let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
    match flag.as_str() {
      "--engine" => a.engines = val()?.split(',').map(str::to_string).collect(),
      "--cell" => a.cell_names.push(val()?),
      "--secs" => a.secs = val()?.parse().map_err(|_| "bad --secs")?,
      "--capacity" => a.capacity = val()?.parse().map_err(|_| "bad --capacity")?,
      "--out" => a.out = Some(val()?),
      "--docs" => a.docs = Some(PathBuf::from(val()?)),
      "--machine" => a.machine = val()?,
      "--rerender" => a.rerender = Some(PathBuf::from(val()?)),
      "--list" => a.list = true,
      "--help" | "-h" => return Err(USAGE.to_string()),
      f => return Err(format!("unknown flag `{f}`\n{USAGE}")),
    }
  }
  if a.cell_names.is_empty() {
    a.cell_names = DEFAULT_CELLS.iter().map(|s| s.to_string()).collect();
  }
  for c in &a.cell_names {
    a.cells.push(Cell::parse(c)?);
  }
  for e in &a.engines {
    if !names.contains(&e.as_str()) {
      return Err(format!("unknown engine `{e}`; known: {}", names.join(", ")));
    }
  }
  Ok(a)
}

fn shell(cmd: &str, args: &[&str]) -> String {
  Command::new(cmd)
    .args(args)
    .output()
    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    .unwrap_or_default()
}

fn machine_state() -> String {
  let pm = shell("pmset", &["-g"]);
  let mode = pm.lines().find(|l| l.contains("powermode")).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "));
  let up = shell("uptime", &[]);
  let load = up.find("load").map(|i| up[i..].to_string()).unwrap_or(up);
  format!("{} {}", mode.unwrap_or_else(|| "powermode ?".into()), load)
}

fn line(engine: &str, cell: &str, cfg: &Cfg, r: &Option<Report>) -> String {
  match r {
    Some(r) => format!(
      "engine={engine} cell=\"{cell}\" capacity={} secs={:.1} total={} hits={} hit_ratio={:.4} mops={:.3} peak_len={} end_len={} settled_len={}",
      cfg.capacity,
      r.elapsed,
      r.total,
      r.hits,
      r.hit_ratio(),
      r.mops(),
      r.peak_len,
      r.end_len,
      r.settled_len
    ),
    None => format!("engine={engine} cell=\"{cell}\" unsupported"),
  }
}

fn write_pages(pages: &Pages, dir: &Path) {
  match pages.write(dir) {
    Ok(files) => println!("\nwrote {} in {}", files.join(", "), dir.display()),
    Err(e) => eprintln!("cannot write {}: {}", dir.display(), e),
  }
}

/// Parses the process arguments and runs the cells against `engines`. Pages
/// are written only when `--docs` names a directory.
pub fn main_with(engines: &[(&'static str, Runner)]) {
  main_with_docs(engines, None)
}

/// As `main_with`, with a default directory for the pages.
pub fn main_with_docs(engines: &[(&'static str, Runner)], default_docs: Option<PathBuf>) {
  let names: Vec<&str> = engines.iter().map(|(n, _)| *n).collect();
  let args = match parse(&names, default_docs) {
    Ok(a) => a,
    Err(e) => {
      eprintln!("{e}");
      std::process::exit(2);
    }
  };

  if let Some(path) = &args.rerender {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
      eprintln!("cannot read {}: {}", path.display(), e);
      std::process::exit(1);
    });
    let mut pages = Pages::from_tsv(args.machine.clone(), &text).unwrap_or_else(|e| {
      eprintln!("cannot parse {}: {}", path.display(), e);
      std::process::exit(1);
    });
    if let Some(run) = std::fs::read_to_string(path.with_file_name("results.json"))
      .ok()
      .and_then(|t| RunInfo::from_json(&t))
    {
      pages.run = run;
    }
    if pages.run.versions.is_empty() {
      pages.run.versions = machine::versions();
    }
    let dir = args
      .docs
      .clone()
      .or_else(|| path.parent().and_then(Path::parent).map(Path::to_path_buf))
      .unwrap_or_else(|| PathBuf::from("docs"));
    write_pages(&pages, &dir);
    return;
  }

  if args.list {
    for c in &args.cell_names {
      println!("{c}: {}", args.engines.join(", "));
    }
    println!("{} cells x {:.0} s", args.cells.len() * args.engines.len(), args.secs);
    return;
  }
  let mut out = args.out.as_ref().map(|p| {
    std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap_or_else(|e| panic!("open {p}: {e}"))
  });
  let mut emit = |s: &str| {
    println!("{s}");
    if let Some(f) = out.as_mut() {
      writeln!(f, "{s}").expect("write --out");
    }
  };
  let mut run = machine::capture();
  emit(&format!("# start {}", machine_state()));
  let mut results: Vec<(usize, String, Option<Report>)> = Vec::new();
  let mut records: Vec<Record> = Vec::new();
  for (i, (cell, name)) in args.cells.iter().zip(&args.cell_names).enumerate() {
    for engine in &args.engines {
      let cfg = Cfg { cell: *cell, capacity: args.capacity, secs: args.secs };
      let runner = engines.iter().find(|(n, _)| n == engine).expect("engine checked in parse").1;
      let r = runner(&cfg);
      emit(&line(engine, name, &cfg, &r));
      if let Some(report) = &r {
        records.push(Record {
          engine: engine.clone(),
          cell: *cell,
          capacity: args.capacity,
          secs: args.secs,
          report: report.clone(),
        });
      }
      results.push((i, engine.clone(), r));
    }
  }
  run.load_at_end = machine::load_average();
  emit(&format!("# end {}", machine_state()));

  println!();
  println!("Mops/s, hit ratio (h) and the peak entry count during the run (peak) at capacity {}.", args.capacity);
  println!();
  println!("| cell | {} |", args.engines.join(" | "));
  println!("|---|{}", "---|".repeat(args.engines.len()));
  for (i, name) in args.cell_names.iter().enumerate() {
    let cols: Vec<String> = results
      .iter()
      .filter(|(c, _, _)| *c == i)
      .map(|(_, _, r)| match r {
        Some(r) if r.hits > 0 => format!("{:.2} h{:.3} peak {}", r.mops(), r.hit_ratio(), r.peak_len),
        Some(r) => format!("{:.2} peak {}", r.mops(), r.peak_len),
        None => "-".to_string(),
      })
      .collect();
    println!("| {name} | {} |", cols.join(" | "));
  }

  if let Some(dir) = &args.docs {
    let pages = Pages { machine: args.machine.clone(), run, records };
    write_pages(&pages, dir);
  }
}
