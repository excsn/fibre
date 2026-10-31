#![cfg(not(target_family = "wasm"))]

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{Fixtures, Package, stdout, value};

fn skip(test: &str, why: &str) {
  let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
}

fn tool_output(tool: &str, args: &[&str]) -> Option<String> {
  let output = Command::new(tool).args(args).output().ok()?;
  output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn target_installed(target: &str) -> bool {
  let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
  let Some(sysroot) = tool_output(&rustc, &["--print", "sysroot"]) else {
    return false;
  };
  Path::new(&sysroot).join("lib/rustlib").join(target).is_dir()
}

/// Why the test cannot run on this machine, if it cannot.
fn missing(target: &str) -> Option<String> {
  if !target_installed(target) {
    return Some(format!("target {target} is not installed, install it with `rustup target add {target}`"));
  }
  if tool_output("node", &["--version"]).is_none() {
    return Some("node is not on PATH".to_owned());
  }
  None
}

fn node(script: &str, wasm: &Path) -> String {
  let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(script);
  let output = Command::new("node").arg(script).arg(wasm).output().unwrap();
  stdout(&output, "node")
}

fn unix_now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

const UNKNOWN: &str = "wasm32-unknown-unknown";

fn host_clock_fixtures() -> Fixtures {
  let stats = "fibre_stats = { path = \"@stats\" }";
  let called = "dep_called = { path = \"../dep_called\" }";
  let unnamed = "dep_unnamed = { path = \"../dep_unnamed\" }";
  Fixtures::write(
    "wasm_host_clock",
    &[
      Package { name: "dep_called", kind: "lib", source: "dep_called.rs", dependencies: &[stats] },
      Package { name: "dep_unnamed", kind: "lib", source: "dep_unnamed.rs", dependencies: &[stats] },
      Package { name: "wasm_host_clock", kind: "cdylib", source: "wasm_host_clock.rs", dependencies: &[stats, called, unnamed] },
      Package { name: "wasm_no_clock", kind: "lib", source: "wasm_no_clock.rs", dependencies: &[stats] },
    ],
  )
}

fn run_host_clock(profile: &str) -> String {
  let fixtures = host_clock_fixtures();
  let mut args = vec!["build", "-q", "-p", "wasm_host_clock", "--target", UNKNOWN];
  if profile == "release" {
    args.push("--release");
  }
  fixtures.build(&args);
  let wasm = fixtures.target_dir().join(UNKNOWN).join(profile).join("wasm_host_clock.wasm");
  node("wasm_host_clock.cjs", &wasm)
}

fn assert_host_clock_contract(out: &str) {
  assert_eq!(value(out, "ctors_exported"), "false", "{out}");
  assert!(value(out, "first_call_count").parse::<u32>().unwrap() >= 4, "{out}");
  assert_eq!(value(out, "listed_local"), "1", "{out}");
  assert_eq!(value(out, "listed_recorded"), "1", "{out}");
  assert_eq!(value(out, "listed_handle"), "1", "{out}");
  assert_eq!(value(out, "listed_unnamed"), "0", "{out}");
  assert_eq!(value(out, "batches_first"), "1", "{out}");
  assert_eq!(value(out, "series_first"), "2", "{out}");
  assert_eq!(value(out, "timer_max_ms"), "5", "{out}");
  assert_eq!(value(out, "batches_second"), "2", "{out}");
  assert_eq!(value(out, "series_second"), "5", "{out}");
  assert_eq!(value(out, "refused"), "0", "{out}");
}

#[test]
fn a_debug_module_with_a_host_clock_fills_the_registry_records_and_flushes() {
  if let Some(why) = missing(UNKNOWN) {
    return skip("a_debug_module_with_a_host_clock_fills_the_registry_records_and_flushes", &why);
  }
  let out = run_host_clock("debug");
  assert_host_clock_contract(&out);
  assert_eq!(
    value(&out, "listed_unreferenced"),
    "0",
    "the usage guide says a definition can be missing even when the crate is called, update it if this changed"
  );
}

#[test]
fn a_release_module_with_a_host_clock_fills_the_registry_records_and_flushes() {
  if let Some(why) = missing(UNKNOWN) {
    return skip("a_release_module_with_a_host_clock_fills_the_registry_records_and_flushes", &why);
  }
  assert_host_clock_contract(&run_host_clock("release"));
}

#[test]
fn without_wasm_bindgen_a_default_config_is_a_compile_error() {
  if !target_installed(UNKNOWN) {
    return skip("without_wasm_bindgen_a_default_config_is_a_compile_error", "wasm32-unknown-unknown is not installed");
  }
  let fixtures = host_clock_fixtures();
  let output = fixtures.cargo(&["check", "-p", "wasm_no_clock", "--target", UNKNOWN]);
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(!output.status.success(), "{stderr}");
  assert!(stderr.contains("E0599") && stderr.contains("`default`") && stderr.contains("StatsConfig"), "{stderr}");
}

#[test]
fn a_wasm_bindgen_module_reads_the_javascript_clock() {
  let test = "a_wasm_bindgen_module_reads_the_javascript_clock";
  if let Some(why) = missing(UNKNOWN) {
    return skip(test, &why);
  }
  let Some(cli) = tool_output("wasm-bindgen", &["--version"]) else {
    return skip(test, "wasm-bindgen is not on PATH, install it with `cargo install wasm-bindgen-cli`");
  };
  let version = cli.rsplit(' ').next().unwrap();
  let library = format!("wasm-bindgen = \"={version}\"");
  let fixtures = Fixtures::write(
    "wasm_bindgen",
    &[Package {
      name: "wasm_bindgen_clock",
      kind: "cdylib",
      source: "wasm_bindgen_clock.rs",
      dependencies: &["fibre_stats = { path = \"@stats\", features = [\"wasm-bindgen\"] }", &library],
    }],
  );
  fixtures.build(&["build", "-q", "-p", "wasm_bindgen_clock", "--target", UNKNOWN]);
  let wasm = fixtures.target_dir().join(UNKNOWN).join("debug/wasm_bindgen_clock.wasm");
  let pkg: PathBuf = fixtures.root().join("pkg");
  let bindgen = Command::new("wasm-bindgen").args(["--target", "nodejs", "--out-dir"]).arg(&pkg).arg(&wasm).output().unwrap();
  stdout(&bindgen, "wasm-bindgen");

  let before = unix_now_ms();
  let run = Command::new("node")
    .arg("-e")
    .arg(format!("process.stdout.write(require({:?}).run())", pkg.join("wasm_bindgen_clock.js")))
    .output()
    .unwrap();
  let out = stdout(&run, "node");
  let after = unix_now_ms();

  assert_eq!(value(&out, "registry"), "2", "{out}");
  let now: u64 = value(&out, "now_ms").parse().unwrap();
  assert!((before..=after).contains(&now), "{before} <= {now} <= {after}");
  assert_eq!(value(&out, "series"), "3", "{out}");
  assert_eq!(value(&out, "timer_points"), "1", "{out}");
  assert_eq!(value(&out, "refused"), "0", "{out}");
}

#[test]
fn a_wasi_program_uses_the_std_clock() {
  let test = "a_wasi_program_uses_the_std_clock";
  let target = "wasm32-wasip1";
  if let Some(why) = missing(target) {
    return skip(test, &why);
  }
  let fixtures = Fixtures::write(
    "wasi",
    &[Package { name: "wasi_std_clock", kind: "bin", source: "wasi_std_clock.rs", dependencies: &["fibre_stats = { path = \"@stats\" }"] }],
  );
  fixtures.build(&["build", "-q", "-p", "wasi_std_clock", "--target", target]);
  let wasm = fixtures.target_dir().join(target).join("debug/wasi_std_clock.wasm");

  let before = unix_now_ms();
  let out = node("wasi_std_clock.cjs", &wasm);
  let after = unix_now_ms();

  assert_eq!(value(&out, "registry"), "2", "{out}");
  let now: u64 = value(&out, "now_ms").parse().unwrap();
  assert!((before..=after).contains(&now), "{before} <= {now} <= {after}");
  assert_eq!(value(&out, "series"), "3", "{out}");
  assert_eq!(value(&out, "refused"), "0", "{out}");
}
