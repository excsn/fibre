//! Host statistics, read with `sysinfo`.

use sysinfo::{Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

use crate::collect::Collector;
use crate::error::CollectError;
use crate::recorder::Recorder;
use crate::tag::Tag;
use crate::value::NumberValue;

crate::metric! {
  #[unit(bytes)]
  pub static MEMORY_TOTAL_BYTES: Gauge = "host.memory.total_bytes";
  #[unit(bytes)]
  pub static MEMORY_USED_BYTES: Gauge = "host.memory.used_bytes";
  #[unit(bytes)]
  pub static MEMORY_FREE_BYTES: Gauge = "host.memory.free_bytes";
  /// Memory a new process can use without swapping.
  #[unit(bytes)]
  pub static MEMORY_AVAILABLE_BYTES: Gauge = "host.memory.available_bytes";
  #[unit(bytes)]
  pub static SWAP_TOTAL_BYTES: Gauge = "host.swap.total_bytes";
  #[unit(bytes)]
  pub static SWAP_USED_BYTES: Gauge = "host.swap.used_bytes";
  #[unit(bytes)]
  pub static SWAP_FREE_BYTES: Gauge = "host.swap.free_bytes";

  /// CPU in use across every core. Zero on the first read, since it needs two reads to measure.
  #[unit(percent)]
  pub static CPU_USAGE_PCT: Gauge = "host.cpu.usage_pct";
  /// CPU in use on one core.
  #[unit(percent)]
  #[tags("cpu")]
  pub static CORE_USAGE_PCT: Gauge = "host.cpu.core.usage_pct";
  #[unit(ratio)]
  pub static LOAD_1: Gauge = "host.cpu.load_1";
  #[unit(ratio)]
  pub static LOAD_5: Gauge = "host.cpu.load_5";
  #[unit(ratio)]
  pub static LOAD_15: Gauge = "host.cpu.load_15";
  #[unit(seconds)]
  pub static UPTIME: Gauge = "host.uptime_secs";

  /// Bytes received since the previous read.
  #[unit(bytes)]
  #[tags("iface")]
  pub static NET_RECV_BYTES: Counter = "host.net.recv_bytes";
  /// Bytes sent since the previous read.
  #[unit(bytes)]
  #[tags("iface")]
  pub static NET_SENT_BYTES: Counter = "host.net.sent_bytes";
  #[tags("iface")]
  pub static NET_RECV_PACKETS: Counter = "host.net.recv_packets";
  #[tags("iface")]
  pub static NET_SENT_PACKETS: Counter = "host.net.sent_packets";
  #[tags("iface")]
  pub static NET_RECV_ERRORS: Counter = "host.net.recv_errors";
  #[tags("iface")]
  pub static NET_SENT_ERRORS: Counter = "host.net.sent_errors";

  #[unit(bytes)]
  #[tags("disk", "mount")]
  pub static DISK_TOTAL_BYTES: Gauge = "host.disk.total_bytes";
  #[unit(bytes)]
  #[tags("disk", "mount")]
  pub static DISK_FREE_BYTES: Gauge = "host.disk.free_bytes";
  #[unit(percent)]
  #[tags("disk", "mount")]
  pub static DISK_FREE_PCT: Gauge = "host.disk.free_pct";

  #[unit(celsius)]
  #[tags("component")]
  pub static TEMP: Gauge = "host.temp.celsius";
  /// The highest temperature the component has reported.
  #[unit(celsius)]
  #[tags("component")]
  pub static TEMP_MAX: Gauge = "host.temp.max_celsius";
  /// The temperature the component calls critical, where it says.
  #[unit(celsius)]
  #[tags("component")]
  pub static TEMP_CRITICAL: Gauge = "host.temp.critical_celsius";
}

/// Records the `host.*` metrics each time it runs.
pub struct HostCollector {
  system: System,
  refresh: RefreshKind,
  networks: Networks,
  disks: Disks,
  components: Components,
}

/// A collector for the machine the process runs on.
pub fn host() -> Result<HostCollector, CollectError> {
  let unavailable = |e: sysinfo::Error| CollectError::Unavailable(format!("{e:?}"));
  let refresh = RefreshKind::nothing()
    .with_cpu(CpuRefreshKind::nothing().with_usage())
    .with_memory(MemoryRefreshKind::everything());
  Ok(HostCollector {
    system: System::new_with_specifics(refresh).map_err(unavailable)?,
    refresh,
    networks: Networks::new_with_refreshed_list().map_err(unavailable)?,
    disks: Disks::new_with_refreshed_list().map_err(unavailable)?,
    components: Components::new_with_refreshed_list().map_err(unavailable)?,
  })
}

fn celsius(value: f32) -> Option<NumberValue> {
  if !value.is_finite() {
    return None;
  }
  NumberValue::ratio(value as f64)
}

impl Collector for HostCollector {
  fn collect(&mut self, recorder: &dyn Recorder) {
    self.system.refresh_specifics(self.refresh);
    self.networks.refresh(true);
    self.disks.refresh(true);
    self.components.refresh(true);

    let system = &self.system;
    MEMORY_TOTAL_BYTES.set_to(recorder, system.total_memory(), Vec::new());
    MEMORY_USED_BYTES.set_to(recorder, system.used_memory(), Vec::new());
    MEMORY_FREE_BYTES.set_to(recorder, system.free_memory(), Vec::new());
    MEMORY_AVAILABLE_BYTES.set_to(recorder, system.available_memory(), Vec::new());
    SWAP_TOTAL_BYTES.set_to(recorder, system.total_swap(), Vec::new());
    SWAP_USED_BYTES.set_to(recorder, system.used_swap(), Vec::new());
    SWAP_FREE_BYTES.set_to(recorder, system.free_swap(), Vec::new());

    CPU_USAGE_PCT.set_to(
      recorder,
      NumberValue::percent_saturating(system.global_cpu_usage()),
      Vec::new(),
    );
    for cpu in system.cpus() {
      CORE_USAGE_PCT.set_to(
        recorder,
        NumberValue::percent_saturating(cpu.usage()),
        vec![Tag::new("cpu", cpu.name().to_string())],
      );
    }
    if let Ok(load) = System::load_average() {
      for (metric, value) in [(&LOAD_1, load.one), (&LOAD_5, load.five), (&LOAD_15, load.fifteen)] {
        if let Some(value) = NumberValue::ratio(value) {
          metric.set_to(recorder, value, Vec::new());
        }
      }
    }
    if let Ok(uptime) = System::uptime() {
      UPTIME.set_to(recorder, uptime, Vec::new());
    }

    for (name, data) in self.networks.iter() {
      let tags = || vec![Tag::new("iface", name.to_string_lossy().into_owned())];
      NET_RECV_BYTES.add_to(recorder, data.received(), tags());
      NET_SENT_BYTES.add_to(recorder, data.transmitted(), tags());
      NET_RECV_PACKETS.add_to(recorder, data.packets_received(), tags());
      NET_SENT_PACKETS.add_to(recorder, data.packets_transmitted(), tags());
      NET_RECV_ERRORS.add_to(recorder, data.errors_on_received(), tags());
      NET_SENT_ERRORS.add_to(recorder, data.errors_on_transmitted(), tags());
    }

    for disk in self.disks.iter() {
      let tags = || {
        vec![
          Tag::new("disk", disk.name().to_string_lossy().into_owned()),
          Tag::new("mount", disk.mount_point().to_string_lossy().into_owned()),
        ]
      };
      let total = disk.total_space();
      let free = disk.available_space();
      DISK_TOTAL_BYTES.set_to(recorder, total, tags());
      DISK_FREE_BYTES.set_to(recorder, free, tags());
      if total > 0 {
        let pct = (free as f64 / total as f64 * 100.0) as f32;
        DISK_FREE_PCT.set_to(recorder, NumberValue::percent_saturating(pct), tags());
      }
    }

    for component in self.components.iter() {
      let tags = || vec![Tag::new("component", component.label().to_string())];
      if let Some(value) = component.temperature().and_then(celsius) {
        TEMP.set_to(recorder, value, tags());
      }
      if let Some(value) = component.max().and_then(celsius) {
        TEMP_MAX.set_to(recorder, value, tags());
      }
      if let Some(value) = component.critical().and_then(celsius) {
        TEMP_CRITICAL.set_to(recorder, value, tags());
      }
    }
  }
}
