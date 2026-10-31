#![cfg(not(target_family = "wasm"))]

mod support;

use support::{Fixtures, Package, stdout};

fn fixtures() -> Fixtures {
  let stats = "fibre_stats = { path = \"@stats\" }";
  let called = "dep_called = { path = \"../dep_called\" }";
  let unnamed = "dep_unnamed = { path = \"../dep_unnamed\" }";
  Fixtures::write(
    "linking",
    &[
      Package { name: "dep_called", kind: "lib", source: "dep_called.rs", dependencies: &[stats] },
      Package { name: "dep_unnamed", kind: "lib", source: "dep_unnamed.rs", dependencies: &[stats] },
      Package { name: "native_unnamed", kind: "bin", source: "native_unnamed.rs", dependencies: &[stats, called, unnamed] },
      Package { name: "native_named", kind: "bin", source: "native_named.rs", dependencies: &[stats, called, unnamed] },
    ],
  )
}

fn registry_of(bin: &str, profile: &[&str]) -> Vec<String> {
  let fixtures = fixtures();
  let mut args = vec!["run", "-q", "-p", bin];
  args.extend_from_slice(profile);
  let output = fixtures.cargo(&args);
  stdout(&output, bin).lines().map(str::to_owned).collect()
}

fn assert_registry_of_a_program_that_never_names_a_crate(profile: &[&str]) {
  let names = registry_of("native_unnamed", profile);
  assert!(names.iter().any(|n| n == "fixture.local"), "{names:?}");
  assert!(names.iter().any(|n| n == "fixture.dep.recorded"), "{names:?}");
  assert!(names.iter().any(|n| n == "fixture.dep.handle"), "{names:?}");
  assert!(!names.iter().any(|n| n == "fixture.unnamed"), "{names:?}");
}

#[test]
fn a_debug_program_lists_what_it_refers_to_and_not_a_crate_it_never_names() {
  assert_registry_of_a_program_that_never_names_a_crate(&[]);
}

#[test]
fn a_release_program_lists_what_it_refers_to_and_not_a_crate_it_never_names() {
  assert_registry_of_a_program_that_never_names_a_crate(&["--release"]);
}

#[cfg(target_os = "macos")]
#[test]
fn on_macos_a_named_crate_contributes_every_definition() {
  let names = registry_of("native_named", &[]);
  for expected in ["fixture.unnamed", "fixture.dep.unreferenced", "fixture.dep.handle", "fixture.dep.recorded"] {
    assert!(names.iter().any(|n| n == expected), "{expected} missing from {names:?}");
  }
}
