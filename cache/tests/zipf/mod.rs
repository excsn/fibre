pub const CAPACITY: u64 = 10_000;
pub const UNIVERSE: u64 = 100_000;
pub const OPS: usize = 2_000_000;
const EXPONENT: f64 = 0.99;

/// A fixed-seed Zipf key stream over `UNIVERSE` keys, rank order shuffled so
/// hot keys spread across shards.
pub struct Zipf {
  pub keys: Vec<u64>,
  /// Probability mass of the `CAPACITY` hottest keys: the hit ratio of a
  /// cache that holds exactly those keys and nothing else, which no cache of
  /// that capacity beats in expectation.
  pub best_hit_ratio: f64,
}

fn xorshift(x: &mut u64) -> u64 {
  *x ^= *x << 13;
  *x ^= *x >> 7;
  *x ^= *x << 17;
  *x
}

pub fn stream() -> Zipf {
  let mut cdf = Vec::with_capacity(UNIVERSE as usize);
  let mut acc = 0.0;
  for rank in 1..=UNIVERSE {
    acc += 1.0 / (rank as f64).powf(EXPONENT);
    cdf.push(acc);
  }
  let best_hit_ratio = cdf[CAPACITY as usize - 1] / acc;

  let mut perm: Vec<u64> = (0..UNIVERSE).collect();
  let mut x = 0x2545_F491_4F6C_DD1D;
  for i in (1..perm.len()).rev() {
    perm.swap(i, (xorshift(&mut x) % (i as u64 + 1)) as usize);
  }

  let keys = (0..OPS)
    .map(|_| {
      let u = (xorshift(&mut x) >> 11) as f64 / (1u64 << 53) as f64 * acc;
      let rank = cdf.partition_point(|&c| c < u).min(UNIVERSE as usize - 1);
      perm[rank]
    })
    .collect();
  Zipf { keys, best_hit_ratio }
}

pub const SHARDS: [usize; 3] = [1, 16, 128];
pub const CHANCES: [u32; 2] = [
  fibre_cache::builder::maintenance_frequency::RESPONSIVE,
  fibre_cache::builder::maintenance_frequency::LOW_OVERHEAD,
];
