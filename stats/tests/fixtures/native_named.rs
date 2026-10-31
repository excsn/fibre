use dep_unnamed as _;

fn main() {
  dep_called::recording::work(1);
  for metric in fibre_stats::registry() {
    println!("{}", metric.name);
  }
}
