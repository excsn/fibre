#![cfg(not(target_family = "wasm"))]

#[cfg(any(feature = "collect-process", feature = "collect-host", feature = "collect-tokio"))]
use fibre_stats::{Batch, Collector, Stats, StatsConfig, registry};

#[cfg(any(feature = "collect-process", feature = "collect-host", feature = "collect-tokio"))]
fn read(collector: &mut impl Collector, reads: usize) -> (Stats, Batch) {
  let stats = Stats::new(StatsConfig::default());
  for _ in 0..reads {
    collector.collect(&stats);
  }
  let batch = stats.drain(true);
  assert_eq!(stats.refused(), 0, "refused: {:?}", stats.last_refusal());
  for series in &batch.series {
    let metric = registry()
      .find(|m| m.name == &*series.name)
      .unwrap_or_else(|| panic!("{} is not in the registry", series.name));
    assert_eq!(metric.kind, series.kind, "{}", series.name);
    for tag in &series.tags {
      assert!(metric.allows(&tag.name), "{} does not declare tag {}", series.name, tag.name);
    }
  }
  (stats, batch)
}

#[cfg(any(feature = "collect-process", feature = "collect-host", feature = "collect-tokio"))]
fn numbers(batch: &Batch, name: &str) -> Vec<f64> {
  batch
    .series
    .iter()
    .filter(|s| &*s.name == name)
    .map(|s| s.points.numbers().unwrap_or_else(|| panic!("{name} has no numbers")).last().unwrap().1.to_f64())
    .collect()
}

#[cfg(any(feature = "collect-process", feature = "collect-host", feature = "collect-tokio"))]
fn number(batch: &Batch, name: &str) -> f64 {
  let values = numbers(batch, name);
  assert_eq!(values.len(), 1, "{name}: {values:?}");
  values[0]
}

#[cfg(feature = "collect-process")]
mod process {
  use super::*;
  use fibre_stats::{Kind, StateValue, collect};

  fn alive(batch: &Batch) -> StateValue {
    let series = batch.series.iter().find(|s| &*s.name == "process.alive").expect("process.alive");
    series.points.states().unwrap().last().unwrap().1
  }

  #[test]
  fn reports_the_running_process() {
    let mut collector = collect::process(std::process::id()).unwrap();
    assert_eq!(collector.pid(), std::process::id());
    let (_, batch) = read(&mut collector, 2);

    assert_eq!(alive(&batch), StateValue::from(true));
    let memory = number(&batch, "process.memory_bytes");
    assert!(memory > 1024.0 * 1024.0, "memory {memory}");
    assert!(number(&batch, "process.virtual_memory_bytes") >= memory);
    let cpu = number(&batch, "process.cpu_pct");
    assert!((0.0..=100.0).contains(&cpu), "cpu {cpu}");
    assert!(number(&batch, "process.run_time_secs") < 86_400.0);
    for name in ["process.disk.read_bytes", "process.disk.written_bytes"] {
      let series = batch.series.iter().find(|s| &*s.name == name).expect(name);
      assert_eq!(series.kind, Kind::Counter);
    }
    let threads = numbers(&batch, "process.threads");
    if cfg!(target_os = "linux") {
      assert!(threads[0] >= 1.0, "threads {threads:?}");
    } else {
      assert!(threads.is_empty(), "threads {threads:?}");
    }
    for fds in numbers(&batch, "process.open_fds") {
      assert!(fds >= 1.0, "fds {fds}");
    }
  }

  #[test]
  fn reports_a_missing_process_as_not_alive_and_nothing_else() {
    let mut collector = collect::process(u32::MAX - 1).unwrap();
    let (_, batch) = read(&mut collector, 1);
    assert_eq!(alive(&batch), StateValue::from(false));
    assert_eq!(batch.series.len(), 1, "{:?}", batch.series);
  }
}

#[cfg(feature = "collect-host")]
mod host {
  use super::*;
  use fibre_stats::{Kind, collect};

  #[test]
  fn reports_memory_within_the_machine_total() {
    let (_, batch) = read(&mut collect::host().unwrap(), 1);
    let total = number(&batch, "host.memory.total_bytes");
    assert!(total > 0.0);
    for name in ["host.memory.used_bytes", "host.memory.free_bytes", "host.memory.available_bytes"] {
      let value = number(&batch, name);
      assert!(value <= total, "{name} {value} above total {total}");
    }
    assert!(number(&batch, "host.swap.used_bytes") <= number(&batch, "host.swap.total_bytes"));
  }

  #[test]
  fn reports_cpu_as_percentages_with_one_series_per_core() {
    let (_, batch) = read(&mut collect::host().unwrap(), 2);
    let overall = number(&batch, "host.cpu.usage_pct");
    assert!((0.0..=100.0).contains(&overall), "cpu {overall}");
    let cores = numbers(&batch, "host.cpu.core.usage_pct");
    assert_eq!(cores.len(), std::thread::available_parallelism().unwrap().get());
    assert!(cores.iter().all(|c| (0.0..=100.0).contains(c)), "{cores:?}");
    assert!(number(&batch, "host.uptime_secs") > 0.0);
    #[cfg(unix)]
    for name in ["host.cpu.load_1", "host.cpu.load_5", "host.cpu.load_15"] {
      assert!(number(&batch, name) >= 0.0, "{name}");
    }
  }

  #[test]
  fn reports_each_disk_with_free_space_within_its_total() {
    let (_, batch) = read(&mut collect::host().unwrap(), 1);
    let totals: Vec<_> = batch.series.iter().filter(|s| &*s.name == "host.disk.total_bytes").collect();
    assert!(!totals.is_empty());
    for total in totals {
      let free = batch.series("host.disk.free_bytes", &total.tags).expect("free bytes for the same disk");
      let total_bytes = total.points.numbers().unwrap()[0].1.to_f64();
      let free_bytes = free.points.numbers().unwrap()[0].1.to_f64();
      assert!(free_bytes <= total_bytes, "{:?}", total.tags);
      if total_bytes > 0.0 {
        let pct = batch.series("host.disk.free_pct", &total.tags).expect("free pct for the same disk");
        let pct = pct.points.numbers().unwrap()[0].1.to_f64();
        assert!((pct - free_bytes / total_bytes * 100.0).abs() < 1.0, "{pct}");
      }
    }
  }

  #[test]
  fn reports_network_counters_per_interface() {
    let (_, batch) = read(&mut collect::host().unwrap(), 2);
    let recv: Vec<_> = batch.series.iter().filter(|s| &*s.name == "host.net.recv_bytes").collect();
    assert!(!recv.is_empty());
    for series in recv {
      assert_eq!(series.kind, Kind::Counter);
      assert_eq!(&*series.tags[0].name, "iface");
      for name in ["host.net.sent_bytes", "host.net.recv_packets", "host.net.sent_packets", "host.net.recv_errors", "host.net.sent_errors"] {
        assert!(batch.series(name, &series.tags).is_some(), "{name} for {:?}", series.tags);
      }
    }
  }
}

#[cfg(feature = "collect-tokio")]
mod tokio_runtime {
  use super::*;
  use fibre_stats::collect;

  fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().build().unwrap()
  }

  #[test]
  fn reports_workers_and_live_tasks() {
    let runtime = runtime();
    let task = runtime.spawn(std::future::pending::<()>());
    let (_, batch) = read(&mut collect::tokio(runtime.handle().clone()), 1);
    assert_eq!(number(&batch, "tokio.workers"), 1.0);
    assert_eq!(number(&batch, "tokio.tasks.alive"), 1.0);
    assert_eq!(number(&batch, "tokio.queue.global"), 1.0);
    task.abort();
  }

  #[test]
  fn reports_worker_activity_only_from_the_second_read() {
    let runtime = runtime();
    let mut collector = collect::tokio(runtime.handle().clone());
    let (_, first) = read(&mut collector, 1);
    assert!(numbers(&first, "tokio.worker.busy_pct").is_empty());
    assert!(first.series.iter().all(|s| &*s.name != "tokio.worker.parks"));

    runtime.block_on(async { std::thread::sleep(std::time::Duration::from_millis(20)) });
    let (_, second) = read(&mut collector, 1);
    let busy = number(&second, "tokio.worker.busy_pct");
    assert!((0.0..=100.0).contains(&busy), "busy {busy}");
    let parks = second.series.iter().find(|s| &*s.name == "tokio.worker.parks").expect("parks");
    assert_eq!(&*parks.tags[0].value, "0");
  }
}

#[cfg(feature = "xs_foundation")]
mod xs_clock {
  use std::sync::Arc;
  use std::time::{SystemTime, UNIX_EPOCH};

  use fibre_stats::{Recorder, Stats, StatsConfig, XsClock};

  #[test]
  fn reads_the_time_from_the_xs_foundation_clock() {
    let clock: Arc<dyn xs_foundation::time::Clock> = Arc::new(xs_foundation::time::UtcClock::new());
    let stats = Stats::new(StatsConfig { clock: Arc::new(XsClock(clock)), ..StatsConfig::default() });
    let system = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
    assert!(stats.now_ns().abs_diff(system) < 60_000_000_000, "{} against {system}", stats.now_ns());
  }
}
