use std::process::Command;

fn main() {
  let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
  if let Ok(out) = Command::new(rustc).arg("-V").output() {
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    println!("cargo:rustc-env=ARENA_RUSTC_VERSION={version}");
  }
  println!("cargo:rerun-if-changed=Cargo.lock");
}
