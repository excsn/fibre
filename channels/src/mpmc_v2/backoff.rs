// `hint::spin_loop` MUST route through the facade: under loom it is a scheduler
// yield-equivalent, whereas a raw `std::hint::spin_loop` is invisible to the
// model checker and blows the branch cap.
use crate::internal::sync::{hint, thread};

/// Emits a CPU instruction that signals the processor that it is in a spin loop.
#[inline(always)]
fn spin_hint() {
  hint::spin_loop();
}

/// An adaptive wait strategy that starts with spinning, then yields, then parks.
pub(crate) fn adaptive_wait<F>(cond: F)
where
  F: Fn() -> bool,
{
  // Under loom every spin/yield iteration is a tracked op counting against the
  // branch cap, so both pre-park phases collapse to a single check and we fall
  // straight through to the park - the path the model checker is meant to
  // explore (see the loom rollout doc's Gotcha #1).
  const SPIN_ROUNDS: usize = if crate::internal::sync::IS_LOOM { 1 } else { 10 };
  const YIELD_ROUNDS: usize = if crate::internal::sync::IS_LOOM { 1 } else { 20 };

  // 1. Spinning Phase
  for _ in 0..SPIN_ROUNDS {
    if cond() {
      return;
    }
    spin_hint();
  }

  // 2. Yielding Phase
  for _ in 0..YIELD_ROUNDS {
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
