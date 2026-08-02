use crate::spec::{Api, Capacity, Cell, Flavor, Mode, Pairing, Stage};

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

/// A oneshot carries one value and is then spent, so capacity, pairing and
/// batching have a single legal answer each and the stage is the only axis
/// left to vary.
fn oneshot_cells(out: &mut Vec<Cell>) {
  for mode in Mode::ALL {
    for stage in Stage::ONESHOT {
      out.push(Cell {
        flavor: Flavor::Oneshot,
        mode,
        capacity: Capacity::Bounded(1),
        pairing: Pairing {
          producers: 1,
          consumers: 1,
        },
        api: Api::Single,
        stage,
      });
    }
  }
}

pub fn cells(batch_size: Option<usize>) -> Vec<Cell> {
  let mut out = Vec::new();
  for flavor in Flavor::ALL {
    if flavor == Flavor::Oneshot {
      oneshot_cells(&mut out);
      continue;
    }
    for mode in Mode::ALL {
      for capacity in Capacity::ALL {
        for pairing in pairings(flavor) {
          out.push(Cell {
            flavor,
            mode,
            capacity,
            pairing,
            api: Api::Single,
            stage: Stage::Stream,
          });
          if let Some(size) = batch_size {
            if capacity.worth_batching() {
              out.push(Cell {
                flavor,
                mode,
                capacity,
                pairing,
                api: Api::Batch(size),
                stage: Stage::Stream,
              });
            }
          }
        }
      }
    }
  }
  out
}
