use std::sync::atomic::{AtomicU64, Ordering};

const WEYL_CONSTANT: u64 = 0x9E3779B97F4A7C15;

/// A fast, `async`-safe, and non-cryptographically secure pseudo-random number
/// generator using a Weyl sequence on an atomic integer.
#[derive(Debug)]
pub(crate) struct FastRng {
  state: AtomicU64,
}

impl FastRng {
  /// Creates a new RNG with a given seed.
  pub fn new(seed: u64) -> Self {
    Self {
      state: AtomicU64::new(if seed == 0 { 1 } else { seed }),
    }
  }

  /// Atomically advances the Weyl sequence and returns the new state.
  #[inline(always)]
  fn next_weyl(&self) -> u64 {
    // fetch_add with wrap-around semantics is exactly what we need.
    // Ordering::Relaxed is sufficient because we only need atomicity for
    // this one value; we don't need to synchronize other memory operations.
    self.state.fetch_add(WEYL_CONSTANT, Ordering::Relaxed)
  }

  /// Returns true with a probability of 1 in `denominator`.
  /// The denominator MUST be a power of two.
  #[inline(always)]
  pub fn should_run(&self, denominator_pow2: u32) -> bool {
    let mask = (denominator_pow2 - 1) as u64;
    (self.next_weyl() & mask) == 0
  }
}
