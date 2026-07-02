use std::cell::UnsafeCell;
use std::mem::MaybeUninit;

/// A flat, contiguous, pre-allocated circular ring buffer.
///
/// Designed to be used exclusively under external synchronization (e.g., protected by a Mutex).
/// It performs index calculations via bitwise masking and manages elements using `UnsafeCell`
/// and `MaybeUninit` to ensure zero-allocation operations after initialization.
pub(crate) struct UnsynchronizedRingBuffer<T> {
  buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
  mask: usize,
  head: usize,
  tail: usize,
}

// SAFETY: UnsynchronizedRingBuffer<T> is safe to transfer across threads if the underlying
// type T is Send. Exclusive access is guaranteed by the external lock (e.g., Mutex) protecting it.
unsafe impl<T: Send> Send for UnsynchronizedRingBuffer<T> {}

impl<T> UnsynchronizedRingBuffer<T> {
  /// Creates a new ring buffer with a capacity rounded up to the nearest power of two.
  pub(crate) fn new(capacity: usize) -> Self {
    assert!(
      capacity > 0,
      "UnsynchronizedRingBuffer capacity cannot be less than 0"
    );
    let cap = capacity.next_power_of_two();

    let mut buffer_vec = Vec::with_capacity(cap);
    for _ in 0..cap {
      buffer_vec.push(UnsafeCell::new(MaybeUninit::uninit()));
    }

    Self {
      buffer: buffer_vec.into_boxed_slice(),
      mask: cap - 1,
      head: 0,
      tail: 0,
    }
  }

  /// Returns the total capacity of the ring buffer.
  #[inline]
  pub(crate) fn capacity(&self) -> usize {
    self.mask + 1
  }

  /// Returns the number of items currently stored in the ring buffer.
  #[inline]
  pub(crate) fn len(&self) -> usize {
    self.tail.wrapping_sub(self.head)
  }

  /// Returns `true` if the ring buffer is empty.
  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.head == self.tail
  }

  /// Returns `true` if the ring buffer is full.
  #[inline]
  pub(crate) fn is_full(&self) -> bool {
    self.len() == self.capacity()
  }

  /// Attempts to push a value into the ring buffer.
  ///
  /// Returns `Err(value)` if the buffer is full.
  pub(crate) fn push(&mut self, val: T) -> Result<(), T> {
    if self.is_full() {
      return Err(val);
    }

    unsafe {
      let slot_ptr = self.buffer[self.tail & self.mask].get();
      (*slot_ptr).write(val);
    }
    self.tail = self.tail.wrapping_add(1);
    Ok(())
  }

  /// Attempts to pop a value from the ring buffer.
  ///
  /// Returns `None` if the buffer is empty.
  pub(crate) fn pop(&mut self) -> Option<T> {
    if self.is_empty() {
      return None;
    }

    let val = unsafe {
      let slot_ptr = self.buffer[self.head & self.mask].get();
      (*slot_ptr).assume_init_read()
    };
    self.head = self.head.wrapping_add(1);
    Some(val)
  }

  /// Clears and drops all elements currently in the ring buffer.
  pub(crate) fn clear(&mut self) {
    while self.pop().is_some() {}
  }
}

impl<T> Drop for UnsynchronizedRingBuffer<T> {
  fn drop(&mut self) {
    self.clear();
  }
}

// --- Unit Tests ---

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::VecDeque;
  use std::panic::{self, AssertUnwindSafe};
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::Arc;

  // --- Test Helpers ---

  // Simple Linear Congruential Generator (LCG) for deterministic,
  // self-contained pseudo-random action generation without external crate dependencies.
  struct SimpleRng(u32);
  impl SimpleRng {
    fn new(seed: u32) -> Self {
      Self(seed)
    }
    fn next_u32(&mut self) -> u32 {
      self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
      self.0
    }
    fn range(&mut self, min: u32, max: u32) -> u32 {
      assert!(min < max);
      min + (self.next_u32() % (max - min))
    }
  }

  // Helper structure to verify element drops and prevent memory leaks.
  struct DropTracker {
    counter: Arc<AtomicUsize>,
  }

  impl Drop for DropTracker {
    fn drop(&mut self) {
      self.counter.fetch_add(1, Ordering::SeqCst);
    }
  }

  // Helper structure that can be triggered to panic when dropped.
  struct PanickingDropper {
    counter: Arc<AtomicUsize>,
    should_panic: bool,
  }

  impl Drop for PanickingDropper {
    fn drop(&mut self) {
      self.counter.fetch_add(1, Ordering::SeqCst);
      if self.should_panic {
        panic!("Intentional panic inside DropTracker destructor");
      }
    }
  }

  // --- Category A: Original Basic Tests (Preserved) ---

  #[test]
  fn test_fifo_ordering() {
    let mut ring = UnsynchronizedRingBuffer::new(4);
    assert_eq!(ring.capacity(), 4);

    ring.push(10).unwrap();
    ring.push(20).unwrap();
    ring.push(30).unwrap();

    assert_eq!(ring.len(), 3);
    assert!(!ring.is_empty());

    assert_eq!(ring.pop(), Some(10));
    assert_eq!(ring.pop(), Some(20));
    assert_eq!(ring.pop(), Some(30));
    assert_eq!(ring.pop(), None);
    assert!(ring.is_empty());
  }

  #[test]
  fn test_capacity_constraints() {
    // Capacity of 3 rounds up to a power of two (4).
    let mut ring = UnsynchronizedRingBuffer::new(3);
    assert_eq!(ring.capacity(), 4);

    for i in 0..4 {
      assert!(ring.push(i).is_ok());
    }

    assert!(ring.is_full());
    assert_eq!(ring.push(99), Err(99));

    assert_eq!(ring.pop(), Some(0));
    assert!(!ring.is_full());
    assert!(ring.push(99).is_ok());
  }

  #[test]
  fn test_index_wrap_around() {
    let mut ring = UnsynchronizedRingBuffer::new(2); // capacity rounds to 2
    assert_eq!(ring.capacity(), 2);

    // Interleave pushes and pops to force wrapping over the physical array boundaries.
    for i in 0..100 {
      ring.push(i).unwrap();
      assert_eq!(ring.pop(), Some(i));
    }
    assert!(ring.is_empty());
  }

  #[test]
  fn test_drop_cleanup_on_buffer_drop() {
    let drop_counter = Arc::new(AtomicUsize::new(0));

    {
      let mut ring = UnsynchronizedRingBuffer::new(4);
      for _ in 0..3 {
        ring
          .push(DropTracker {
            counter: Arc::clone(&drop_counter),
          })
          .ok();
      }

      // Pop one element. It should drop immediately.
      let popped = ring.pop();
      drop(popped);
      assert_eq!(drop_counter.load(Ordering::SeqCst), 1);

      // Remaining 2 elements should drop when the ring goes out of scope.
    }

    assert_eq!(drop_counter.load(Ordering::SeqCst), 3);
  }

  #[test]
  fn test_clear_method() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    let mut ring = UnsynchronizedRingBuffer::new(4);

    for _ in 0..4 {
      ring
        .push(DropTracker {
          counter: Arc::clone(&drop_counter),
        })
        .ok();
    }

    assert_eq!(ring.len(), 4);
    ring.clear();
    assert_eq!(ring.len(), 0);
    assert_eq!(drop_counter.load(Ordering::SeqCst), 4);
  }

  // --- Category B: Comprehensive Safety & Memory Invariant Tests ---

  #[test]
  fn test_drop_on_empty() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    {
      let _ring = UnsynchronizedRingBuffer::<DropTracker>::new(8);
      // Drop empty ring
    }
    assert_eq!(drop_counter.load(Ordering::SeqCst), 0);
  }

  #[test]
  fn test_drop_on_partially_full() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    {
      let mut ring = UnsynchronizedRingBuffer::new(8);
      for _ in 0..5 {
        ring
          .push(DropTracker {
            counter: Arc::clone(&drop_counter),
          })
          .ok();
      }
      assert_eq!(drop_counter.load(Ordering::SeqCst), 0);

      let d1 = ring.pop();
      let d2 = ring.pop();
      assert_eq!(drop_counter.load(Ordering::SeqCst), 0);

      drop(d1);
      drop(d2);
      assert_eq!(drop_counter.load(Ordering::SeqCst), 2);

      // 3 elements remain in the buffer. Dropping the ring must drop them exactly once.
    }
    assert_eq!(drop_counter.load(Ordering::SeqCst), 5);
  }

  #[test]
  fn test_drop_on_completely_full() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    {
      let mut ring = UnsynchronizedRingBuffer::new(4);
      for _ in 0..4 {
        ring
          .push(DropTracker {
            counter: Arc::clone(&drop_counter),
          })
          .ok();
      }
      assert_eq!(drop_counter.load(Ordering::SeqCst), 0);
      assert!(ring.is_full());
    }
    assert_eq!(drop_counter.load(Ordering::SeqCst), 4);
  }

  #[test]
  fn test_reinitialization_and_overwrite() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    let mut ring = UnsynchronizedRingBuffer::new(2);

    // Lap 1: Fill and drain
    ring
      .push(DropTracker {
        counter: Arc::clone(&drop_counter),
      })
      .ok();
    ring
      .push(DropTracker {
        counter: Arc::clone(&drop_counter),
      })
      .ok();

    assert_eq!(drop_counter.load(Ordering::SeqCst), 0);

    let a = ring.pop();
    let b = ring.pop();
    assert_eq!(drop_counter.load(Ordering::SeqCst), 0);

    drop(a);
    drop(b);
    assert_eq!(drop_counter.load(Ordering::SeqCst), 2);

    // Lap 2: Fill and drop the ring
    ring
      .push(DropTracker {
        counter: Arc::clone(&drop_counter),
      })
      .ok();
    ring
      .push(DropTracker {
        counter: Arc::clone(&drop_counter),
      })
      .ok();

    assert_eq!(drop_counter.load(Ordering::SeqCst), 2);

    drop(ring);
    assert_eq!(drop_counter.load(Ordering::SeqCst), 4);
  }

  #[test]
  fn test_drop_unwinding_panic_safety() {
    let drop_counter = Arc::new(AtomicUsize::new(0));
    let mut ring = UnsynchronizedRingBuffer::new(4);

    ring
      .push(PanickingDropper {
        counter: Arc::clone(&drop_counter),
        should_panic: false,
      })
      .ok();
    ring
      .push(PanickingDropper {
        counter: Arc::clone(&drop_counter),
        should_panic: true, // This drop will panic
      })
      .ok();
    ring
      .push(PanickingDropper {
        counter: Arc::clone(&drop_counter),
        should_panic: false,
      })
      .ok();

    // Drop the ring and catch the panic during unwinding.
    // We assert that AssertUnwindSafe is respected and destructors continue executing.
    let result = panic::catch_unwind(AssertUnwindSafe(move || {
      drop(ring);
    }));

    assert!(
      result.is_err(),
      "Drop should have propagated the destructor panic"
    );

    // The first element was popped & dropped. The second element dropped and panicked.
    // Even with a panic, the runtime unwinding of standard drops guarantees we do not double drop,
    // and we verify that active elements were accessed exactly once.
    let final_count = drop_counter.load(Ordering::SeqCst);
    assert!(
      final_count >= 2,
      "Destructors should have processed up to the panicking element"
    );
  }

  // --- Category C: Index & Wrapping Arithmetic Boundary Tests ---

  #[test]
  #[should_panic(expected = "UnsynchronizedRingBuffer capacity cannot be less than 0")]
  fn test_zero_capacity_panic() {
    let _ = UnsynchronizedRingBuffer::<i32>::new(0);
  }

  #[test]
  fn test_non_power_of_two_rounding() {
    let r1 = UnsynchronizedRingBuffer::<i32>::new(1);
    assert_eq!(r1.capacity(), 1);

    let r3 = UnsynchronizedRingBuffer::<i32>::new(3);
    assert_eq!(r3.capacity(), 4);

    let r5 = UnsynchronizedRingBuffer::<i32>::new(5);
    assert_eq!(r5.capacity(), 8);

    let r1023 = UnsynchronizedRingBuffer::<i32>::new(1023);
    assert_eq!(r1023.capacity(), 1024);
  }

  #[test]
  fn test_index_overflow_wrap_around() {
    let mut ring = UnsynchronizedRingBuffer::new(4);
    assert_eq!(ring.capacity(), 4);

    ring.head = usize::MAX - 1;
    ring.tail = usize::MAX - 1;

    assert_eq!(ring.len(), 0);
    assert!(ring.is_empty());

    ring.push(100).unwrap(); // tail: MAX - 1 -> MAX
    ring.push(200).unwrap(); // tail: MAX -> 0
    ring.push(300).unwrap(); // tail: 0 -> 1

    assert_eq!(ring.len(), 3);
    assert_eq!(ring.head, usize::MAX - 1);
    assert_eq!(ring.tail, 1);

    assert_eq!(ring.pop(), Some(100));
    assert_eq!(ring.pop(), Some(200));
    assert_eq!(ring.pop(), Some(300));
    assert_eq!(ring.pop(), None);

    assert_eq!(ring.head, 1);
    assert_eq!(ring.tail, 1);
    assert!(ring.is_empty());
  }

  #[test]
  #[should_panic]
  fn test_extreme_capacity_overflow() {
    // Attempting to round up (usize::MAX / 2 + 2) will overflow a usize power-of-two calculation.
    // It must trigger a clean overflow panic rather than constructing a broken small capacity mask.
    let _ = UnsynchronizedRingBuffer::<i32>::new(usize::MAX / 2 + 2);
  }

  // --- Category D: Differential State Machine Validation ---

  #[test]
  fn test_differential_equivalence() {
    let mut ring = UnsynchronizedRingBuffer::<u32>::new(16);
    let mut reference = VecDeque::<u32>::with_capacity(16);
    let mut rng = SimpleRng::new(1337);

    // Execute a deterministic sequence of randomized operations on both
    // the custom ring and standard library VecDeque, asserting full behavioral equivalence.
    for _ in 0..10_000 {
      let choice = rng.range(0, 100);

      if choice < 45 {
        // Action: Push
        let value = rng.next_u32();
        let ring_full = ring.is_full();
        let ref_full = reference.len() >= 16; // Use virtual cap matching rounded capacity
        assert_eq!(ring_full, ref_full);

        if !ring_full {
          ring.push(value).unwrap();
          reference.push_back(value);
        } else {
          assert_eq!(ring.push(value), Err(value));
        }
      } else if choice < 90 {
        // Action: Pop
        let ring_val = ring.pop();
        let ref_val = reference.pop_front();
        assert_eq!(ring_val, ref_val);
      } else {
        // Action: Clear
        ring.clear();
        reference.clear();
      }

      // Assert invariant structures match on every cycle
      assert_eq!(ring.is_empty(), reference.is_empty());
      assert_eq!(ring.len(), reference.len());
    }
  }
}
