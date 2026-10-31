//! Process statistics for one PID, read with `sysinfo`.

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::collect::Collector;
use crate::error::CollectError;
use crate::recorder::Recorder;
use crate::value::NumberValue;

crate::metric! {
  /// Whether the process was found at the last read.
  pub static ALIVE: State = "process.alive";

  /// CPU used by the process as a share of every core, so 100 is the whole machine.
  /// Zero on the first read, since it needs two reads to measure.
  #[unit(percent)]
  pub static CPU_PCT: Gauge = "process.cpu_pct";

  /// Resident memory.
  #[unit(bytes)]
  pub static MEMORY_BYTES: Gauge = "process.memory_bytes";

  /// Virtual memory.
  #[unit(bytes)]
  pub static VIRTUAL_MEMORY_BYTES: Gauge = "process.virtual_memory_bytes";

  /// Threads. Linux only; not recorded elsewhere.
  pub static THREADS: Gauge = "process.threads";

  /// Open file descriptors. Not recorded where the platform cannot say.
  pub static OPEN_FDS: Gauge = "process.open_fds";

  /// Bytes read from disk since the previous read.
  #[unit(bytes)]
  pub static DISK_READ_BYTES: Counter = "process.disk.read_bytes";

  /// Bytes written to disk since the previous read.
  #[unit(bytes)]
  pub static DISK_WRITTEN_BYTES: Counter = "process.disk.written_bytes";

  /// Seconds since the process started.
  #[unit(seconds)]
  pub static RUN_TIME: Gauge = "process.run_time_secs";
}

/// Records the `process.*` metrics for one PID each time it runs.
pub struct ProcessCollector {
  pid: Pid,
  system: System,
  refresh: ProcessRefreshKind,
  cores: f32,
}

/// A collector for the process with this PID. The caller adds any tags that
/// name the process, through `Stats::add_collector_with_tags` or a `Context`.
pub fn process(pid: u32) -> Result<ProcessCollector, CollectError> {
  let system = System::new().map_err(|e| CollectError::Unavailable(format!("{e:?}")))?;
  let cores = std::thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(1) as f32;
  Ok(ProcessCollector {
    pid: Pid::from_u32(pid),
    system,
    refresh: ProcessRefreshKind::nothing()
      .with_cpu()
      .with_memory()
      .with_tasks()
      .with_disk_usage(),
    cores,
  })
}

impl ProcessCollector {
  pub fn pid(&self) -> u32 {
    self.pid.as_u32()
  }
}

impl Collector for ProcessCollector {
  fn collect(&mut self, recorder: &dyn Recorder) {
    let pids = [self.pid];
    self
      .system
      .refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, self.refresh);
    let Some(process) = self.system.process(self.pid) else {
      ALIVE.set_to(recorder, false, Vec::new());
      return;
    };
    ALIVE.set_to(recorder, true, Vec::new());
    CPU_PCT.set_to(
      recorder,
      NumberValue::percent_saturating(process.cpu_usage() / self.cores),
      Vec::new(),
    );
    MEMORY_BYTES.set_to(recorder, process.memory(), Vec::new());
    VIRTUAL_MEMORY_BYTES.set_to(recorder, process.virtual_memory(), Vec::new());
    if let Some(tasks) = process.tasks() {
      THREADS.set_to(recorder, tasks.len(), Vec::new());
    }
    if let Some(open) = process.open_files() {
      OPEN_FDS.set_to(recorder, open, Vec::new());
    }
    let disk = process.disk_usage();
    DISK_READ_BYTES.add_to(recorder, disk.read_bytes, Vec::new());
    DISK_WRITTEN_BYTES.add_to(recorder, disk.written_bytes, Vec::new());
    RUN_TIME.set_to(recorder, process.run_time(), Vec::new());
  }
}
