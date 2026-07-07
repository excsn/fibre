use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct WatchdogConfig {
  pub stall_ms: u64,
  pub stall_count: usize,
  pub expected: usize,
}

/// One periodic watchdog observation, kept so a full run's progress-over-time can be
/// recorded (see `main.rs`'s `results.jsonl` output), not just the final tally.
#[derive(Debug, Clone, Default)]
pub struct WatchdogTick {
  pub elapsed_ms: u64,
  pub sent: usize,
  pub received: usize,
  pub in_flight: usize,
  pub ring_len: Option<usize>,
  pub send_waiters: Option<usize>,
  pub recv_waiters: Option<usize>,
}

/// Spawns the stall-watchdog thread.
///
/// `detail` - if provided, called each tick to get `(ring_len, send_waiters, recv_waiters)`.
/// Only available for mpmc_v3 sync (the only handle with `debug_state`).
pub fn spawn(
  sent: Arc<AtomicUsize>,
  received: Arc<AtomicUsize>,
  done: Arc<AtomicBool>,
  detail: Option<Box<dyn Fn() -> (usize, usize, usize) + Send>>,
  cfg: WatchdogConfig,
) -> thread::JoinHandle<Vec<WatchdogTick>> {
  thread::spawn(move || {
    let mut last = (0usize, 0usize);
    let mut stalls = 0usize;
    let mut elapsed_ms = 0u64;
    let mut ticks = Vec::new();
    loop {
      thread::sleep(Duration::from_millis(cfg.stall_ms));
      elapsed_ms += cfg.stall_ms;
      if done.load(Ordering::Relaxed) {
        break;
      }
      let s = sent.load(Ordering::Relaxed);
      let r = received.load(Ordering::Relaxed);
      let inflight = s.saturating_sub(r);
      let (ring_len, send_waiters, recv_waiters) = if let Some(ref f) = detail {
        let (len, sw, rw) = f();
        eprintln!(
          "[watchdog] sent={s} recv={r} in_flight={inflight} ring_len={len} send_waiters={sw} recv_waiters={rw}"
        );
        (Some(len), Some(sw), Some(rw))
      } else {
        eprintln!("[watchdog] sent={s} recv={r} in_flight={inflight}");
        (None, None, None)
      };
      ticks.push(WatchdogTick {
        elapsed_ms,
        sent: s,
        received: r,
        in_flight: inflight,
        ring_len,
        send_waiters,
        recv_waiters,
      });

      if (s, r) == last {
        stalls += 1;
        if stalls >= cfg.stall_count {
          let stall_ms_total = cfg.stall_ms * cfg.stall_count as u64;
          eprintln!(
            "\n[watchdog] *** STALL (no progress {stall_ms_total}ms) - likely deadlock ***"
          );
          eprintln!("[watchdog]   sent={s} recv={r} expected={}", cfg.expected);
          if let Some(ref f) = detail {
            let (len, sw, rw) = f();
            let inflight = s.saturating_sub(r);
            eprintln!("[watchdog]   in_flight={inflight} ring_len={len} send_waiters={sw} recv_waiters={rw}");
            if inflight == len && rw > 0 {
              eprintln!(
                "[watchdog]   => LOST WAKEUP: {len} item(s) in ring, {rw} receiver(s) parked."
              );
            } else if inflight > len {
              eprintln!(
                "[watchdog]   => LOST ITEM(S): {} item(s) neither in ring nor received.",
                inflight - len
              );
            } else if sw > 0 && rw > 0 {
              eprintln!("[watchdog]   => MUTUAL DEADLOCK: producers AND receivers both parked.");
            } else {
              eprintln!("[watchdog]   => (unclassified - see counts above)");
            }
          }
          fibre::telemetry::print_stall_report();
          std::process::exit(2);
        }
      } else {
        stalls = 0;
        last = (s, r);
      }
    }
    ticks
  })
}
