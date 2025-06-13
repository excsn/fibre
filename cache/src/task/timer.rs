use parking_lot::Mutex;
use std::collections::LinkedList;
use std::sync::{Arc, Weak};
use std::time::Duration;

// A node in the timer wheel's linked list.
pub(crate) struct Timer {
  pub(crate) laps: usize,
  pub(crate) key_hash: u64,
}

pub(crate) type TimerHandle = u64; // The handle is now just the key's hash.

pub(crate) struct TimerWheel {
  wheel: Vec<Mutex<LinkedList<Timer>>>,
  current_tick: usize,
  tick_duration: Duration,
}

impl TimerWheel {
  pub(crate) fn new(wheel_size: usize, tick_duration: Duration) -> Self {
    let mut wheel = Vec::with_capacity(wheel_size);
    for _ in 0..wheel_size {
      wheel.push(Mutex::new(LinkedList::new()));
    }
    Self {
      wheel,
      current_tick: 0,
      tick_duration,
    }
  }

  pub(crate) fn schedule(&self, key_hash: u64, duration: Duration) -> TimerHandle {
    let ticks = (duration.as_secs_f64() / self.tick_duration.as_secs_f64()).round() as usize;
    let laps = ticks / self.wheel.len();
    let slot = (self.current_tick + ticks) % self.wheel.len();

    let timer = Timer { laps, key_hash };
    self.wheel[slot].lock().push_back(timer);
    key_hash
  }

  pub(crate) fn cancel(&self, handle: &TimerHandle) {
    // This is O(N) where N is the number of timers in the target bucket.
    // A more complex implementation with an intrusive list would be O(1).
    // This is an acceptable trade-off for simplicity.
    for bucket in self.wheel.iter() {
      let mut list = bucket.lock();
      if let Some(pos) = list.iter().position(|t| t.key_hash == *handle) {
        // `drain_filter` is unstable, so we do it manually.
        let mut rest = list.split_off(pos);
        rest.pop_front(); // Remove the element at the found position.
        list.append(&mut rest); // Append the rest of the list back.
        return; // Assume hashes are unique enough for one cancellation.
      }
    }
  }

  pub(crate) fn advance(&mut self) -> Vec<u64> {
    self.current_tick = (self.current_tick + 1) % self.wheel.len();
    let mut current_bucket = self.wheel[self.current_tick].lock();

    let mut expired_hashes = Vec::new();
    let mut still_running = LinkedList::new();

    while let Some(mut timer) = current_bucket.pop_front() {
      if timer.laps > 0 {
        timer.laps -= 1;
        still_running.push_back(timer);
      } else {
        expired_hashes.push(timer.key_hash);
      }
    }

    *current_bucket = still_running;
    expired_hashes
  }
}
