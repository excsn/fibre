use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct WatchdogConfig {
  pub stall_ms: u64,
  pub stall_count: usize,
  pub expected: usize,
}

/// Spawns the stall-watchdog thread.
///
/// `detail` — if provided, called each tick to get `(ring_len, send_waiters, recv_waiters)`.
/// Only available for mpmc_v3 sync (the only handle with `debug_state`).
pub fn spawn(
  sent: Arc<AtomicUsize>,
  received: Arc<AtomicUsize>,
  done: Arc<AtomicBool>,
  detail: Option<Box<dyn Fn() -> (usize, usize, usize) + Send>>,
  cfg: WatchdogConfig,
) -> thread::JoinHandle<()> {
  thread::spawn(move || {
    let mut last = (0usize, 0usize);
    let mut stalls = 0usize;
    loop {
      thread::sleep(Duration::from_millis(cfg.stall_ms));
      if done.load(Ordering::Relaxed) {
        break;
      }
      let s = sent.load(Ordering::Relaxed);
      let r = received.load(Ordering::Relaxed);
      let inflight = s.saturating_sub(r);
      if let Some(ref f) = detail {
        let (len, sw, rw) = f();
        eprintln!(
          "[watchdog] sent={s} recv={r} in_flight={inflight} ring_len={len} send_waiters={sw} recv_waiters={rw}"
        );
      } else {
        eprintln!("[watchdog] sent={s} recv={r} in_flight={inflight}");
      }

      if (s, r) == last {
        stalls += 1;
        if stalls >= cfg.stall_count {
          let stall_ms_total = cfg.stall_ms * cfg.stall_count as u64;
          eprintln!(
            "\n[watchdog] *** STALL (no progress {stall_ms_total}ms) — likely deadlock ***"
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
              eprintln!("[watchdog]   => (unclassified — see counts above)");
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
  })
}
