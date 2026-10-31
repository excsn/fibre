#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Builds the sources in `tests/fixtures` as separate crates, so each program
/// has its own link and `registry()` shows what that link kept.
pub struct Fixtures {
  root: PathBuf,
}

pub struct Package<'a> {
  pub name: &'a str,
  /// `lib`, `cdylib` or `bin`.
  pub kind: &'a str,
  /// A file in `tests/fixtures`.
  pub source: &'a str,
  /// Lines for the `[dependencies]` table. `@stats` is the path to this crate.
  pub dependencies: &'a [&'a str],
}

impl Fixtures {
  pub fn write(name: &str, packages: &[Package]) -> Fixtures {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let members: Vec<String> = packages.iter().map(|p| format!("{:?}", p.name)).collect();
    fs::create_dir_all(&root).unwrap();
    fs::write(
      root.join("Cargo.toml"),
      format!("[workspace]\nresolver = \"2\"\nmembers = [{}]\n", members.join(", ")),
    )
    .unwrap();
    for package in packages {
      let dir = root.join(package.name);
      fs::create_dir_all(&dir).unwrap();
      let source = crate_dir.join("tests/fixtures").join(package.source);
      let target = match package.kind {
        "bin" => format!("[[bin]]\nname = {:?}\npath = {:?}\n", package.name, source),
        "lib" => format!("[lib]\npath = {:?}\n", source),
        "cdylib" => format!("[lib]\npath = {:?}\ncrate-type = [\"cdylib\"]\n", source),
        other => panic!("unknown kind {other}"),
      };
      let dependencies = package.dependencies.join("\n").replace("@stats", crate_dir.to_str().unwrap());
      fs::write(
        dir.join("Cargo.toml"),
        format!(
          "[package]\nname = {:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n{target}[dependencies]\n{dependencies}\n",
          package.name
        ),
      )
      .unwrap();
    }
    Fixtures { root }
  }

  pub fn cargo(&self, args: &[&str]) -> Output {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
      .args(args)
      .current_dir(&self.root)
      .env("CARGO_TARGET_DIR", self.target_dir())
      .env_remove("RUSTFLAGS")
      .env_remove("CARGO_ENCODED_RUSTFLAGS")
      .output()
      .unwrap()
  }

  /// Runs cargo and panics with its output if it fails.
  pub fn build(&self, args: &[&str]) {
    let output = self.cargo(args);
    assert!(output.status.success(), "cargo {args:?}:\n{}", String::from_utf8_lossy(&output.stderr));
  }

  pub fn target_dir(&self) -> PathBuf {
    self.root.join("target")
  }

  pub fn root(&self) -> &Path {
    &self.root
  }
}

pub fn stdout(output: &Output, what: &str) -> String {
  assert!(output.status.success(), "{what}:\n{}", String::from_utf8_lossy(&output.stderr));
  String::from_utf8(output.stdout.clone()).unwrap()
}

/// Parses `key=value` lines into their value for `key`.
pub fn value<'a>(text: &'a str, key: &str) -> &'a str {
  text
    .lines()
    .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    .unwrap_or_else(|| panic!("{key} is not in:\n{text}"))
}
