use std::collections::VecDeque;
use std::fmt;
use std::iter::FusedIterator;
use std::mem::MaybeUninit;
use std::ptr;

/// A block of slots for the BlockedVecDeque.
///
/// Aligned to 128 bytes to ensure cache-line alignment on most architectures (x86_64: 64, aarch64: up to 128).
#[repr(align(128))]
struct Block<T, const N: usize> {
  slots: [MaybeUninit<T>; N],
}

impl<T, const N: usize> Block<T, N> {
  fn new() -> Self {
    // Safety: An array of MaybeUninit is safe to create uninitialized.
    // We use unsafe to assume_init because MaybeUninit::uninit_array is unstable.
    let slots = unsafe { MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init() };
    Self { slots }
  }
}

/// A deque implemented as a list of fixed-size blocks.
///
/// This structure is optimized for high-throughput scenarios where elements are
/// added and removed in bulk or streams, minimizing allocator pressure by
/// allocating/freeing memory in chunks (`BLOCK_SIZE`).
///
/// # Tuning
/// `BLOCK_SIZE` defaults to 64. Sweet spots for waiter queues are often 32-128 to balance
/// allocation churn, cache locality, and scan latency.
pub(crate) struct BlockedVecDeque<T, const BLOCK_SIZE: usize = 64> {
  // Improvement: Use VecDeque instead of Vec for blocks to allow O(1) push_front/pop_front
  // of blocks without shifting or infinite index growth.
  blocks: VecDeque<Block<T, BLOCK_SIZE>>,

  // Position within the head block (blocks.front())
  head_offset: usize,
  // Position within the tail block (blocks.back())
  tail_offset: usize,

  len: usize,
}

impl<T, const BLOCK_SIZE: usize> fmt::Debug for BlockedVecDeque<T, BLOCK_SIZE> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("BlockedVecDeque")
      .field("len", &self.len)
      .field("head_offset", &self.head_offset)
      .field("tail_offset", &self.tail_offset)
      .field("block_count", &self.blocks.len())
      .finish()
  }
}

impl<T, const BLOCK_SIZE: usize> BlockedVecDeque<T, BLOCK_SIZE> {
  /// Creates a new empty BlockedVecDeque.
  pub(crate) fn new() -> Self {
    Self {
      blocks: VecDeque::new(),
      head_offset: 0,
      tail_offset: 0,
      len: 0,
    }
  }

  /// Returns the number of elements in the deque.
  #[inline]
  pub(crate) fn len(&self) -> usize {
    self.len
  }

  /// Returns true if the deque is empty.
  #[inline(always)]
  pub(crate) fn is_empty(&self) -> bool {
    self.len == 0
  }

  /// Adds an element to the back of the deque.
  pub(crate) fn push_back(&mut self, item: T) {
    // 1. Fast path: Write to existing tail block if available and has space
    if !self.blocks.is_empty() && self.tail_offset < BLOCK_SIZE {
      let tail_block = self.blocks.back_mut().unwrap();
      unsafe {
        tail_block
          .slots
          .get_unchecked_mut(self.tail_offset)
          .write(item);
      }
      self.tail_offset += 1;
    } else {
      // 2. Slow path: Allocate new block (either queue is empty or tail is full)
      let mut new_block = Block::new();
      unsafe {
        new_block.slots.get_unchecked_mut(0).write(item);
      }
      self.blocks.push_back(new_block);

      if self.len == 0 {
        self.head_offset = 0;
      }
      self.tail_offset = 1;
    }

    self.len += 1;
  }

  /// Removes the first element and returns it, or None if the deque is empty.
  pub(crate) fn pop_front(&mut self) -> Option<T> {
    if self.len == 0 {
      return None;
    }

    // 2. Read item from head block
    let head_block = self.blocks.front_mut().unwrap();
    let item = unsafe {
      head_block
        .slots
        .get_unchecked(self.head_offset)
        .assume_init_read()
    };

    // 3. Increment head_offset
    self.head_offset += 1;

    // 4. If head block exhausted
    if self.head_offset == BLOCK_SIZE {
      if self.blocks.len() > 1 {
        // Drop/free head block
        self.blocks.pop_front();
        self.head_offset = 0;
      } else {
        // Was last element in the last block, reset offsets
        self.head_offset = 0;
        self.tail_offset = 0;
      }
    } else if self.len == 1 {
      // Special case: We just popped the last element, but didn't exhaust the block size.
      // Reset to clean state to avoid creeping offsets.
      self.head_offset = 0;
      self.tail_offset = 0;
    }

    self.len -= 1;
    Some(item)
  }

  /// Adds an element to the front of the deque.
  pub(crate) fn push_front(&mut self, item: T) {
    if self.head_offset > 0 {
      // 1. Simple case: space in current head block
      self.head_offset -= 1;
      let head_block = self.blocks.front_mut().unwrap();
      unsafe {
        head_block
          .slots
          .get_unchecked_mut(self.head_offset)
          .write(item);
      }
    } else if self.len == 0 {
      // 2. Empty case: Reuse existing block or allocate one.
      // We write at index 0 to favor the common push_back/pop_front pattern (FIFO).
      if self.blocks.is_empty() {
        self.blocks.push_back(Block::new());
      }
      // Ensure offsets are reset (should be 0 already if len==0, but be explicit)
      self.head_offset = 0;
      self.tail_offset = 1;

      let head_block = self.blocks.front_mut().unwrap();
      unsafe {
        head_block.slots.get_unchecked_mut(0).write(item);
      }
    } else {
      // 3. Full head block case: Allocate new block at front
      let mut new_block = Block::new();
      self.head_offset = BLOCK_SIZE - 1;
      unsafe {
        new_block
          .slots
          .get_unchecked_mut(self.head_offset)
          .write(item);
      }
      self.blocks.push_front(new_block);
    }

    self.len += 1;
  }

  /// Removes the last element and returns it, or None if the deque is empty.
  pub(crate) fn pop_back(&mut self) -> Option<T> {
    if self.len == 0 {
      return None;
    }

    let item;
    if self.tail_offset > 0 {
      // 2. If tail_offset > 0
      self.tail_offset -= 1;
      let tail_block = self.blocks.back_mut().unwrap();
      item = unsafe {
        tail_block
          .slots
          .get_unchecked(self.tail_offset)
          .assume_init_read()
      };
    } else {
      // 3. Else (tail_offset == 0, need to move to previous block)
      if self.blocks.len() > 1 {
        // Drop/free tail block (it's empty or we are moving past it)
        // Actually if tail_offset == 0, the tail block is empty (or we just filled it?
        // No, tail_offset points to next write. If 0, it's empty).
        // But wait, if tail_offset == 0, we shouldn't have a tail block unless it's the only one?
        // If len > 0 and tail_offset == 0, it implies the *current* tail block is empty
        // and the data is in the previous block ending at BLOCK_SIZE.
        // However, our push_back logic allocates a new block and sets tail_offset=1.
        // So tail_offset is only 0 if the queue is empty or we just reset.
        // Let's re-read spec: "If tail_offset == 0... If tail_block_idx > head_block_idx".
        // This implies we have an empty tail block sitting there?
        // Our push_back only allocates when writing.
        // So tail_offset should rarely be 0 if len > 0, unless we manually added an empty block?
        // Ah, if we push_front'ed into a new block, tail_offset might be 0 in that new block?
        // No, push_front adds to front.

        // Let's handle the case where we might have an empty tail block or need to step back.
        // If tail_offset is 0, we must drop the current tail block and step back.
        self.blocks.pop_back();
        self.tail_offset = BLOCK_SIZE - 1;
        let new_tail = self.blocks.back_mut().unwrap();
        item = unsafe {
          new_tail
            .slots
            .get_unchecked(self.tail_offset)
            .assume_init_read()
        };
      } else {
        // Single element case where tail_offset might be 0?
        // If len > 0, tail_offset must be > head_offset.
        // If tail_offset is 0, then len must be 0.
        // But we checked len == 0.
        // So this branch is unreachable unless invariants are broken.
        // We'll return None or panic.
        return None;
      }
    }

    self.len -= 1;
    if self.len == 0 {
      self.head_offset = 0;
      self.tail_offset = 0;
    }
    Some(item)
  }

  /// Returns a reference to the element at the given index.
  pub(crate) fn get(&self, index: usize) -> Option<&T> {
    if index >= self.len {
      return None;
    }

    let pos = self.head_offset + index;
    let block_idx = pos / BLOCK_SIZE;
    let offset = pos % BLOCK_SIZE;

    // blocks[block_idx] is valid because pos < len + head_offset
    let block = self.blocks.get(block_idx)?;
    unsafe { Some(block.slots.get_unchecked(offset).assume_init_ref()) }
  }

  /// Returns a mutable reference to the element at the given index.
  pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
    if index >= self.len {
      return None;
    }

    let pos = self.head_offset + index;
    let block_idx = pos / BLOCK_SIZE;
    let offset = pos % BLOCK_SIZE;

    let block = self.blocks.get_mut(block_idx)?;
    unsafe { Some(block.slots.get_unchecked_mut(offset).assume_init_mut()) }
  }

  /// Clears the deque, dropping all elements and freeing memory.
  pub(crate) fn clear(&mut self) {
    // Efficiently drop all elements
    while self.pop_front().is_some() {}
    // Blocks are freed by pop_front or here
    self.blocks.clear();
    self.head_offset = 0;
    self.tail_offset = 0;
    self.len = 0;
  }

  /// Shrinks the capacity of the underlying deque as much as possible.
  pub(crate) fn shrink_to_fit(&mut self) {
    self.blocks.shrink_to_fit();
  }

  /// Creates an iterator over the elements.
  pub(crate) fn iter(&self) -> Iter<'_, T, BLOCK_SIZE> {
    Iter {
      deque: self,
      block_idx: 0,
      offset: self.head_offset,
      remaining: self.len,
    }
  }

  /// Creates a mutable iterator over the elements.
  pub(crate) fn iter_mut(&mut self) -> IterMut<'_, T, BLOCK_SIZE> {
    IterMut {
      block_iter: self.blocks.iter_mut(),
      current_block_ptr: ptr::null_mut(),
      block_offset: self.head_offset,
      remaining: self.len,
      _marker: std::marker::PhantomData,
    }
  }

  /// Creates a draining iterator that removes elements.
  pub(crate) fn drain(&mut self) -> Drain<'_, T, BLOCK_SIZE> {
    Drain { deque: self }
  }

  /// Removes the first occurrence of an item from the deque.
  ///
  /// This is an O(N) operation that requires draining and rebuilding the deque.
  /// It is highly inefficient and should be used with caution.
  pub(crate) fn remove_item(&mut self, item_to_remove: T) -> bool
  where
    T: PartialEq,
  {
    if self.is_empty() {
      return false;
    }

    let original_len = self.len();
    let mut temp_items = Vec::with_capacity(original_len.saturating_sub(1));

    let mut found = false;

    // Drain the deque, keeping items that don't match.
    while let Some(item) = self.pop_front() {
      if !found && item == item_to_remove {
        found = true;
        // Do not push this item to temp_items, effectively removing it.
      } else {
        temp_items.push(item);
      }
    }

    // Rebuild the deque from the temporary items.
    for item in temp_items {
      self.push_back(item);
    }

    found
  }
}

impl<T, const B: usize> Drop for BlockedVecDeque<T, B> {
  fn drop(&mut self) {
    self.clear();
  }
}

// --- Iterators ---

pub(crate) struct Iter<'a, T, const B: usize> {
  deque: &'a BlockedVecDeque<T, B>,
  block_idx: usize,
  offset: usize,
  remaining: usize,
}

impl<'a, T, const B: usize> Iterator for Iter<'a, T, B> {
  type Item = &'a T;

  fn next(&mut self) -> Option<Self::Item> {
    if self.remaining == 0 {
      return None;
    }

    let block = &self.deque.blocks[self.block_idx];
    let item = unsafe { block.slots.get_unchecked(self.offset).assume_init_ref() };

    self.offset += 1;
    if self.offset == B {
      self.offset = 0;
      self.block_idx += 1;
    }
    self.remaining -= 1;

    Some(item)
  }
}

pub(crate) struct IterMut<'a, T, const B: usize> {
  block_iter: std::collections::vec_deque::IterMut<'a, Block<T, B>>,
  current_block_ptr: *mut Block<T, B>,
  block_offset: usize,
  remaining: usize,
  _marker: std::marker::PhantomData<&'a mut T>,
}

impl<'a, T, const B: usize> Iterator for IterMut<'a, T, B> {
  type Item = &'a mut T;

  fn next(&mut self) -> Option<Self::Item> {
    if self.remaining == 0 {
      return None;
    }

    if self.current_block_ptr.is_null() {
      match self.block_iter.next() {
        Some(block) => {
          self.current_block_ptr = block as *mut _;
        }
        None => return None,
      }
    }

    let block = unsafe { &mut *self.current_block_ptr };
    let item = unsafe {
      block
        .slots
        .get_unchecked_mut(self.block_offset)
        .assume_init_mut()
    };

    self.block_offset += 1;
    self.remaining -= 1;

    if self.block_offset == B {
      self.block_offset = 0;
      self.current_block_ptr = ptr::null_mut();
    }

    Some(item)
  }
}

pub(crate) struct Drain<'a, T, const B: usize> {
  deque: &'a mut BlockedVecDeque<T, B>,
}

impl<'a, T, const B: usize> Iterator for Drain<'a, T, B> {
  type Item = T;

  fn next(&mut self) -> Option<Self::Item> {
    self.deque.pop_front()
  }
}

impl<'a, T, const B: usize> Drop for Drain<'a, T, B> {
  fn drop(&mut self) {
    // Drop remaining elements
    self.deque.clear();
  }
}

impl<'a, T, const B: usize> FusedIterator for Drain<'a, T, B> {}
