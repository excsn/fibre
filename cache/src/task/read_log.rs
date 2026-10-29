//! Per-shard log of the reads a key-based policy has not seen yet. Every hit
//! is kept, in order; producers push under the shard's read lock and the
//! shard's writer drains under its write lock, so the two never overlap. A full
//! log refuses the push and the hit path applies that read itself under the
//! write lock.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

const SLOTS: usize = 256;

struct Slot<K> {
  seq: AtomicUsize,
  val: UnsafeCell<MaybeUninit<(K, u64)>>,
}

pub(crate) struct ReadLog<K> {
  slots: Box<[Slot<K>]>,
  tail: AtomicUsize,
  head: AtomicUsize,
}

unsafe impl<K: Send> Send for ReadLog<K> {}
unsafe impl<K: Send> Sync for ReadLog<K> {}

impl<K> ReadLog<K> {
  pub(crate) fn new() -> Self {
    Self {
      slots: (0..SLOTS)
        .map(|i| Slot { seq: AtomicUsize::new(i), val: UnsafeCell::new(MaybeUninit::uninit()) })
        .collect(),
      tail: AtomicUsize::new(0),
      head: AtomicUsize::new(0),
    }
  }

  /// Appends a read. Returns false, cloning nothing, when the log is full.
  #[inline]
  pub(crate) fn push(&self, key: &K, cost: u64) -> bool
  where
    K: Clone,
  {
    let mut pos = self.tail.load(Ordering::Relaxed);
    loop {
      let slot = &self.slots[pos & (SLOTS - 1)];
      let seq = slot.seq.load(Ordering::Acquire);
      let diff = (seq as isize).wrapping_sub(pos as isize);
      if diff == 0 {
        match self.tail.compare_exchange_weak(pos, pos.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
          Ok(_) => {
            unsafe { (*slot.val.get()).write((key.clone(), cost)) };
            slot.seq.store(pos.wrapping_add(1), Ordering::Release);
            return true;
          }
          Err(p) => pos = p,
        }
      } else if diff < 0 {
        return false;
      } else {
        pos = self.tail.load(Ordering::Relaxed);
      }
    }
  }

  /// Hands every logged read to `f` in order. Only the shard's writer calls this.
  pub(crate) fn drain(&self, mut f: impl FnMut(K, u64)) {
    let mut pos = self.head.load(Ordering::Relaxed);
    loop {
      let slot = &self.slots[pos & (SLOTS - 1)];
      if slot.seq.load(Ordering::Acquire) != pos.wrapping_add(1) {
        break;
      }
      let (k, c) = unsafe { (*slot.val.get()).assume_init_read() };
      slot.seq.store(pos.wrapping_add(SLOTS), Ordering::Release);
      pos = pos.wrapping_add(1);
      f(k, c);
    }
    self.head.store(pos, Ordering::Relaxed);
  }
}

impl<K> Drop for ReadLog<K> {
  fn drop(&mut self) {
    self.drain(|_, _| {});
  }
}
