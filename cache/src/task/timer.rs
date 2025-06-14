use generational_arena::{Arena, Index};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
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

pub(crate) struct TimerWheel {
  wheel: Vec<Mutex<Slot>>,
  timers: Mutex<Arena<Timer>>,
  current_tick: AtomicUsize,
  tick_duration: Duration,
}

impl TimerWheel {
  pub(crate) fn new(wheel_size: usize, tick_duration: Duration) -> Self {
    let wheel = (0..wheel_size)
      .map(|_| Mutex::new(Slot::default()))
      .collect();

    Self {
      wheel,
      timers: Mutex::new(Arena::new()),
      current_tick: AtomicUsize::new(0),
      tick_duration,
    }
  }

  pub(crate) fn schedule(&self, key_hash: u64, duration: Duration) -> TimerHandle {
    let ticks = (duration.as_secs_f64() / self.tick_duration.as_secs_f64()).round() as usize;
    // Load the current tick atomically. Ordering::Relaxed is fine because we don't
    // need to synchronize memory with other operations; we just need the value.
    let current_tick = self.current_tick.load(Ordering::Relaxed);
    let laps = ticks / self.wheel.len();
    let slot = (current_tick + ticks) % self.wheel.len();

    let timer = Timer {
      laps,
      key_hash,
      slot_index: slot,
      prev: None,
      next: None,
    };

    let mut timers = self.timers.lock();
    let mut slot_guard = self.wheel[slot].lock();

    let index = timers.insert(timer);

    // Link at the head of the slot's list
    let old_head = slot_guard.head;
    if let Some(old_head_index) = old_head {
      timers[old_head_index].prev = Some(index);
    }
    timers[index].next = old_head;
    slot_guard.head = Some(index);

    if slot_guard.tail.is_none() {
      slot_guard.tail = Some(index);
    }

    TimerHandle { index }
  }

  pub(crate) fn cancel(&self, handle: &TimerHandle) {
    // This is now an O(1) operation.
    let mut timers = self.timers.lock();

    // Check if the timer still exists before trying to remove it.
    if let Some(timer) = timers.get(handle.index) {
      let slot_index = timer.slot_index;
      let prev_index = timer.prev;
      let next_index = timer.next;

      // Lock the specific slot to update its head/tail pointers.
      let mut slot_guard = self.wheel[slot_index].lock();

      // Unlink the timer from the list.
      if let Some(p) = prev_index {
        timers[p].next = next_index;
      } else {
        // It was the head of the list.
        slot_guard.head = next_index;
      }

      if let Some(n) = next_index {
        timers[n].prev = prev_index;
      } else {
        // It was the tail of the list.
        slot_guard.tail = prev_index;
      }

      // Finally, remove the timer from the arena.
      timers.remove(handle.index);
    }
  }

  pub(crate) fn advance(&self) -> Vec<u64> {
    let tick_to_process = self.current_tick.fetch_add(1, Ordering::Relaxed);
    let slot_index = tick_to_process % self.wheel.len();

    let mut timers = self.timers.lock();
    let mut slot = self.wheel[slot_index].lock();

    let mut expired_hashes = Vec::new();
    let mut current_opt = slot.head;
    let mut to_remove = Vec::new();

    // First pass: identify timers to remove and update laps.
    while let Some(current_index) = current_opt {
      let timer = &mut timers[current_index];
      if timer.laps > 0 {
        timer.laps -= 1;
        current_opt = timer.next;
      } else {
        // Expired. Mark for removal.
        expired_hashes.push(timer.key_hash);
        to_remove.push(current_index);
        current_opt = timer.next;
      }
    }

    // Second pass: remove the expired timers from the list and arena.
    for index_to_remove in to_remove {
      let timer = &timers[index_to_remove];
      let prev = timer.prev;
      let next = timer.next;

      if let Some(p) = prev {
        timers[p].next = next;
      } else {
        slot.head = next;
      }
      if let Some(n) = next {
        timers[n].prev = prev;
      } else {
        slot.tail = prev;
      }
      timers.remove(index_to_remove);
    }

    expired_hashes
  }
}
