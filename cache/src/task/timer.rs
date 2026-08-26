use generational_arena::{Arena, Index};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

/// A timer entry stored in the shared Arena.
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
/// It holds an index into the `TimerWheel`'s internal arena.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimerHandle {
  index: Index,
}

/// Slots and arena live under one mutex: every operation needs the arena, so
/// separate slot locks only added a second acquire to the same critical section.
#[derive(Default)]
struct WheelInner {
  slots: Vec<Slot>,
  timers: Arena<Timer>,
}

#[derive(Default)]
pub(crate) struct TimerWheel {
  inner: Mutex<WheelInner>,
  wheel_len: usize,
  current_tick: AtomicUsize,
  tick_duration: Duration,
  /// Wall clock (nanos since the cache epoch) up to which the wheel has been advanced.
  /// The wheel's tick count is call-driven, so this is what anchors it to real time:
  /// `advance()` catches up on however many ticks have elapsed since it was last called.
  last_advanced: AtomicU64,
}

impl TimerWheel {
  pub(crate) fn new(wheel_size: usize, tick_duration: Duration) -> Self {
    Self {
      inner: Mutex::new(WheelInner {
        slots: vec![Slot::default(); wheel_size],
        timers: Arena::new(),
      }),
      wheel_len: wheel_size,
      current_tick: AtomicUsize::new(0),
      tick_duration,
      last_advanced: AtomicU64::new(crate::time::now_duration().as_nanos() as u64),
    }
  }

  pub(crate) fn schedule(&self, key_hash: u64, duration: Duration) -> TimerHandle {
    // Ceiling plus one guard tick: `current_tick` may lag wall time by up to a full
    // tick, so rounding down (or to nearest) could fire before the entry's expires_at.
    // Late by up to two ticks is fine; early would evict a live entry.
    let tick_ns = self.tick_duration.as_nanos().max(1);
    let ticks = (duration.as_nanos().div_ceil(tick_ns) as usize).saturating_add(1);
    let current_tick = self.current_tick.load(Ordering::Relaxed);
    let laps = ticks / self.wheel_len;
    let slot = (current_tick + ticks) % self.wheel_len;

    let timer = Timer {
      laps,
      key_hash,
      slot_index: slot,
      prev: None,
      next: None,
    };

    let mut inner = self.inner.lock();
    let index = inner.timers.insert(timer);

    // Link at the head of the slot's list
    let old_head = inner.slots[slot].head;
    if let Some(old_head_index) = old_head {
      inner.timers[old_head_index].prev = Some(index);
    }
    inner.timers[index].next = old_head;
    inner.slots[slot].head = Some(index);

    if inner.slots[slot].tail.is_none() {
      inner.slots[slot].tail = Some(index);
    }

    TimerHandle { index }
  }

  pub(crate) fn cancel(&self, handle: &TimerHandle) {
    // This is now an O(1) operation.
    let mut inner = self.inner.lock();

    // Check if the timer still exists before trying to remove it.
    if let Some(timer) = inner.timers.get(handle.index) {
      let slot_index = timer.slot_index;
      let prev_index = timer.prev;
      let next_index = timer.next;

      // Unlink the timer from the list.
      if let Some(p) = prev_index {
        inner.timers[p].next = next_index;
      } else {
        // It was the head of the list.
        inner.slots[slot_index].head = next_index;
      }

      if let Some(n) = next_index {
        inner.timers[n].prev = prev_index;
      } else {
        // It was the tail of the list.
        inner.slots[slot_index].tail = prev_index;
      }

      // Finally, remove the timer from the arena.
      inner.timers.remove(handle.index);
    }
  }

  pub(crate) fn advance(&self) -> Vec<u64> {
    let wheel_len = self.wheel_len;
    let tick_ns = self.tick_duration.as_nanos() as u64;
    if wheel_len == 0 || tick_ns == 0 {
      return Vec::new();
    }

    let now = crate::time::now_duration().as_nanos() as u64;
    let last = self.last_advanced.load(Ordering::Relaxed);
    let ticks_due = (now.saturating_sub(last) / tick_ns) as usize;
    if ticks_due == 0 {
      return Vec::new();
    }
    // Advance by whole ticks only, keeping the remainder, so cadence never drifts.
    self
      .last_advanced
      .store(last + ticks_due as u64 * tick_ns, Ordering::Relaxed);
    let start = self.current_tick.fetch_add(ticks_due, Ordering::Relaxed);

    // Holding the lock across all slots serializes against schedule() and cancel(),
    // so a timer placed relative to the advanced tick cannot be swept by the
    // catch-up pass that advanced it.
    let full_laps = ticks_due / wheel_len;
    let remainder = ticks_due % wheel_len;
    let start_slot = start % wheel_len;

    let mut expired_hashes = Vec::new();
    let mut inner = self.inner.lock();

    for slot_index in 0..wheel_len {
      let rel = (slot_index + wheel_len - start_slot) % wheel_len;
      let passes = full_laps + usize::from(rel < remainder);
      if passes == 0 {
        continue;
      }

      // A timer survives `passes` visits iff it has at least that many laps left.
      let mut current_opt = inner.slots[slot_index].head;
      let mut to_remove = Vec::new();
      while let Some(current_index) = current_opt {
        let timer = &mut inner.timers[current_index];
        if timer.laps >= passes {
          timer.laps -= passes;
          current_opt = timer.next;
        } else {
          expired_hashes.push(timer.key_hash);
          to_remove.push(current_index);
          current_opt = timer.next;
        }
      }

      for index_to_remove in to_remove {
        let timer = &inner.timers[index_to_remove];
        let prev = timer.prev;
        let next = timer.next;

        if let Some(p) = prev {
          inner.timers[p].next = next;
        } else {
          inner.slots[slot_index].head = next;
        }
        if let Some(n) = next {
          inner.timers[n].prev = prev;
        } else {
          inner.slots[slot_index].tail = prev;
        }
        inner.timers.remove(index_to_remove);
      }
    }

    expired_hashes
  }
}
