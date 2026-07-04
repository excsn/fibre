use std::{
  ptr,
  sync::atomic::{AtomicPtr, Ordering},
  task::Waker,
  thread::Thread,
};

// The node containing a waiter. It's part of a lock-free intrusive linked list (stack).
pub(super) struct WaiterNode {
  // Can be a sync thread or an async task waker.
  pub(super) waiter: Waiter,
  // Pointer to the next node in the stack.
  pub(super) next: AtomicPtr<WaiterNode>,
}

// An enum abstracting over the two types of waiters we support.
pub(super) enum Waiter {
  Thread(Thread),
  Task(Waker),
}

impl Waiter {
  pub(super) fn wake(self) {
    match self {
      Waiter::Thread(t) => t.unpark(),
      Waiter::Task(w) => w.wake(),
    }
  }
}

// A lock-free Treiber stack to manage the queue of waiters.
pub(super) struct WaitQueue {
  head: AtomicPtr<WaiterNode>,
}

impl WaitQueue {
  pub(super) fn new() -> Self {
    Self {
      head: AtomicPtr::new(ptr::null_mut()),
    }
  }

  // Pushes a new waiter to the head of the stack.
  pub(super) fn push(&self, node: *mut WaiterNode) {
    let mut head = self.head.load(Ordering::Relaxed);
    loop {
      // Safety: The raw pointer `node` is valid and represents an allocated box.
      unsafe { (*node).next.store(head, Ordering::Relaxed) };

      // Attempt to CAS the head pointer.
      match self
        .head
        .compare_exchange_weak(head, node, Ordering::Release, Ordering::Relaxed)
      {
        Ok(_) => break,     // Success
        Err(h) => head = h, // Contention, retry with the new head
      }
    }
  }

  // Pops all waiters from the stack and returns them in a Vec.
  // This is simpler than popping one-by-one in a highly contended scenario.
  pub(super) fn pop_all(&self) -> Vec<Box<WaiterNode>> {
    let mut head = self.head.swap(ptr::null_mut(), Ordering::Acquire);
    if head.is_null() {
      return Vec::new();
    }

    let mut waiters = Vec::new();
    while !head.is_null() {
      // Safety: The raw pointer `head` is valid as it came from the atomic ptr.
      let node = unsafe { Box::from_raw(head) };
      head = node.next.load(Ordering::Relaxed);
      waiters.push(node);
    }
    // The waiters are in LIFO order, so reverse them to get FIFO.
    waiters.reverse();
    waiters
  }
}
