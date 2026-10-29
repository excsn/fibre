use cache_arena::engines::builtins;
use cache_arena::harness::{Cell, Cfg};

#[test]
fn every_engine_runs_every_cell_it_supports() {
  for cell in ["hit sync 2", "insert sync 2", "zipf sync 2", "hit async 2", "insert async 2", "zipf async 2", "hit mixed 2", "insert mixed 2", "zipf mixed 2"] {
    for (engine, run) in builtins() {
      let cfg = Cfg { cell: Cell::parse(cell).unwrap(), capacity: 200, secs: 0.05 };
      let r = run(&cfg);
      let mixed = cell.contains("mixed");
      let Some(r) = r else {
        assert!(mixed && (engine == "moka" || engine == "stretto"), "{engine} {cell}: unsupported");
        continue;
      };
      assert!(r.total > 0, "{engine} {cell}: no operations");
      if !cell.starts_with("insert") {
        assert!(r.hits > 0, "{engine} {cell}: no hits");
      }
      assert!(r.settled_len > 0, "{engine} {cell}: empty after settle");
    }
  }
}
