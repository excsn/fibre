use generational_arena::{Arena, Index};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A timer entry stored in the wheel's arena.
/// It is part of a doubly-linked list within a specific wheel slot.
pub(crate) struct Timer {
  pub(crate) laps: usize,
  pub(crate) key_hash: u64,
  slot_index: usize,
  prev: Option<Index>,
  next: Option<Index>,
}

/// A single slot in the wheel, containing the head/tail of a linked list of timers.
#[derive(Default, Clone, Copy)]
struct Slot {
  head: Option<Index>,
  tail: Option<Index>,
}

/// A handle to a scheduled timer, allowing for O(1) cancellation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimerHandle {
  index: Index,
}

/// A shard's TTL timers. Owned by the shard map and changed only under its
/// write lock, so a timer is armed and disarmed in the same step as its entry.
pub(crate) struct TimerWheel {
  slots: Vec<Slot>,
  timers: Arena<Timer>,
  wheel_len: usize,
  current_tick: usize,
  tick_ns: u64,
  /// Wall clock (nanos since the cache epoch) up to which the wheel has been advanced.
  last_advanced: u64,
  /// Nanos since the cache epoch of the earliest slot holding a timer, `u64::MAX`
  /// when none does. Read without the shard lock to skip shards with nothing due.
  next_due: Arc<AtomicU64>,
}

impl TimerWheel {
  pub(crate) fn new(wheel_size: usize, tick_duration: Duration, next_due: Arc<AtomicU64>) -> Self {
    next_due.store(u64::MAX, Ordering::Relaxed);
    Self {
      slots: Vec::new(),
      timers: Arena::new(),
      wheel_len: wheel_size,
      current_tick: 0,
      tick_ns: (tick_duration.as_nanos() as u64).max(1),
      last_advanced: crate::time::now_duration().as_nanos() as u64,
      next_due,
    }
  }

  #[cfg(test)]
  pub(crate) fn len(&self) -> usize {
    self.timers.len()
  }

  /// Arms a timer that fires on the first advance at or after `expires_at`
  /// (nanos since the cache epoch).
  pub(crate) fn schedule(&mut self, key_hash: u64, expires_at: u64) -> TimerHandle {
    if self.slots.is_empty() {
      self.slots = vec![Slot::default(); self.wheel_len];
      self.last_advanced = crate::time::now_duration().as_nanos() as u64;
    }
    // The slot `offset` ahead of the current tick is swept once the wheel has
    // advanced `offset + 1` ticks, the first advance at or after `expires_at`.
    let ticks = (expires_at.saturating_sub(self.last_advanced).div_ceil(self.tick_ns) as usize).max(1);
    let offset = ticks - 1;
    let laps = offset / self.wheel_len;
    let slot = (self.current_tick + offset) % self.wheel_len;

    let index = self.timers.insert(Timer {
      laps,
      key_hash,
      slot_index: slot,
      prev: None,
      next: self.slots[slot].head,
    });
    if let Some(old_head) = self.slots[slot].head {
      self.timers[old_head].prev = Some(index);
    }
    self.slots[slot].head = Some(index);
    if self.slots[slot].tail.is_none() {
      self.slots[slot].tail = Some(index);
    }

    let due = self.last_advanced.saturating_add(ticks as u64 * self.tick_ns);
    self.next_due.fetch_min(due, Ordering::Relaxed);
    TimerHandle { index }
  }

  /// Removes a timer. A handle whose timer already fired is ignored.
  pub(crate) fn cancel(&mut self, handle: &TimerHandle) {
    if let Some(timer) = self.timers.remove(handle.index) {
      self.unlink(timer.slot_index, timer.prev, timer.next);
    }
  }

  pub(crate) fn is_armed(&self, handle: &TimerHandle) -> bool {
    self.timers.contains(handle.index)
  }

  pub(crate) fn clear(&mut self) {
    self.timers.clear();
    self.slots.fill(Slot::default());
    self.next_due.store(u64::MAX, Ordering::Relaxed);
  }

  fn unlink(&mut self, slot_index: usize, prev: Option<Index>, next: Option<Index>) {
    match prev {
      Some(p) => self.timers[p].next = next,
      None => self.slots[slot_index].head = next,
    }
    match next {
      Some(n) => self.timers[n].prev = prev,
      None => self.slots[slot_index].tail = prev,
    }
  }

  /// Moves the wheel to `now` (nanos since the cache epoch) and returns the key
  /// hashes of the timers that fired, catching up on every tick since the last call.
  pub(crate) fn advance(&mut self, now: u64) -> Vec<u64> {
    let wheel_len = self.wheel_len;
    let tick_ns = self.tick_ns;
    if self.slots.is_empty() {
      return Vec::new();
    }
    let ticks_due = (now.saturating_sub(self.last_advanced) / tick_ns) as usize;
    if ticks_due == 0 {
      return Vec::new();
    }
    // Advance by whole ticks only, keeping the remainder, so cadence never drifts.
    self.last_advanced += ticks_due as u64 * tick_ns;
    let start = self.current_tick;
    self.current_tick += ticks_due;

    let full_laps = ticks_due / wheel_len;
    let remainder = ticks_due % wheel_len;
    let start_slot = start % wheel_len;

    let mut expired_hashes = Vec::new();
    for slot_index in 0..wheel_len {
      let rel = (slot_index + wheel_len - start_slot) % wheel_len;
      let passes = full_laps + usize::from(rel < remainder);
      if passes == 0 {
        continue;
      }

      // A timer survives `passes` visits iff it has at least that many laps left.
      let mut current_opt = self.slots[slot_index].head;
      while let Some(current_index) = current_opt {
        let timer = &mut self.timers[current_index];
        current_opt = timer.next;
        if timer.laps >= passes {
          timer.laps -= passes;
        } else {
          let timer = self.timers.remove(current_index).expect("linked timer");
          expired_hashes.push(timer.key_hash);
          self.unlink(slot_index, timer.prev, timer.next);
        }
      }
    }

    let mut next = u64::MAX;
    for off in 0..wheel_len {
      if self.slots[(self.current_tick + off) % wheel_len].head.is_some() {
        next = self.last_advanced + (off as u64 + 1) * tick_ns;
        break;
      }
    }
    self.next_due.store(next, Ordering::Relaxed);

    expired_hashes
  }
}
