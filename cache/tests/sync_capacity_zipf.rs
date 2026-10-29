mod zipf;

use fibre_cache::CacheBuilder;
use zipf::{CAPACITY, CHANCES, SHARDS};

#[test]
fn zipf_load_holds_capacity_after_maintenance() {
  let z = zipf::stream();
  let mut failures = Vec::new();
  for shards in SHARDS {
    for chance in CHANCES {
      let cache = CacheBuilder::<u64, u64>::new()
        .capacity(CAPACITY)
        .shards(shards)
        .maintenance_chance(chance)
        .build()
        .unwrap();

      let mut hits = 0u64;
      for &k in &z.keys {
        if cache.get(&k, |_| ()).is_some() {
          hits += 1;
        } else {
          cache.insert(k, k, 1);
        }
      }
      cache.run_maintenance();

      let m = cache.metrics();
      let held = cache.iter().count() as u64;
      let hit_ratio = hits as f64 / z.keys.len() as f64;
      let label = format!("shards {shards}, maintenance 1/{chance}");
      if m.current_cost > CAPACITY {
        failures.push(format!("{label}: cost {} over capacity {CAPACITY}", m.current_cost));
      }
      if held > CAPACITY {
        failures.push(format!("{label}: {held} entries over capacity {CAPACITY}"));
      }
      if hit_ratio > z.best_hit_ratio + 0.02 {
        failures.push(format!(
          "{label}: hit ratio {hit_ratio:.4} above the {:.4} a cache of {CAPACITY} can reach",
          z.best_hit_ratio
        ));
      }
    }
  }
  assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
