pub fn stats() -> fibre_stats::Stats {
  fibre_stats::Stats::new(fibre_stats::StatsConfig::default())
}
