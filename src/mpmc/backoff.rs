use std::thread;

/// Emits a CPU instruction that signals the processor that it is in a spin loop.
#[inline(always)]
fn spin_hint() {
  std::hint::spin_loop();
}

/// An adaptive wait strategy that starts with spinning, then yields, then parks.
pub(crate) fn adaptive_wait<F>(cond: F)
where
  F: Fn() -> bool,
{
  // 1. Spinning Phase
  for _ in 0..10 {
    if cond() {
      return;
    }
    spin_hint();
  }

  // 2. Yielding Phase
  for _ in 0..20 {
    if cond() {
      return;
    }
    thread::yield_now();
  }

  // 3. Blocking Phase - Simplified and hardened
  while !cond() {
    // Park indefinitely. The thread will ONLY be woken by an `unpark()` call.
    // This is less complex and more robust than park_timeout for this pattern.
    thread::park();
  }
}
