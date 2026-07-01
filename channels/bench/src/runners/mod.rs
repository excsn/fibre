pub mod mpmc_v2;
pub mod mpmc_v3;
pub mod mpsc;
pub mod spmc;
pub mod spsc;

pub struct BenchConfig {
  pub producers: usize,
  pub consumers: usize,
  pub capacity: usize,
  pub items: usize,
  pub stall_ms: u64,
  pub stall_count: usize,
  pub batch_size: usize,
}

pub struct RunResult {
  pub sent: usize,
  pub received: usize,
  pub duration: std::time::Duration,
  pub watchdog_ticks: Vec<crate::watchdog::WatchdogTick>,
}
