use crate::internal::cache_padded::CachePadded;
use parking_lot::Mutex;
use core::fmt;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

/// Capacity of each block segment.
/// 32 is chosen to balance allocation frequency with memory footprint and cache locality.
const BLOCK_CAPACITY: usize = 32;

/// Slot states for handling the write-race.
const SLOT_EMPTY: u8 = 0;
const SLOT_WRITING: u8 = 1;
const SLOT_READY: u8 = 2;

struct Slot<T> {
  state: AtomicU8,
  value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Default for Slot<T> {
  fn default() -> Self {
    Self {
      state: AtomicU8::new(SLOT_EMPTY),
      value: UnsafeCell::new(MaybeUninit::uninit()),
    }
  }
}

pub(crate) struct Block<T> {
  /// Pointer to the next block in the linked list.
  /// We use a raw AtomicPtr, but the protocol is that this pointer
  /// OWNS one strong reference count to the `Arc<Block<T>>`.
  next: AtomicPtr<Block<T>>,
  write_index: CachePadded<AtomicUsize>,
  slots: [Slot<T>; BLOCK_CAPACITY],
}

// Safety: We manage access to the `slots` array via atomic state flags.
// Access to `next` is atomic. `write_index` is atomic.
unsafe impl<T: Send> Send for Block<T> {}
unsafe impl<T: Send> Sync for Block<T> {}

impl<T> fmt::Debug for Block<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Block")
      .field("write_index", &self.write_index.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

impl<T> Block<T> {
  fn new() -> Self {
    // Initialize array of slots safely without creating uninitialized integers.
    let slots: [Slot<T>; BLOCK_CAPACITY] = unsafe {
      // Create an uninitialized array of MaybeUninit<Slot<T>>
      let mut slots_uninit: [MaybeUninit<Slot<T>>; BLOCK_CAPACITY] =
        MaybeUninit::uninit().assume_init();

      // Initialize each element
      for i in 0..BLOCK_CAPACITY {
        slots_uninit[i].write(Slot::default());
      }

      // Transmute to the initialized type.
      // This is safe because MaybeUninit<T> has the same layout as T,
      // and we have initialized all elements.
      // We use pointer casting instead of mem::transmute to avoid issues with type inference.
      ptr::read(&slots_uninit as *const _ as *const [Slot<T>; BLOCK_CAPACITY])
    };

    Self {
      next: AtomicPtr::new(ptr::null_mut()),
      write_index: CachePadded::new(AtomicUsize::new(0)),
      slots,
    }
  }
}

/// An unbounded queue implemented as a segmented linked list of fixed-size blocks.
///
/// # Safety Architecture
///
/// 1. **Head (Producer)**: Protected by a `Mutex<Arc<Block>>`.
///    - Producers mostly access their thread-local cache (fast path).
///    - When cache is empty/full, they lock this mutex to get the current block (slow path).
///    - The Mutex ensures we hold a strong `Arc` while reading the block, preventing Use-After-Free.
///
/// 2. **Tail (Consumer)**: Protected by `UnsafeCell<Arc<Block>>`.
///    - Consumer owns the tail block via a strong `Arc`.
///    - When traversing `next`, it converts the raw pointer back to `Arc`, claiming ownership.
///
/// 3. **Next Pointer**: `AtomicPtr<Block>`.
///    - Stores a raw pointer that was created via `Arc::into_raw`.
///    - This "leaks" a reference count into the structure itself.
pub(crate) struct UnboundedBlockQueue<T> {
  /// The current block where new items should be appended.
  /// Protected by a Mutex to serialize block rotation and ensure safety.
  head: CachePadded<Mutex<Arc<Block<T>>>>,

  /// The current block being read by the consumer.
  /// Only accessed by the single consumer.
  tail: CachePadded<UnsafeCell<Arc<Block<T>>>>,

  /// Index of the next item to read in the tail block.
  tail_cursor: CachePadded<UnsafeCell<usize>>,
}

// Safe to send across threads because internal synchronization handles it.
unsafe impl<T: Send> Send for UnboundedBlockQueue<T> {}
unsafe impl<T: Send> Sync for UnboundedBlockQueue<T> {}

impl<T> UnboundedBlockQueue<T> {
  pub(crate) fn new() -> Self {
    let block = Arc::new(Block::new());
    Self {
      head: CachePadded::new(Mutex::new(block.clone())),
      tail: CachePadded::new(UnsafeCell::new(block)),
      tail_cursor: CachePadded::new(UnsafeCell::new(0)),
    }
  }

  /// Pushes an item into the queue.
  ///
  /// `block_cache` is a thread-local (or struct-local) cache of the last used block.
  /// If `block_cache` is valid and has space, we use it (Lock-Free Fast Path).
  /// If not, we lock the global head and update the cache (Slow Path).
  pub(crate) fn push(&self, value: T, block_cache: &mut Option<Arc<Block<T>>>) {
    loop {
      // --- Fast Path: Try using the cached block ---
      if let Some(block) = block_cache {
        // Optimistically increment index
        let idx = block.write_index.fetch_add(1, Ordering::Relaxed);

        if idx < BLOCK_CAPACITY {
          // Success! We claimed a slot.
          let slot = &block.slots[idx];

          // 1. Mark as WRITING. Consumer will spin if it sees this.
          slot.state.store(SLOT_WRITING, Ordering::Release);

          // 2. Write the data.
          unsafe {
            (*slot.value.get()).write(value);
          }

          // 3. Mark as READY. Consumer can now read.
          slot.state.store(SLOT_READY, Ordering::Release);
          return;
        } else {
          // Block is full. Clear cache and fall through to Slow Path.
          *block_cache = None;
        }
      }

      // --- Slow Path: Rotate Block or Fetch New Head ---
      {
        let mut head_guard = self.head.lock(); // Serialize producers here
        let current_head = head_guard.clone();

        // Check if the global head actually has space (maybe another thread rotated it?)
        if current_head.write_index.load(Ordering::Relaxed) < BLOCK_CAPACITY {
          // Good news, global head has space. Update cache and retry loop.
          *block_cache = Some(current_head);
          continue;
        }

        // Global head is also full. We must allocate a new block.
        let new_block = Arc::new(Block::new());

        // Link current_head -> new_block
        // Crucial: We use `into_raw` to transfer ownership of one ref-count to the `next` ptr.
        let new_block_ptr = Arc::into_raw(new_block.clone()) as *mut Block<T>;
        current_head.next.store(new_block_ptr, Ordering::Release);

        // Advance global head
        *head_guard = new_block.clone();

        // Update our cache
        *block_cache = Some(new_block);
      }
      // Loop continues, will hit Fast Path with the new cached block.
    }
  }

  /// Pops an item from the queue.
  pub(crate) fn pop(&self) -> Option<T> {
    unsafe {
      let tail_arc_ptr = self.tail.get(); // *mut Arc<Block<T>>
      let cursor_ptr = self.tail_cursor.get();
      let mut cursor = *cursor_ptr;

      // Check if we reached the end of the current block
      if cursor == BLOCK_CAPACITY {
        // We need to move to the next block.
        // Scope the access to the current tail block so we don't hold a reference
        // while we potentially drop the Arc backing it.
        let next_ptr = {
          let tail_block = &**tail_arc_ptr;
          tail_block.next.load(Ordering::Acquire)
        };

        if !next_ptr.is_null() {
          // Found next block.
          // 1. Convert raw pointer back to Arc. This claims the ref-count we leaked in push().
          let next_block_arc = Arc::from_raw(next_ptr);

          // 2. Replace current tail with next block.
          // This drops the old Arc. Since we scoped `tail_block` above, no dangling reference exists.
          ptr::replace(tail_arc_ptr, next_block_arc);

          // 3. Reset cursor.
          *cursor_ptr = 0;

          // Recursively try popping from the new block.
          return self.pop();
        } else {
          // No next block yet. Queue is empty.
          return None;
        }
      }

      // Try to read the slot at current cursor
      // It is safe to take a reference here because we know we aren't replacing tail in this branch.
      let tail_block = &**tail_arc_ptr;
      let slot = &tail_block.slots[cursor];
      let state = slot.state.load(Ordering::Acquire);

      if state == SLOT_READY {
        let value = slot.value.get().read().assume_init();
        *cursor_ptr = cursor + 1;
        return Some(value);
      } else if state == SLOT_WRITING {
        // Producer claimed this slot index but hasn't finished writing the value.
        // This is a very short temporary race. We spin.
        let mut spin_count = 0;
        while slot.state.load(Ordering::Acquire) != SLOT_READY {
          if spin_count > 100 {
            thread::yield_now();
          } else {
            std::hint::spin_loop();
          }
          spin_count += 1;
        }
        let value = slot.value.get().read().assume_init();
        *cursor_ptr = cursor + 1;
        return Some(value);
      } else {
        // SLOT_EMPTY: Producer index hasn't reached here yet. Queue is truly empty.
        return None;
      }
    }
  }

  pub(crate) fn is_empty(&self) -> bool {
    unsafe {
      let tail_block = &**self.tail.get();
      let cursor = *self.tail_cursor.get();

      if cursor == BLOCK_CAPACITY {
        let next = tail_block.next.load(Ordering::Acquire);
        // If next is null, we are at the end, so empty.
        // If next exists, we effectively have items (or at least a new block to check).
        return next.is_null();
      }

      let state = tail_block.slots[cursor].state.load(Ordering::Acquire);
      state == SLOT_EMPTY
    }
  }
}

impl<T> Drop for UnboundedBlockQueue<T> {
  fn drop(&mut self) {
    unsafe {
      let tail_ptr = self.tail.get();

      // Crucial: Increment ref count because we are creating a second owner `current_arc`
      // via ptr::read. `self.tail` will still drop at the end of this struct's lifetime,
      // so we need 2 ref counts for 2 handles.
      let tail_ref = &*tail_ptr;
      Arc::increment_strong_count(Arc::as_ptr(tail_ref));

      let mut current_arc = ptr::read(tail_ptr);
      let mut cursor = *self.tail_cursor.get();

      loop {
        // Drop initialized items in this block
        for i in cursor..BLOCK_CAPACITY {
          let slot = &current_arc.slots[i];
          // We use Acquire to ensure we see writes from producers
          if slot.state.load(Ordering::Acquire) == SLOT_READY {
            (*slot.value.get()).assume_init_drop();
          } else {
            break; // No more items in this block
          }
        }

        // Check for next block
        let next_ptr = current_arc.next.load(Ordering::Acquire);

        if !next_ptr.is_null() {
          // Claim ownership of the next block from the raw pointer
          let next_arc = Arc::from_raw(next_ptr);
          // Move to next. This drops the old `current_arc`.
          current_arc = next_arc;
          cursor = 0;
        } else {
          break;
        }
      }
      // Implicit drop of `current_arc` happens here.
      // Implicit drop of `self.tail` happens after this function returns.
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::{Arc, Barrier};
  use std::thread;

  use std::sync::Mutex;
  // Use a simple RNG to avoid heavy dependencies if 'rand' isn't available,
  // or use standard rand if present. Assuming rand 0.8 is available as per previous context.
  use rand::Rng;

  #[test]
  fn test_simple_push_pop() {
    let q = UnboundedBlockQueue::new();
    let mut cache = None;
    q.push(1, &mut cache);
    q.push(2, &mut cache);

    assert_eq!(q.pop(), Some(1));
    assert_eq!(q.pop(), Some(2));
    assert_eq!(q.pop(), None);
  }

  #[test]
  fn test_block_rotation() {
    let q = UnboundedBlockQueue::new();
    let mut cache = None;
    // Fill first block
    for i in 0..BLOCK_CAPACITY {
      q.push(i, &mut cache);
    }
    // Trigger allocation
    q.push(999, &mut cache);

    for i in 0..BLOCK_CAPACITY {
      assert_eq!(q.pop(), Some(i));
    }
    assert_eq!(q.pop(), Some(999));
    assert_eq!(q.pop(), None);
  }

  #[test]
  fn test_concurrent_push_pop() {
    let q = Arc::new(UnboundedBlockQueue::new());
    let q_clone = q.clone();

    let t = thread::spawn(move || {
      let mut cache = None;
      for i in 0..1000 {
        q_clone.push(i, &mut cache);
      }
    });

    let mut received = 0;
    for _ in 0..1000 {
      loop {
        if let Some(_) = q.pop() {
          received += 1;
          break;
        }
        thread::yield_now();
      }
    }
    t.join().unwrap();
    assert_eq!(received, 1000);
    assert_eq!(q.pop(), None);
  }

  #[test]
  fn test_drop_cleanup() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    struct Dropper(usize);
    impl Drop for Dropper {
      fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::Relaxed);
      }
    }

    let q = UnboundedBlockQueue::new();
    let mut cache = None;
    q.push(Dropper(1), &mut cache);
    q.push(Dropper(2), &mut cache);
    q.push(Dropper(3), &mut cache);
    assert_eq!(q.pop().map(|d| d.0), Some(1));
    // Dropper(1) dropped here

    drop(q);
    // Dropper(2) and Dropper(3) dropped here

    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 3);
  }

  #[test]
  fn test_multi_producer_race_to_allocate_block() {
    // This test forces multiple threads to contend for the slot `BLOCK_CAPACITY`
    // which triggers new block allocation.
    const THREADS: usize = 8;
    const ITEMS_PER_THREAD: usize = BLOCK_CAPACITY * 4;

    let q = Arc::new(UnboundedBlockQueue::new());
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = vec![];

    for i in 0..THREADS {
      let q = q.clone();
      let b = barrier.clone();
      handles.push(thread::spawn(move || {
        let mut cache = None;
        b.wait();
        for j in 0..ITEMS_PER_THREAD {
          q.push(i * ITEMS_PER_THREAD + j, &mut cache);
        }
      }));
    }

    for h in handles {
      h.join().unwrap();
    }

    // Collect all items
    let mut results = Vec::new();
    while let Some(val) = q.pop() {
      results.push(val);
    }

    assert_eq!(results.len(), THREADS * ITEMS_PER_THREAD);
    results.sort();

    for i in 0..THREADS {
      for j in 0..ITEMS_PER_THREAD {
        assert!(results.contains(&(i * ITEMS_PER_THREAD + j)));
      }
    }
  }

  #[test]
  fn test_drop_cleanup_across_multiple_blocks() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
    struct Dropper;
    impl Drop for Dropper {
      fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::Relaxed);
      }
    }

    DROP_COUNT.store(0, Ordering::Relaxed);
    let q = UnboundedBlockQueue::new();
    let mut cache = None;

    // Push enough to fill 2.5 blocks
    let count = (BLOCK_CAPACITY * 2) + (BLOCK_CAPACITY / 2);
    for _ in 0..count {
      q.push(Dropper, &mut cache);
    }

    // Pop half a block
    for _ in 0..(BLOCK_CAPACITY / 2) {
      q.pop();
    }
    // These pops dropped BLOCK_CAPACITY/2 items.
    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), BLOCK_CAPACITY / 2);

    // Drop the queue. Remaining items:
    // 1. Half of first block (tail block)
    // 2. Full second block
    // 3. Half of third block (head block)
    drop(q);

    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), count);
  }

  #[test]
  fn test_is_empty_correctness() {
    let q = UnboundedBlockQueue::new();
    let mut cache = None;
    assert!(q.is_empty());

    q.push(1, &mut cache);
    assert!(!q.is_empty());

    q.pop();
    assert!(q.is_empty());

    // Push until rotation
    for i in 0..BLOCK_CAPACITY {
      q.push(i, &mut cache);
    }
    // Now full block, but effectively empty if we pop all
    for _ in 0..BLOCK_CAPACITY {
      q.pop();
    }
    assert!(q.is_empty());

    // Trigger new block
    q.push(100, &mut cache);
    assert!(!q.is_empty());
    q.pop();
    assert!(q.is_empty());
  }

  #[test]
  fn test_stress_random_ops() {
    const NUM_PRODUCERS: usize = 8;
    const NUM_OPS_PER_PRODUCER: usize = if cfg!(miri) { 100 } else { 50_000 };

    let q = Arc::new(UnboundedBlockQueue::new());
    // We wrap the consumer end (q) in a Mutex to simulate single-consumer restriction
    // if we were accessing `pop` from multiple threads, but `UnboundedBlockQueue`
    // itself doesn't enforce single-thread popping at compile time (it's UnsafeCell).
    // However, the contract is SPSC or MPSC.
    let consumer_lock = Arc::new(Mutex::new(q.clone()));

    let mut handles = vec![];

    // Spawn producers
    for i in 0..NUM_PRODUCERS {
      let q = q.clone();
      handles.push(thread::spawn(move || {
        let mut rng = rand::thread_rng();
        let mut cache = None;
        for _ in 0..NUM_OPS_PER_PRODUCER {
          if rng.gen_bool(0.01) {
            thread::yield_now();
          }
          q.push(i, &mut cache); // Push producer ID
        }
      }));
    }

    // Spawn a "consumer" thread that drains aggressively
    let received_counts = Arc::new(Mutex::new(vec![0; NUM_PRODUCERS]));
    let receiver_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let c_lock = consumer_lock.clone();
    let counts = received_counts.clone();
    let finished = receiver_finished.clone();

    let consumer_handle = thread::spawn(move || loop {
      let q = c_lock.lock().unwrap();
      match q.pop() {
        Some(producer_id) => {
          let mut c = counts.lock().unwrap();
          c[producer_id] += 1;
        }
        None => {
          if finished.load(Ordering::Acquire) {
            if q.is_empty() {
              break;
            }
          } else {
            drop(q);
            thread::yield_now();
          }
        }
      }
    });

    for h in handles {
      h.join().unwrap();
    }
    receiver_finished.store(true, Ordering::Release);
    consumer_handle.join().unwrap();

    let final_counts = received_counts.lock().unwrap();
    for (i, count) in final_counts.iter().enumerate() {
      assert_eq!(
        *count, NUM_OPS_PER_PRODUCER,
        "Producer {} count mismatch",
        i
      );
    }
  }
}
