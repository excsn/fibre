//! Miri-only diagnostic scaffolding for the bounded MPSC's node-ownership
//! protocol: a global per-node ownership state machine (`BoundedQueue::shadow`)
//! and a non-atomic ownership canary (`Node::owner_canary`), both asserting
//! FREE -> PRODUCER -> PUBLISHED -> CONSUMED -> FREE at every transition so a
//! custody violation (double ownership, premature recycle, ghost follow)
//! panics AT the boundary instead of racing downstream into a val race or
//! wrong value.
//!
//! The struct FIELDS this backs (`Node::owner_canary`, `BoundedQueue::shadow`,
//! `LocalCache::members`) stay declared on their owning structs in `shared.rs`
//! (Rust can't split a struct's fields across files) — this module holds the
//! free-standing constants, the custody logging macro, and every check that
//! doesn't need to live inline at its call site.

use super::shared::{BoundedQueue, LocalCache, Node};

// `Node::owner_canary` states (see the field doc in shared.rs).
#[cfg(miri)]
pub(crate) const OWNER_FREE: u8 = 0;
#[cfg(miri)]
pub(crate) const OWNER_PRODUCER: u8 = 1;
#[cfg(miri)]
pub(crate) const OWNER_LIVE: u8 = 2;

// `BoundedQueue::shadow` global per-node ownership states. A node moves
// FREE -> PRODUCER (grabbed in fill_pool) -> PUBLISHED (publish_run) ->
// CONSUMED (follow_link) -> FREE (push_node/recycle). Every transition is an
// atomic `swap` asserting the previous state, so a double-ownership (e.g.
// fill_pool grabbing a still-LIVE node) or a premature recycle panics AT the
// boundary instead of racing downstream. Unlike `LocalCache::members` this is
// one global table (not per-cache); unlike Relaxed load+store checks a `swap`
// reads-and-writes atomically.
#[cfg(miri)]
pub(crate) const SH_FREE: u32 = 0;
#[cfg(miri)]
pub(crate) const SH_PUBLISHED: u32 = 1;
#[cfg(miri)]
pub(crate) const SH_CONSUMED: u32 = 2;
#[cfg(miri)]
pub(crate) const SH_PRODUCER: u32 = 3;

// Custody logging for miri ghost hunts: every node ownership transition gets
// one stderr line, so a data-race report can be joined against the full
// lifecycle of the offending node index. Caveat: eprintln's stderr lock adds
// synchronization edges, which can in principle hide a race — it didn't hide
// the 0d2dc31 ghost, but if a red test goes green under logging, that itself
// is the finding.
#[cfg(miri)]
macro_rules! custody {
  ($($arg:tt)*) => {{
    // CURRENTLY DISABLED (body commented): under miri every eprintln is a
    // scheduling yield point, and both the two-sided and consumer-only
    // variants turned the batch_paths_recycle_chunks race green across 32
    // seeds. Re-enable by uncommenting to resume a custody hunt.
    //
    // One-sided when enabled: only the (named) test/consumer thread logs, so
    // producer-side stderr-lock edges don't manufacture the very
    // producer->consumer happens-before under investigation.
    // let t = std::thread::current();
    // if t.name().is_some() {
    //   eprintln!("[custody {:?}] {}", t.id(), format_args!($($arg)*));
    // }
  }};
}
#[cfg(not(miri))]
macro_rules! custody {
  ($($arg:tt)*) => {};
}
pub(crate) use custody;

/// FREE -> PRODUCER, plus the canary mark, for a node grabbed OUTSIDE
/// `fill_pool`'s chunk walk (the cap-1 probe pops the chunk stack directly).
/// Panics on double ownership.
#[cfg(miri)]
pub(crate) fn mark_producer_grab<T: Send>(shared: &BoundedQueue<T>, node: *mut Node<T>) {
  use std::sync::atomic::Ordering::Relaxed;
  let idx = shared.ptr_to_idx(node);
  let prev = shared.shadow[idx as usize].swap(SH_PRODUCER, Relaxed);
  if prev != SH_FREE {
    panic!(
      "DOUBLE OWNERSHIP: probe_pop grabbed node {} but shadow state was {} (want FREE={}); it is still owned elsewhere",
      idx, prev, SH_FREE
    );
  }
  unsafe {
    *(*node).owner_canary.get() = OWNER_PRODUCER;
  }
}

/// Verifies one node of a freshly-popped chunk in `fill_pool`'s walk: the
/// chain must be exactly `chunk_len` distinct, not-yet-a-member nodes, each
/// FREE -> PRODUCER. Panics naming the violation (chain too short, overlap
/// with this cache, or double ownership) instead of racing downstream.
#[cfg(miri)]
pub(crate) fn fill_pool_verify_node<T: Send>(
  shared: &BoundedQueue<T>,
  cache: &mut LocalCache<T>,
  head_idx: u32,
  chunk_len: usize,
  node: *mut Node<T>,
  pos: usize,
) {
  use std::sync::atomic::Ordering::Relaxed;
  if node.is_null() {
    panic!(
      "CHUNK SHORT: chunk head={} declared len={} but free chain ended after {} nodes",
      head_idx, chunk_len, pos
    );
  }
  let idx = shared.ptr_to_idx(node);
  if !cache.members.insert(idx) {
    panic!(
      "CHUNK OVERLAP: node {} (pos {} of chunk head={} len={}) already in this cache",
      idx, pos, head_idx, chunk_len
    );
  }
  // A producer now owns this node: FREE -> PRODUCER. If the previous state is
  // not FREE, this node is simultaneously owned elsewhere (still LIVE /
  // CONSUMED / already PRODUCER) — the double-ownership bug, caught HERE at
  // the grab instead of racing in follow_link.
  let prev = shared.shadow[idx as usize].swap(SH_PRODUCER, Relaxed);
  if prev != SH_FREE {
    panic!(
      "DOUBLE OWNERSHIP: fill_pool grabbed node {} (pos {} of chunk head={} len={}) but its shadow state was {} (want FREE={}); it is still owned by producer/consumer",
      idx, pos, head_idx, chunk_len, prev, SH_FREE
    );
  }
  // Canary mirrors the state for the follow_link data-race probe.
  unsafe {
    *(*node).owner_canary.get() = OWNER_PRODUCER;
  }
}

/// `pop_node`'s membership bookkeeping: the popped node leaves this cache.
#[cfg(miri)]
pub(crate) fn pool_pop_remove_member<T: Send>(
  shared: &BoundedQueue<T>,
  cache: &mut LocalCache<T>,
  node: *mut Node<T>,
) {
  cache.members.remove(&shared.ptr_to_idx(node));
}

/// `push_node`'s membership + shadow bookkeeping. `push_node` is dual-use: the
/// CONSUMER recycling a CONSUMED node (-> FREE), and a PRODUCER stashing a
/// freshly-grabbed node back into the shared cache (the `allocate_node` path
/// when the pool empties mid-batch) — that node is still PRODUCER-owned and
/// must stay PRODUCER. FREE is the initial stub. Only PUBLISHED here is the
/// real bug: recycling a still-live node.
#[cfg(miri)]
pub(crate) fn push_node_check<T: Send>(
  shared: &BoundedQueue<T>,
  cache: &mut LocalCache<T>,
  node: *mut Node<T>,
) {
  use std::sync::atomic::Ordering::Relaxed;
  let idx = shared.ptr_to_idx(node);
  if !cache.members.insert(idx) {
    panic!(
      "DOUBLE FREE: node {} pushed into a cache it is already a member of",
      idx
    );
  }
  let i = idx as usize;
  let prev = shared.shadow[i].load(Relaxed);
  if prev == SH_PUBLISHED {
    panic!(
      "PREMATURE RECYCLE: freeing node {} whose shadow state is PUBLISHED (still live) — it is being recycled before it was consumed",
      i
    );
  }
  if prev == SH_CONSUMED {
    shared.shadow[i].store(SH_FREE, Relaxed);
  }
}

/// `flush`'s pre-release check: before a cache's free-chain becomes
/// producer-grabbable, every node in it must already be FREE (placed there by
/// `push_node`'s CONSUMED->FREE). Bisects consumer-side corruption (count/
/// chunk_len over-reach, or a next-linkage sweeping in a node `push_node`
/// never recycled) from a fill_pool-side double-grab, localized to this
/// cache, before the chunk_stack.
#[cfg(miri)]
pub(crate) fn flush_verify_chain<T: Send>(shared: &BoundedQueue<T>, cache: &LocalCache<T>) {
  use std::sync::atomic::Ordering::Relaxed;
  let mut curr = cache.head;
  for i in 0..cache.count {
    if curr.is_null() {
      panic!(
        "FLUSH CHAIN SHORT: chunk head={} count={} but free chain ended after {} nodes",
        shared.ptr_to_idx(cache.head),
        cache.count,
        i
      );
    }
    let idx = shared.ptr_to_idx(curr);
    let st = shared.shadow[idx as usize].load(Relaxed);
    if st != SH_FREE {
      panic!(
        "FLUSH NON-FREE: node {} (pos {} of {}) is being flushed with shadow {} (want FREE={}) — the consumer's free-chain includes a node push_node never recycled",
        idx, i, cache.count, st, SH_FREE
      );
    }
    curr = unsafe { shared.next_of(curr) };
  }
}

/// Mirrors a manual detach (the batch send paths take a run of nodes off the
/// chain directly instead of via `pop_node`).
#[cfg(miri)]
pub(crate) fn shadow_detach<T: Send>(
  shared: &BoundedQueue<T>,
  cache: &mut LocalCache<T>,
  first: *mut Node<T>,
  k: usize,
) {
  let mut curr = first;
  for i in 0..k {
    if curr.is_null() {
      panic!("DETACH walked off chain end at pos {} of k={}", i, k);
    }
    let idx = shared.ptr_to_idx(curr);
    if !cache.members.remove(&idx) {
      panic!("DETACH of non-member node {} (pos {} of k={})", idx, i, k);
    }
    curr = unsafe { shared.next_of(curr) };
  }
}

/// `follow_link`'s shadow check: PUBLISHED -> CONSUMED, Relaxed atomics add NO
/// happens-before edges on their own — the real protocol's own Release/
/// Acquire also carries the shadow store on every legitimate transition, so a
/// stale read here is only possible when synchronization is absent.
#[cfg(miri)]
pub(crate) fn check_follow_shadow<T: Send>(
  shared: &BoundedQueue<T>,
  tail: *mut Node<T>,
  next: *mut Node<T>,
) {
  use std::sync::atomic::Ordering::Relaxed;
  let t = shared.ptr_to_idx(next) as usize;
  let prev = shared.shadow[t].swap(SH_CONSUMED, Relaxed);
  if prev != SH_PUBLISHED {
    panic!(
      "GHOST: follow out of tail={} into node {} but shadow state was {} (want PUBLISHED={}); the tail reached a node that is not a live published node",
      shared.ptr_to_idx(tail),
      t,
      prev,
      SH_PUBLISHED
    );
  }
}

/// `follow_link`'s no-false-negative ghost probe: this non-atomic read pairs
/// with the producer's non-atomic canary writes (`fill_pool`/`publish_run`).
/// On a legit follow the Acquire load of the junction synchronized-with the
/// producer's Release, so this read is happens-before those writes and is
/// race-free; on a ghost follow (stale link, no hb) miri reports the read
/// against the racing producer write at THIS instruction, with a follow_link
/// <-> fill_pool/publish_run backtrace that names the mechanism. If it
/// somehow does not race, a non-LIVE value still means the tail walked into
/// non-live memory. Silence here is meaningful (unlike the shadow check).
#[cfg(miri)]
pub(crate) fn check_follow_canary<T: Send>(
  shared: &BoundedQueue<T>,
  tail: *mut Node<T>,
  next: *mut Node<T>,
) {
  let o = unsafe { *(*next).owner_canary.get() };
  if o != OWNER_LIVE {
    panic!(
      "OWNERSHIP: consumer at tail={} followed into node {} whose owner canary = {} (want LIVE=2; 1=producer-grabbed, 0=free) — the tail walked into non-live memory",
      shared.ptr_to_idx(tail),
      shared.ptr_to_idx(next),
      o
    );
  }
}

/// `release_chunk`'s reachability check: every node about to become
/// producer-grabbable must be strictly behind the consumer — never the
/// current tail nor the live `tail.next`. If one is, we are releasing a node
/// the consumer can still reach (the premature-recycle hypothesis); panic
/// naming it BEFORE it hits the chunk stack. Runs on the (single) consumer
/// thread.
#[cfg(miri)]
pub(crate) fn check_release_reachability<T: Send>(
  shared: &BoundedQueue<T>,
  chunk_head: *mut Node<T>,
  released: usize,
) {
  use std::sync::atomic::Ordering::Relaxed;
  unsafe {
    let tail = *shared.tail.get();
    let tail_next = (*tail).next.load(Relaxed);
    let mut curr = chunk_head;
    for i in 0..released {
      if curr == tail || curr == tail_next {
        panic!(
          "PREMATURE RELEASE: releasing node {} (pos {} of {}) that is the current consumer tail={} or its live tail.next={} — the consumer can still reach it",
          shared.ptr_to_idx(curr),
          i,
          released,
          shared.ptr_to_idx(tail),
          if tail_next.is_null() {
            u32::MAX
          } else {
            shared.ptr_to_idx(tail_next)
          }
        );
      }
      curr = shared.next_of(curr);
    }
  }
}

/// `publish_run`'s per-node transition: PRODUCER -> PUBLISHED, plus the canary
/// mark. Panics if the node wasn't actually producer-owned (publishing a node
/// this producer never grabbed, or a double publish).
#[cfg(miri)]
pub(crate) fn shadow_publish<T: Send>(shared: &BoundedQueue<T>, node: *mut Node<T>) {
  use std::sync::atomic::Ordering::Relaxed;
  let i = shared.ptr_to_idx(node) as usize;
  let prev = shared.shadow[i].swap(SH_PUBLISHED, Relaxed);
  if prev != SH_PRODUCER {
    panic!(
      "CUSTODY: publishing node {} but shadow state was {} (want PRODUCER={}; publishing a node this producer never grabbed, or a double publish)",
      i, prev, SH_PRODUCER
    );
  }
  unsafe {
    *(*node).owner_canary.get() = OWNER_LIVE;
  }
}
