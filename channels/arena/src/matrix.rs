use crate::spec::{Api, Capacity, Cell, Flavor, Mode, Pairing};

/// Pairings for a shape: the requested loads, each clamped to the shape's
/// limits, deduplicated. SPSC collapses all seven to 1x1; only MPMC keeps them
/// all distinct.
pub fn pairings(flavor: Flavor) -> Vec<Pairing> {
  let mut out: Vec<Pairing> = Vec::new();
  for requested in Pairing::REQUESTED {
    let pairing = Pairing::clamped(requested, flavor);
    if !out.contains(&pairing) {
      out.push(pairing);
    }
  }
  out
}

pub fn cells(batch_size: Option<usize>) -> Vec<Cell> {
  let mut out = Vec::new();
  for flavor in Flavor::ALL {
    for mode in Mode::ALL {
      for capacity in Capacity::ALL {
        for pairing in pairings(flavor) {
          out.push(Cell {
            flavor,
            mode,
            capacity,
            pairing,
            api: Api::Single,
          });
          if let Some(size) = batch_size {
            if capacity.worth_batching() {
              out.push(Cell {
                flavor,
                mode,
                capacity,
                pairing,
                api: Api::Batch(size),
              });
            }
          }
        }
      }
    }
  }
  out
}
