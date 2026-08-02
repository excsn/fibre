use std::fmt;

/// The workload shape, not the channel's maximum capability. A general-purpose
/// MPMC library is measured under the SPSC, MPSC and MPMC shapes alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flavor {
  Spsc,
  Mpsc,
  Spmc,
  Mpmc,
  Oneshot,
}

impl Flavor {
  pub const ALL: [Flavor; 5] = [
    Flavor::Spsc,
    Flavor::Mpsc,
    Flavor::Spmc,
    Flavor::Mpmc,
    Flavor::Oneshot,
  ];

  pub fn as_str(&self) -> &'static str {
    match self {
      Flavor::Spsc => "spsc",
      Flavor::Mpsc => "mpsc",
      Flavor::Spmc => "spmc",
      Flavor::Mpmc => "mpmc",
      Flavor::Oneshot => "oneshot",
    }
  }

  pub fn parse(s: &str) -> Option<Flavor> {
    Flavor::ALL.into_iter().find(|f| f.as_str() == s)
  }

  pub fn max_producers(&self) -> Option<usize> {
    match self {
      Flavor::Spsc | Flavor::Spmc | Flavor::Oneshot => Some(1),
      _ => None,
    }
  }

  pub fn max_consumers(&self) -> Option<usize> {
    match self {
      Flavor::Spsc | Flavor::Mpsc | Flavor::Oneshot => Some(1),
      _ => None,
    }
  }

  /// SPMC is a broadcast fan-out: every consumer receives every item, so the
  /// expected receive count is `sent * consumers` rather than `sent`.
  pub fn semantics(&self) -> Semantics {
    match self {
      Flavor::Spmc => Semantics::Broadcast,
      _ => Semantics::WorkSharing,
    }
  }
}

impl fmt::Display for Flavor {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantics {
  WorkSharing,
  Broadcast,
}

/// What one op is. The streaming flavors push an item through a channel that
/// already exists, so there is only ever one answer. A oneshot needs its own
/// channel per op, which makes construction part of the workload rather than
/// setup, so it is measured both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
  Stream,
  Cycle,
  Handoff,
}

impl Stage {
  pub const ONESHOT: [Stage; 2] = [Stage::Cycle, Stage::Handoff];

  pub fn as_str(&self) -> &'static str {
    match self {
      Stage::Stream => "stream",
      Stage::Cycle => "full-cycle",
      Stage::Handoff => "handoff",
    }
  }

  pub fn parse(s: &str) -> Option<Stage> {
    [Stage::Stream, Stage::Cycle, Stage::Handoff]
      .into_iter()
      .find(|stage| stage.as_str() == s)
  }
}

impl fmt::Display for Stage {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
  Sync,
  Async,
}

impl Mode {
  pub const ALL: [Mode; 2] = [Mode::Sync, Mode::Async];

  pub fn as_str(&self) -> &'static str {
    match self {
      Mode::Sync => "sync",
      Mode::Async => "async",
    }
  }

  pub fn parse(s: &str) -> Option<Mode> {
    Mode::ALL.into_iter().find(|m| m.as_str() == s)
  }
}

impl fmt::Display for Mode {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

/// Ordering is the table's row order: rendezvous, then ascending bounds, then
/// unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capacity {
  Rendezvous,
  Bounded(usize),
  Unbounded,
}

impl Capacity {
  pub const ALL: [Capacity; 5] = [
    Capacity::Rendezvous,
    Capacity::Bounded(1),
    Capacity::Bounded(128),
    Capacity::Bounded(1024),
    Capacity::Unbounded,
  ];

  pub fn as_str(&self) -> String {
    match self {
      Capacity::Rendezvous => "rendezvous".to_string(),
      Capacity::Bounded(n) => n.to_string(),
      Capacity::Unbounded => "unbounded".to_string(),
    }
  }

  pub fn parse(s: &str) -> Option<Capacity> {
    match s {
      "rendezvous" | "0" => Some(Capacity::Rendezvous),
      "unbounded" => Some(Capacity::Unbounded),
      other => other.parse().ok().map(Capacity::Bounded),
    }
  }

  /// Batching into a channel that holds one item, or none, measures the
  /// blocking handshake rather than the batch path.
  pub fn worth_batching(&self) -> bool {
    matches!(self, Capacity::Bounded(n) if *n >= 128) || matches!(self, Capacity::Unbounded)
  }
}

impl fmt::Display for Capacity {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.as_str())
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pairing {
  pub producers: usize,
  pub consumers: usize,
}

impl Pairing {
  /// Requested loads: symmetric first, then skewed. Each is clamped to what the
  /// shape allows, so 16x16 becomes 16x1 under MPSC and 1x16 under SPMC, and
  /// the skewed entries collapse into those same rows. Only MPMC, which
  /// constrains neither side, sees all seven as distinct.
  pub const REQUESTED: [(usize, usize); 7] = [
    (1, 1),
    (16, 16),
    (64, 64),
    (16, 1),
    (1, 16),
    (64, 1),
    (1, 64),
  ];

  pub fn clamped((producers, consumers): (usize, usize), flavor: Flavor) -> Pairing {
    Pairing {
      producers: flavor.max_producers().map_or(producers, |m| m.min(producers)),
      consumers: flavor.max_consumers().map_or(consumers, |m| m.min(consumers)),
    }
  }

  pub fn label(&self) -> String {
    format!("{}x{}", self.producers, self.consumers)
  }

  pub fn threads(&self) -> usize {
    self.producers + self.consumers
  }
}

impl fmt::Display for Pairing {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.label())
  }
}

/// Whether items move one at a time or in chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Api {
  Single,
  Batch(usize),
}

impl Api {
  pub fn as_str(&self) -> String {
    match self {
      Api::Single => "single".to_string(),
      Api::Batch(n) => format!("batch-{}", n),
    }
  }

  pub fn batch_size(&self) -> usize {
    match self {
      Api::Single => 1,
      Api::Batch(n) => *n,
    }
  }

  pub fn parse(s: &str) -> Option<Api> {
    match s {
      "single" => Some(Api::Single),
      other => other.strip_prefix("batch-")?.parse().ok().map(Api::Batch),
    }
  }
}

impl fmt::Display for Api {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.as_str())
  }
}

/// What an implementation offers natively, as opposed to what the harness
/// emulates by looping single-item calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchSupport {
  None,
  Recv,
  SendRecv,
}

impl BatchSupport {
  pub fn label(&self) -> &'static str {
    match self {
      BatchSupport::None => "none",
      BatchSupport::Recv => "receive only",
      BatchSupport::SendRecv => "send and receive",
    }
  }

  pub fn parse(s: &str) -> Option<BatchSupport> {
    [BatchSupport::None, BatchSupport::Recv, BatchSupport::SendRecv]
      .into_iter()
      .find(|support| support.label() == s)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
  pub flavor: Flavor,
  pub mode: Mode,
  pub capacity: Capacity,
  pub pairing: Pairing,
  pub api: Api,
  pub stage: Stage,
}

impl Cell {
  /// The single-item cell this one should be compared against.
  pub fn single(&self) -> Cell {
    Cell {
      api: Api::Single,
      ..*self
    }
  }
}

impl fmt::Display for Cell {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self.stage {
      Stage::Stream => write!(
        f,
        "{}/{}/cap-{}/{}/{}",
        self.flavor, self.mode, self.capacity, self.pairing, self.api
      ),
      stage => write!(f, "{}/{}/{}", self.flavor, self.mode, stage),
    }
  }
}
