use std::fmt;
use std::marker::{PhantomData, PhantomPinned};
use std::ptr::NonNull;
use std::sync::atomic::AtomicBool;
use std::task::Waker;
use std::thread::{self, Thread};

pub(crate) trait LinkedNode: Sized {
  fn prev(&self) -> Option<NonNull<Self>>;
  fn next(&self) -> Option<NonNull<Self>>;
  fn set_prev(&mut self, ptr: Option<NonNull<Self>>);
  fn set_next(&mut self, ptr: Option<NonNull<Self>>);
  fn set_enqueued(&mut self, val: bool);
  fn is_enqueued(&self) -> bool;
}

pub(crate) struct SyncWaiterNode<T> {
  // Pointers to neighbors.
  prev: Option<NonNull<Self>>,
  next: Option<NonNull<Self>>,
  pub(crate) item: Option<T>,
  pub(crate) thread: Thread,
  pub(crate) notified: AtomicBool,
  pub(crate) enqueued: bool,
  _pin: PhantomPinned,
}

impl<T> SyncWaiterNode<T> {
  pub(crate) fn new(thread: Thread) -> Self {
    Self {
      prev: None,
      next: None,
      item: None,
      thread,
      notified: AtomicBool::new(false),
      enqueued: false,
      _pin: PhantomPinned,
    }
  }

  pub(crate) fn wake(&self) {
    self.notified.store(true, std::sync::atomic::Ordering::Release);
    self.thread.unpark();
  }
}

impl<T> LinkedNode for SyncWaiterNode<T> {
  #[inline(always)] fn prev(&self) -> Option<NonNull<Self>> { self.prev }
  #[inline(always)] fn next(&self) -> Option<NonNull<Self>> { self.next }
  #[inline(always)] fn set_prev(&mut self, ptr: Option<NonNull<Self>>) { self.prev = ptr; }
  #[inline(always)] fn set_next(&mut self, ptr: Option<NonNull<Self>>) { self.next = ptr; }
  #[inline(always)] fn set_enqueued(&mut self, val: bool) { self.enqueued = val; }
  #[inline(always)] fn is_enqueued(&self) -> bool { self.enqueued }
}

#[derive(Debug)]
pub(crate) struct AsyncWaiterNode<T> {
  prev: Option<NonNull<Self>>,
  next: Option<NonNull<Self>>,
  pub(crate) item: Option<T>,
  pub(crate) waker: Option<Waker>,
  pub(crate) enqueued: bool,
  _pin: PhantomPinned,
}

impl<T> AsyncWaiterNode<T> {
  pub(crate) fn new() -> Self {
    Self {
      prev: None,
      next: None,
      item: None,
      waker: None,
      enqueued: false,
      _pin: PhantomPinned,
    }
  }

  pub(crate) fn wake(&self) {
    if let Some(w) = &self.waker {
      w.wake_by_ref();
    }
  }
}

impl<T> LinkedNode for AsyncWaiterNode<T> {
  #[inline(always)] fn prev(&self) -> Option<NonNull<Self>> { self.prev }
  #[inline(always)] fn next(&self) -> Option<NonNull<Self>> { self.next }
  #[inline(always)] fn set_prev(&mut self, ptr: Option<NonNull<Self>>) { self.prev = ptr; }
  #[inline(always)] fn set_next(&mut self, ptr: Option<NonNull<Self>>) { self.next = ptr; }
  #[inline(always)] fn set_enqueued(&mut self, val: bool) { self.enqueued = val; }
  #[inline(always)] fn is_enqueued(&self) -> bool { self.enqueued }
}

// Safety: WaiterNode is owned by a single task/thread (Future or Receiver).
// It is only accessed concurrently when linked in the queue, which is protected by the queue's lock.
// The pointers are internal and managed safely.
unsafe impl<T: Send> Send for SyncWaiterNode<T> {}
unsafe impl<T: Send> Sync for SyncWaiterNode<T> {}
unsafe impl<T: Send> Send for AsyncWaiterNode<T> {}
unsafe impl<T: Send> Sync for AsyncWaiterNode<T> {}

/// A non-owning intrusive FIFO of `WaiterNode<T>` pointers.
///
/// # Safety Contract
/// - Each node passed into the queue *must* be pinned (not moved) for the
///   duration it is enqueued (e.g. `Box::pin(node)` and keep the `Pin<Box<_>>` alive).
/// - The caller must ensure exclusive access to the queue when calling
///   `push_back`, `pop_front`, or `remove`.
/// - Nodes must not be inserted into multiple queues concurrently.
/// - This queue does **not** perform synchronization itself. Wrap it in a
///   `Mutex` (or other sync primitive) if you need cross-thread access.
///
/// # Example
///
/// ```rust,ignore
/// use std::ptr::NonNull;
/// use std::thread;
///
/// let mut queue = WaiterQueue::<()>::new();
///
/// // 1. Create and pin the node
/// let mut node = Box::pin(WaiterNode::new_sync(thread::current()));
/// let node_ptr = unsafe { NonNull::from(node.as_mut().get_unchecked_mut()) };
///
/// // 2. Push to queue (requires exclusive access)
/// unsafe { queue.push_back(node_ptr); }
/// ```
pub(crate) struct WaiterQueue<N: LinkedNode> {
  head: Option<NonNull<N>>,
  tail: Option<NonNull<N>>,
  len: usize,
  _marker: PhantomData<Box<N>>,
}

// The queue holds pointers to nodes that are owned elsewhere.
// The queue itself is not thread-safe and must be synchronized externally (e.g. Mutex).
// It is Send because it only holds raw pointers and does not access thread-local data
// or drop the nodes (which are owned by the caller).
unsafe impl<N: LinkedNode + Send> Send for WaiterQueue<N> {}

impl<N: LinkedNode> fmt::Debug for WaiterQueue<N> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("WaiterQueue")
      .field("len", &self.len)
      .finish()
  }
}

impl<N: LinkedNode> Default for WaiterQueue<N> {
  fn default() -> Self {
    Self::new()
  }
}

impl<N: LinkedNode> WaiterQueue<N> {
  pub(crate) fn new() -> Self {
    Self {
      head: None,
      tail: None,
      len: 0,
      _marker: PhantomData,
    }
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.len == 0
  }

  pub(crate) fn len(&self) -> usize {
    self.len
  }

  /// Adds a node to the back of the queue.
  ///
  /// # Safety
  /// The `node` pointer must be valid and the node must be pinned or stable in memory
  /// for the duration it is in the queue. The caller must ensure exclusive access.
  pub(crate) unsafe fn push_back(&mut self, mut node: NonNull<N>) {
    let node_ref = node.as_mut();

    debug_assert!(
      !self.contains(node),
      "attempted to push a node that is already linked in this queue"
    );

    node_ref.set_next(None);
    node_ref.set_prev(self.tail);
    node_ref.set_enqueued(true);

    if let Some(mut tail) = self.tail {
      tail.as_mut().set_next(Some(node));
    } else {
      self.head = Some(node);
    }

    self.tail = Some(node);
    self.len += 1;
  }

  /// Returns the node at the front of the queue without removing it.
  pub(crate) fn peek_front(&self) -> Option<NonNull<N>> {
    self.head
  }

  /// Checks if the queue contains the given node by traversing the list.
  /// This is O(N) and intended for debugging or assertions.
  pub(crate) fn contains(&self, node: NonNull<N>) -> bool {
    unsafe {
      let mut cur = self.head;
      while let Some(p) = cur {
        if p == node {
          return true;
        }
        cur = p.as_ref().next();
      }
      false
    }
  }

  /// Removes the node from the front of the queue.
  ///
  /// # Safety
  /// The caller must ensure exclusive access.
  pub(crate) unsafe fn pop_front(&mut self) -> Option<NonNull<N>> {
    if let Some(mut head) = self.head {
      let head_ref = head.as_mut();
      let next = head_ref.next();

      if let Some(mut next_node) = next {
        next_node.as_mut().set_prev(None);
      } else {
        self.tail = None;
      }

      self.head = next;

      assert!(self.len > 0, "removing node from empty queue");
      self.len -= 1;

      head_ref.set_prev(None);
      head_ref.set_next(None);
      head_ref.set_enqueued(false);

      Some(head)
    } else {
      None
    }
  }

  /// Removes a specific node from the queue.
  ///
  /// # Safety
  /// The `node` pointer must be valid. The caller must ensure exclusive access.
  /// If the node is not in the queue, this operation attempts to be safe but
  /// relies on the node not being in *another* queue.
  pub(crate) unsafe fn remove(&mut self, mut node: NonNull<N>) {
    let node_ref = node.as_mut();

    // Check if node is detached
    if node_ref.prev().is_none() && node_ref.next().is_none() && self.head != Some(node) {
      return;
    }

    let prev = node_ref.prev();
    let next = node_ref.next();

    if let Some(mut prev_node) = prev {
      prev_node.as_mut().set_next(next);
    } else {
      self.head = next;
    }

    if let Some(mut next_node) = next {
      next_node.as_mut().set_prev(prev);
    } else {
      self.tail = prev;
    }

    node_ref.set_prev(None);
    node_ref.set_next(None);
    node_ref.set_enqueued(false);

    assert!(self.len > 0, "removing node from empty queue");
    self.len -= 1;
  }
}

impl<N: LinkedNode> Iterator for WaiterQueue<N> {
  type Item = NonNull<N>;

  fn next(&mut self) -> Option<Self::Item> {
    unsafe { self.pop_front() }
  }
}

impl<N: LinkedNode> FromIterator<NonNull<N>> for WaiterQueue<N> {
  fn from_iter<I: IntoIterator<Item = NonNull<N>>>(iter: I) -> Self {
    let mut queue = Self::new();
    for node in iter {
      unsafe { queue.push_back(node) };
    }
    queue
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::ptr::NonNull;
  use std::sync::atomic::{AtomicBool, Ordering};
  use std::sync::Arc;
  use std::task::{RawWaker, RawWakerVTable, Waker};
  use std::thread;

  // Helper to create a pinned node on the heap and get a NonNull pointer to it.
  fn make_pinned_node<T>(node: SyncWaiterNode<T>) -> std::pin::Pin<Box<SyncWaiterNode<T>>> {
    Box::pin(node)
  }

  fn get_ptr<T>(pinned: &mut std::pin::Pin<Box<SyncWaiterNode<T>>>) -> NonNull<SyncWaiterNode<T>> {
    // Safety: We are deriving the pointer from a pinned box, which won't move.
    unsafe { NonNull::from(pinned.as_mut().get_unchecked_mut()) }
  }

  #[test]
  fn test_push_pop() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    let mut node1 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let mut node2 = make_pinned_node(SyncWaiterNode::new(thread::current()));

    unsafe {
      queue.push_back(get_ptr(&mut node1));
      queue.push_back(get_ptr(&mut node2));
    }

    assert_eq!(queue.len(), 2);

    unsafe {
      let popped1 = queue.pop_front();
      assert_eq!(popped1, Some(get_ptr(&mut node1)));
      assert_eq!(queue.len(), 1);

      let popped2 = queue.pop_front();
      assert_eq!(popped2, Some(get_ptr(&mut node2)));
      assert_eq!(queue.len(), 0);

      let popped3 = queue.pop_front();
      assert!(popped3.is_none());
    }
  }

  #[test]
  fn test_remove_middle() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    let mut node1 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let mut node2 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let mut node3 = make_pinned_node(SyncWaiterNode::new(thread::current()));

    unsafe {
      queue.push_back(get_ptr(&mut node1));
      queue.push_back(get_ptr(&mut node2));
      queue.push_back(get_ptr(&mut node3));

      // Remove middle
      queue.remove(get_ptr(&mut node2));
    }

    assert_eq!(queue.len(), 2);

    unsafe {
      let popped1 = queue.pop_front();
      assert_eq!(popped1, Some(get_ptr(&mut node1)));

      let popped2 = queue.pop_front();
      assert_eq!(popped2, Some(get_ptr(&mut node3)));
    }
  }

  #[test]
  fn test_remove_head_tail() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    let mut node1 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let mut node2 = make_pinned_node(SyncWaiterNode::new(thread::current()));

    unsafe {
      queue.push_back(get_ptr(&mut node1));
      queue.push_back(get_ptr(&mut node2));

      // Remove head
      queue.remove(get_ptr(&mut node1));
      assert_eq!(queue.len(), 1);
      assert_eq!(queue.head, Some(get_ptr(&mut node2)));

      // Remove tail
      queue.remove(get_ptr(&mut node2));
      assert_eq!(queue.len(), 0);
      assert!(queue.head.is_none());
      assert!(queue.tail.is_none());
    }
  }

  #[test]
  fn test_remove_detached() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    let mut node1 = make_pinned_node(SyncWaiterNode::new(thread::current()));

    unsafe {
      // Should be no-op
      queue.remove(get_ptr(&mut node1));
    }
    assert_eq!(queue.len(), 0);
  }

  #[test]
  fn test_len_empty() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);

    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    unsafe {
      queue.push_back(get_ptr(&mut node));
    }

    assert!(!queue.is_empty());
    assert_eq!(queue.len(), 1);

    unsafe {
      queue.pop_front();
    }
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
  }

  #[test]
  fn test_single_node_lifecycle() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let node_ptr = get_ptr(&mut node);

    unsafe {
      // 1. Push then Remove (via remove)
      queue.push_back(node_ptr);
      assert_eq!(queue.len(), 1);
      assert_eq!(queue.head, Some(node_ptr));
      assert_eq!(queue.tail, Some(node_ptr));

      queue.remove(node_ptr);
      assert_eq!(queue.len(), 0);
      assert!(queue.head.is_none());
      assert!(queue.tail.is_none());
      // Verify node is clean
      assert!(node.prev.is_none());
      assert!(node.next.is_none());

      // 2. Push then Pop (via pop_front)
      queue.push_back(node_ptr);
      assert_eq!(queue.len(), 1);

      let popped = queue.pop_front();
      assert_eq!(popped, Some(node_ptr));
      assert_eq!(queue.len(), 0);
      assert!(node.prev.is_none());
      assert!(node.next.is_none());
    }
  }

  #[test]
  fn test_reinsertion() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let node_ptr = get_ptr(&mut node);

    unsafe {
      // Reuse the same node multiple times
      for _ in 0..3 {
        queue.push_back(node_ptr);
        assert_eq!(queue.len(), 1);
        let popped = queue.pop_front();
        assert_eq!(popped, Some(node_ptr));
        assert_eq!(queue.len(), 0);
      }
    }
  }

  #[test]
  fn test_complex_interleaving() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    // Create 4 nodes
    let mut nodes: Vec<_> = (0..4)
      .map(|_| make_pinned_node(SyncWaiterNode::new(thread::current())))
      .collect();
    let ptrs: Vec<_> = nodes.iter_mut().map(|n| get_ptr(n)).collect();

    unsafe {
      // State: [0]
      queue.push_back(ptrs[0]);
      // State: [0, 1]
      queue.push_back(ptrs[1]);
      // State: [0, 1, 2]
      queue.push_back(ptrs[2]);

      assert_eq!(queue.len(), 3);
      assert_eq!(queue.head, Some(ptrs[0]));
      assert_eq!(queue.tail, Some(ptrs[2]));

      // Remove middle (1): State [0, 2]
      queue.remove(ptrs[1]);
      assert_eq!(queue.len(), 2);
      assert_eq!(queue.head, Some(ptrs[0]));
      assert_eq!(queue.tail, Some(ptrs[2]));

      // Verify links for 0 and 2
      assert_eq!(nodes[0].next, Some(ptrs[2]));
      assert_eq!(nodes[2].prev, Some(ptrs[0]));
      // Verify 1 is detached
      assert!(nodes[1].prev.is_none());
      assert!(nodes[1].next.is_none());

      // Push 3: State [0, 2, 3]
      queue.push_back(ptrs[3]);
      assert_eq!(queue.len(), 3);
      assert_eq!(queue.tail, Some(ptrs[3]));

      // Pop 0: State [2, 3]
      let popped = queue.pop_front();
      assert_eq!(popped, Some(ptrs[0]));
      assert_eq!(queue.len(), 2);
      assert_eq!(queue.head, Some(ptrs[2]));

      // Remove 3 (tail): State [2]
      queue.remove(ptrs[3]);
      assert_eq!(queue.len(), 1);
      assert_eq!(queue.tail, Some(ptrs[2]));
      assert_eq!(queue.head, Some(ptrs[2]));
    }
  }

  #[test]
  fn test_drain_fifo() {
    let mut queue = WaiterQueue::<SyncWaiterNode<usize>>::new();
    let mut nodes: Vec<_> = (0..5)
      .map(|_| make_pinned_node(SyncWaiterNode::new(thread::current())))
      .collect();

    unsafe {
      for node in &mut nodes {
        queue.push_back(get_ptr(node));
      }
    }

    assert_eq!(queue.len(), 5);

    for i in 0..5 {
      unsafe {
        let popped = queue.pop_front();
        assert_eq!(popped, Some(get_ptr(&mut nodes[i])));
      }
    }
    assert!(queue.is_empty());
  }

  #[test]
  fn test_remove_evens() {
    let mut queue = WaiterQueue::<SyncWaiterNode<usize>>::new();
    let count = 6;
    let mut nodes: Vec<_> = (0..count)
      .map(|_| make_pinned_node(SyncWaiterNode::new(thread::current())))
      .collect();
    let ptrs: Vec<_> = nodes.iter_mut().map(|n| get_ptr(n)).collect();

    unsafe {
      for ptr in &ptrs {
        queue.push_back(*ptr);
      }

      // Remove 0, 2, 4
      // List: 0, 1, 2, 3, 4, 5
      queue.remove(ptrs[0]); // Remove Head -> 1, 2, 3, 4, 5
      queue.remove(ptrs[2]); // Remove Middle -> 1, 3, 4, 5
      queue.remove(ptrs[4]); // Remove Middle/Tail -> 1, 3, 5
    }

    assert_eq!(queue.len(), 3);

    unsafe {
      // Verify remaining: 1, 3, 5
      assert_eq!(queue.pop_front(), Some(ptrs[1]));
      assert_eq!(queue.pop_front(), Some(ptrs[3]));
      assert_eq!(queue.pop_front(), Some(ptrs[5]));
      assert!(queue.pop_front().is_none());
    }
  }

  #[test]
  fn test_remove_single_via_remove() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      queue.push_back(ptr);
      assert_eq!(queue.len(), 1);
      queue.remove(ptr);
    }

    assert!(queue.is_empty());
    assert!(queue.head.is_none());
    assert!(queue.tail.is_none());

    // Verify node pointers are cleared
    assert!(node.prev.is_none());
    assert!(node.next.is_none());
  }

  #[test]
  fn test_reuse_node_across_queues() {
    let mut q1 = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut q2 = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      q1.push_back(ptr);
      let popped = q1.pop_front();
      assert_eq!(popped, Some(ptr));

      q2.push_back(ptr);
      assert_eq!(q2.len(), 1);
      q2.remove(ptr);
      assert!(q2.is_empty());
    }
  }

  // --- New Tests ---

  #[test]
  fn test_wake_sync() {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
      // signal that we're about to park
      tx.send(()).unwrap();
      // park until unparked
      thread::park();
      // return
    });

    // wait until child is about to park
    rx.recv().unwrap();

    // now we can safely create a node with the child's Thread handle
    let node = SyncWaiterNode::<()>::new(handle.thread().clone());
    node.wake();

    handle.join().expect("thread failed");
  }

  fn make_flag_waker(flag: Arc<AtomicBool>) -> Waker {
    unsafe fn clone(data: *const ()) -> RawWaker {
      // Reconstruct original Arc (without changing ownership),
      // then clone to create a new Arc and return its raw pointer.
      let orig = Arc::from_raw(data as *const AtomicBool);
      let cloned = orig.clone();
      // restore the original Arc's raw pointer so we don't drop it here:
      let _ = Arc::into_raw(orig);
      let ptr = Arc::into_raw(cloned);
      RawWaker::new(ptr as *const (), &VTABLE)
    }
    unsafe fn wake(data: *const ()) {
      let arc = Arc::from_raw(data as *const AtomicBool);
      arc.store(true, Ordering::SeqCst);
      // Wake consumes the waker's refcount, so we drop arc here.
    }
    unsafe fn wake_by_ref(data: *const ()) {
      let arc = Arc::from_raw(data as *const AtomicBool);
      arc.store(true, Ordering::SeqCst);
      // Put back the Arc to avoid dropping it here.
      let _ = Arc::into_raw(arc);
    }
    unsafe fn drop_raw(data: *const ()) {
      // drop the Arc that was stored in the raw pointer
      let _ = Arc::from_raw(data as *const AtomicBool);
    }

    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);

    let raw = Arc::into_raw(flag) as *const ();
    let raw_waker = RawWaker::new(raw, &VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
  }

  #[test]
  fn test_wake_async() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut node = AsyncWaiterNode::<()>::new();
    node.waker = Some(make_flag_waker(flag.clone()));
    
    node.wake();
    assert!(flag.load(Ordering::SeqCst));
  }

  #[test]
  #[should_panic(expected = "attempted to push a node that is already linked in this queue")]
  fn test_double_insert() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      queue.push_back(ptr);
      // This should panic due to assert!
      queue.push_back(ptr);
    }
  }

  #[test]
  fn test_is_linked_singleton() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      queue.push_back(ptr);
    }

    // Correct check for singleton membership
    assert_eq!(queue.head, Some(ptr));
    assert_eq!(queue.tail, Some(ptr));
    assert_eq!(queue.len(), 1);
  }

  #[test]
  fn test_double_remove() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      queue.push_back(ptr);
      assert_eq!(queue.len(), 1);

      queue.remove(ptr);
      assert_eq!(queue.len(), 0);

      // Second remove should be no-op
      queue.remove(ptr);
      assert_eq!(queue.len(), 0);
    }
  }

  #[test]
  fn test_contains() {
    let mut queue = WaiterQueue::<SyncWaiterNode<()>>::new();
    let mut node = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr = get_ptr(&mut node);

    unsafe {
      assert!(!queue.contains(ptr));

      queue.push_back(ptr);
      assert!(queue.contains(ptr));

      queue.remove(ptr);
      assert!(!queue.contains(ptr));
    }
  }

  #[test]
  fn test_peek_front_and_contains() {
    let mut queue = WaiterQueue::<SyncWaiterNode<i32>>::new();
    let mut node1 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let mut node2 = make_pinned_node(SyncWaiterNode::new(thread::current()));
    let ptr1 = get_ptr(&mut node1);
    let ptr2 = get_ptr(&mut node2);

    unsafe {
      assert!(queue.peek_front().is_none());
      assert!(!queue.contains(ptr1));

      queue.push_back(ptr1);
      assert_eq!(queue.peek_front(), Some(ptr1));
      assert!(queue.contains(ptr1));
      assert!(!queue.contains(ptr2));

      queue.push_back(ptr2);
      assert_eq!(queue.peek_front(), Some(ptr1));
      assert!(queue.contains(ptr1));
      assert!(queue.contains(ptr2));

      queue.pop_front();
      assert_eq!(queue.peek_front(), Some(ptr2));
      assert!(!queue.contains(ptr1));
      assert!(queue.contains(ptr2));
    }
  }

  #[test]
  fn test_stress_concurrent() {
    let queue = Arc::new(std::sync::Mutex::new(WaiterQueue::<SyncWaiterNode<usize>>::new()));
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let mut handles = vec![];

    for _ in 0..4 {
      let q = queue.clone();
      let b = barrier.clone();
      handles.push(thread::spawn(move || {
        let mut node = make_pinned_node(SyncWaiterNode::<usize>::new(thread::current()));
        let ptr = get_ptr(&mut node);
        b.wait();
        for _ in 0..1000 {
          let mut g = q.lock().unwrap();
          unsafe {
            g.push_back(ptr);
            g.remove(ptr);
          }
        }
      }));
    }

    for h in handles {
      h.join().unwrap();
    }

    let g = queue.lock().unwrap();
    assert!(g.is_empty());
  }
}
