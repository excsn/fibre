fibre_stats::metric! {
  /// In the program.
  pub static LOCAL: Counter = "fixture.local";
}

fn main() {
  dep_called::recording::work(1);
  std::hint::black_box(&dep_called::handle::HANDLE);
  for metric in fibre_stats::registry() {
    println!("{}", metric.name);
  }
}
