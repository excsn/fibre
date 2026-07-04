use futures_core::Stream;

use crate::async_util::AtomicWaker;
use crate::error::{
  BatchSendErrorReason, CloseError, RecvError, SendBatchError, SendError, TryRecvError,
  TrySendBatchError, TrySendError,
};
use crate::internal::cache_padded::CachePadded;
use crate::internal::left_right;
use crate::telemetry;
use crate::{sync_util, RecvErrorTimeout};

use core::marker::PhantomPinned;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

// `hint::spin_loop` routes through the facade so the PARK_CONSUMING waits below
// are scheduler yield-points loom can explore rather than branch-cap blowups.
use crate::internal::sync::{
  fence, hint, thread, Arc, AtomicBool, AtomicU8, AtomicUsize, Mutex, Ordering, Thread,
};

// --- Producer Parking State Machine ---
pub(crate) const PARK_IDLE: u8 = 0; // No one is parked or unparking.
pub(crate) const PARK_PARKED: u8 = 1; // Producer has written its thread handle and is about to park.
pub(crate) const PARK_CONSUMING: u8 = 2; // A consumer has claimed the thread handle and will unpark.

// --- Telemetry Constants ---
const LOC_P_SEND: &str = "BoundedSyncSender::send";
const LOC_P_TRY_SEND: &str = "BoundedSyncSender::try_send";
const LOC_C_TRY_RECV: &str = "try_recv_internal";
const LOC_WAKEP: &str = "SpmcShared::wake_producer";
const LOC_SYNCWAKER: &str = "sync_waker";

const EVT_P_BUFFER_FULL: &str = "P:BufferFull";
const EVT_P_TRY_SEND_FULL: &str = "P:TrySendFull";
const EVT_P_ARM_PARK: &str = "P:ArmPark";
const EVT_P_RECHECK_PASS: &str = "P:RecheckPass";
const EVT_P_RECHECK_SPACE: &str = "P:RecheckSpace";
const EVT_P_RECHECK_FAIL_PARK: &str = "P:RecheckFailPark";
const EVT_P_CAS_UNARM_SUCCESS: &str = "P:CASUnarmSuccess";
const EVT_P_CAS_UNARM_FAIL: &str = "P:CASUnarmFail";
const EVT_P_EXEC_PARK: &str = "P:ExecPark";
const EVT_P_UNPARKED: &str = "P:Unparked";
const EVT_P_WRITE_ITEM: &str = "P:WriteItem";
const EVT_P_SEQ_STORED: &str = "P:SeqStored";
const EVT_P_WAKE_SLOT: &str = "P:WakeSlotWakers";
const EVT_P_ADVANCE_HEAD: &str = "P:AdvanceHead";
const EVT_P_NO_CONSUMERS: &str = "P:NoReceivers";
const EVT_P_DROPPED_WAKE_ALL: &str = "P:DroppedWakeAllReceivers";

const EVT_C_TRY_EMPTY: &str = "C:TryRecvEmpty";
const EVT_C_TRY_DISCONNECTED: &str = "C:TryRecvDisconnected";
const EVT_C_TRY_SUCCESS: &str = "C:TryRecvSuccess";
const EVT_C_REG_WAKER: &str = "C:RegisterWaker";
const EVT_C_GOT_ON_RECHECK: &str = "C:GotOnRecheck";
const EVT_C_EXEC_PARK: &str = "C:ExecPark";
const EVT_C_UNPARKED: &str = "C:Unparked";

const EVT_WAKEP_ENTER: &str = "WakeP:Enter";
const EVT_WAKEP_SYNC_ARMED: &str = "WakeP:SyncArmed";
const EVT_WAKEP_CAS_OK: &str = "WakeP:CAS_OK";
const EVT_WAKEP_UNPARK_OK: &str = "WakeP:UnparkSyncOK";
const EVT_WAKEP_UNPARK_NOTHR: &str = "WakeP:UnparkSyncNoThr";
const EVT_WAKEP_WAKE_ASYNC: &str = "WakeP:WakeAsync";

const EVT_SYNCWAKER_WAKE: &str = "SyncWaker:Wake";
const EVT_SYNCWAKER_CLONE: &str = "SyncWaker:Clone";
const EVT_SYNCWAKER_DROP: &str = "SyncWaker:Drop";

const CTR_P_PARK_ATTEMPTS: &str = "SenderParkAttempts";
const CTR_C_PARK_ATTEMPTS: &str = "ReceiverParkAttempts";
const CTR_WAKEP_CALLS: &str = "WakeSenderCalls";
const CTR_SLOT_WAKES: &str = "SlotWakesIssued";
const CTR_GLOBAL_CONSUMER_WAKES_ON_P_DROP: &str = "GlobalReceiverWakesOnPDrop";

// sync_waker function
fn sync_waker(thread: Thread) -> Waker {
  const VTABLE: RawWakerVTable = RawWakerVTable::new(
    |data| unsafe {
      // clone
      crate::log_event!(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_CLONE,
        "for {:?}",
        (*(data as *const Thread)).id()
      );
      let thread_ptr = Box::into_raw(Box::new((*(data as *const Thread)).clone()));
      RawWaker::new(thread_ptr as *const (), &VTABLE)
    },
    |data| unsafe {
      // wake (consumes)
      let thread_to_wake = Box::from_raw(data as *mut Thread);
      crate::log_event!(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_WAKE,
        "consuming for {:?}",
        thread_to_wake.id()
      );
      thread_to_wake.unpark();
    },
    |data| unsafe {
      // wake_by_ref
      let thread_to_wake = &*(data as *const Thread);
      crate::log_event!(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_WAKE,
        "by_ref for {:?}",
        thread_to_wake.id()
      );
      thread_to_wake.unpark();
    },
    |data| unsafe {
      // drop
      let thread_to_drop = Box::from_raw(data as *mut Thread);
      crate::log_event!(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_DROP,
        "for {:?}",
        thread_to_drop.id()
      );
      drop(thread_to_drop);
    },
  );
  let thread_ptr = Box::into_raw(Box::new(thread));
  unsafe { Waker::from_raw(RawWaker::new(thread_ptr as *const (), &VTABLE)) }
}

pub(crate) struct Slot<T> {
  sequence: AtomicUsize,
  value: UnsafeCell<MaybeUninit<T>>,
  wakers: Mutex<Vec<Waker>>,
}

impl<T> Drop for Slot<T> {
  fn drop(&mut self) {
    if self.sequence.load(Ordering::Relaxed) % 2 == 1 {
      unsafe { self.value.get_mut().assume_init_drop() };
    }
  }
}

pub(crate) struct SpmcShared<T: Send + Clone> {
  buffer: Box<[CachePadded<Slot<T>>]>,
  capacity: usize,
  head: CachePadded<AtomicUsize>, // Producer's current write index (logical, unbounded)
  tails_reader: left_right::ReadHandle<Vec<Arc<AtomicUsize>>>,
  tails_writer: left_right::WriteHandle<Vec<Arc<AtomicUsize>>>,
  tails_mutex: Mutex<()>, // Serialises slow-path consumer registrations (clone/drop)
  producer_parked_sync_flag: CachePadded<AtomicU8>,
  producer_thread_sync: CachePadded<UnsafeCell<Option<Thread>>>,
  producer_waker: AtomicWaker,
  producer_dropped: AtomicBool,
}

impl<T: Send + Clone> fmt::Debug for SpmcShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcShared")
      .field("capacity", &self.capacity)
      .field("head", &self.head.load(Ordering::Relaxed))
      .field("tails_count", &self.tails_reader.enter().len())
      .field(
        "producer_parked_sync_flag",
        &self.producer_parked_sync_flag.load(Ordering::Relaxed),
      )
      .field(
        "producer_dropped",
        &self.producer_dropped.load(Ordering::Relaxed),
      )
      .finish_non_exhaustive()
  }
}

impl<T: Send + Clone> SpmcShared<T> {
  fn new(capacity: usize) -> Self {
    let mut buffer = Vec::with_capacity(capacity);
    for i in 0..capacity {
      buffer.push(CachePadded::new(Slot {
        sequence: AtomicUsize::new(2 * i),
        value: UnsafeCell::new(MaybeUninit::uninit()),
        wakers: Mutex::new(Vec::new()),
      }));
    }
    let (tails_reader, tails_writer) = left_right::new::<Vec<Arc<AtomicUsize>>>();
    SpmcShared {
      buffer: buffer.into_boxed_slice(),
      capacity,
      head: CachePadded::new(AtomicUsize::new(0)),
      tails_reader,
      tails_writer,
      tails_mutex: Mutex::new(()),
      producer_parked_sync_flag: CachePadded::new(AtomicU8::new(PARK_IDLE)),
      producer_thread_sync: CachePadded::new(UnsafeCell::new(None)),
      producer_waker: AtomicWaker::new(),
      producer_dropped: AtomicBool::new(false),
    }
  }

  fn wake_producer(&self) {
    crate::log_event!(
      None,
      LOC_WAKEP,
      EVT_WAKEP_ENTER,
      "prod_parked_flag:{}",
      self.producer_parked_sync_flag.load(Ordering::Relaxed)
    );
    crate::increment_counter!(LOC_WAKEP, CTR_WAKEP_CALLS);

    // SeqCst fence pairs with the producer's store(PARK_PARKED, Release) +
    // fence(SeqCst) sequence: guarantees that if the producer set PARK_PARKED
    // before our tail update became visible, we will observe PARK_PARKED here.
    fence(Ordering::SeqCst);

    if self.producer_parked_sync_flag.load(Ordering::Acquire) == PARK_PARKED {
      crate::log_event!(None, LOC_WAKEP, EVT_WAKEP_SYNC_ARMED);
      if self
        .producer_parked_sync_flag
        .compare_exchange(
          PARK_PARKED,
          PARK_CONSUMING,
          Ordering::AcqRel,
          Ordering::Acquire,
        )
        .is_ok()
      {
        crate::log_event!(None, LOC_WAKEP, EVT_WAKEP_CAS_OK);
        // Safety: CONSUMING state gives us exclusive access to producer_thread_sync;
        // the producer is parked and will not touch the slot until we store PARK_IDLE.
        let thread_to_unpark = unsafe { (*self.producer_thread_sync.get()).take() };
        self
          .producer_parked_sync_flag
          .store(PARK_IDLE, Ordering::Release);
        if let Some(thread) = thread_to_unpark {
          crate::log_event!(
            None,
            LOC_WAKEP,
            EVT_WAKEP_UNPARK_OK,
            "tid:{:?}",
            thread.id()
          );
          thread.unpark();
        } else {
          crate::log_event!(None, LOC_WAKEP, EVT_WAKEP_UNPARK_NOTHR);
        }
        crate::log_event!(None, LOC_WAKEP, EVT_WAKEP_WAKE_ASYNC, "after sync unpark");
      } else {
        crate::log_event!(None, LOC_WAKEP, "CASFail (not PARKED)");
      }
    } else {
      crate::log_event!(None, LOC_WAKEP, EVT_WAKEP_WAKE_ASYNC, "sync not parked");
    }
    self.producer_waker.wake();
  }

  fn wake_all_consumers_from_slots(&self) {
    crate::log_event!(None, "SpmcShared", EVT_P_DROPPED_WAKE_ALL);
    crate::increment_counter!("SpmcShared", CTR_GLOBAL_CONSUMER_WAKES_ON_P_DROP);
    for slot_idx in 0..self.capacity {
      let slot = &self.buffer[slot_idx];
      let mut wakers_guard = slot.wakers.lock();
      for waker in wakers_guard.drain(..) {
        waker.wake();
      }
    }
  }

  fn try_send_internal(&self, value: T) -> Result<(), TrySendError<T>> {
    let current_head_idx = self.head.load(Ordering::Relaxed);
    let min_tail_idx;
    {
      let tails_guard = self.tails_reader.enter();
      if tails_guard.is_empty() {
        crate::log_event!(Some(current_head_idx), LOC_P_TRY_SEND, EVT_P_NO_CONSUMERS);
        return Err(TrySendError::Closed(value));
      }
      min_tail_idx = tails_guard
        .iter()
        .map(|t_arc| t_arc.load(Ordering::Acquire))
        .min()
        .expect("BoundedSyncReceiver list checked for non-empty");
    }

    if current_head_idx - min_tail_idx >= self.capacity {
      crate::log_event!(
        Some(current_head_idx),
        LOC_P_TRY_SEND,
        EVT_P_TRY_SEND_FULL,
        "H:{}-MT:{} >= Cap:{}",
        current_head_idx,
        min_tail_idx,
        self.capacity
      );
      return Err(TrySendError::Full(value));
    }

    let slot_idx = current_head_idx % self.capacity;
    let slot = &self.buffer[slot_idx];

    // If the slot contains an active, initialized item from a previous cycle, we must drop it first to prevent a memory leak.
    if slot.sequence.load(Ordering::Relaxed) % 2 == 1 {
      unsafe {
        (*slot.value.get()).assume_init_drop();
      }
    }

    crate::log_event!(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_WRITE_ITEM,
      "slot_idx:{}",
      slot_idx
    );
    unsafe {
      (*slot.value.get()).write(value);
      slot
        .sequence
        .store(2 * current_head_idx + 1, Ordering::Release);
      crate::log_event!(
        Some(current_head_idx),
        LOC_P_TRY_SEND,
        EVT_P_SEQ_STORED,
        "slot_idx:{}, new_seq:{}",
        slot_idx,
        2 * current_head_idx + 1
      );
    }

    // `head` must be published before the waker list is drained: batch
    // consumers recheck availability via `head` (not the slot sequence) after
    // registering, so a consumer that registers after the drain must be able
    // to observe the new head through the waker-mutex happens-before.
    self.head.store(current_head_idx + 1, Ordering::Release);
    crate::log_event!(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_ADVANCE_HEAD,
      "new_head:{}",
      current_head_idx + 1
    );

    crate::log_event!(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_WAKE_SLOT,
      "slot_idx:{}",
      slot_idx
    );

    let wakers_to_wake: Vec<Waker> = {
      let mut guard = slot.wakers.lock();
      guard.drain(..).collect()
    };

    crate::increment_counter!(LOC_P_TRY_SEND, CTR_SLOT_WAKES);
    for waker in wakers_to_wake {
      waker.wake();
    }

    Ok(())
  }

  /// Producer-side capacity snapshot under the tails read handle.
  /// Returns `Err(())` if all consumers are gone. Because the single producer
  /// is the only writer of `head` and consumer tails only ever advance, the
  /// space returned can only grow until the producer's next write.
  fn producer_space(&self) -> Result<usize, ()> {
    let current_head_idx = self.head.load(Ordering::Relaxed);
    let tails_guard = self.tails_reader.enter();
    if tails_guard.is_empty() {
      return Err(());
    }
    let min_tail_idx = tails_guard
      .iter()
      .map(|t_arc| t_arc.load(Ordering::Acquire))
      .min()
      .expect("BoundedSyncReceiver list checked for non-empty");
    Ok(self.capacity - (current_head_idx - min_tail_idx).min(self.capacity))
  }

  /// Writes up to `k` items from `iter` into consecutive slots. The caller
  /// must guarantee `k` does not exceed the available space (safe to compute
  /// via `producer_space` because space only grows under a single producer).
  ///
  /// All slot values and sequence numbers are written first, then `head` is
  /// advanced exactly once (Release), then each written slot's waker list is
  /// drained and all collected wakers are woken outside the slot locks.
  /// Returns the number of items written.
  fn write_batch_unchecked(&self, iter: &mut impl Iterator<Item = T>, k: usize) -> usize {
    let current_head_idx = self.head.load(Ordering::Relaxed);
    let mut written = 0;

    for i in 0..k {
      let Some(value) = iter.next() else { break };
      let idx = current_head_idx + i;
      let slot = &self.buffer[idx % self.capacity];

      // If the slot still holds an item from a previous lap, drop it in
      // place to prevent a memory leak. No consumer can be reading it: the
      // space check guarantees every consumer tail is past that lap.
      if slot.sequence.load(Ordering::Relaxed) % 2 == 1 {
        unsafe {
          (*slot.value.get()).assume_init_drop();
        }
      }

      unsafe {
        (*slot.value.get()).write(value);
      }
      slot.sequence.store(2 * idx + 1, Ordering::Release);
      written += 1;
    }

    if written == 0 {
      return 0;
    }
    // `head` must be published before any waker list is drained: batch
    // consumers recheck availability via `head` after registering, so a
    // consumer that registers after its slot's drain must observe the new
    // head through the waker-mutex happens-before.
    self
      .head
      .store(current_head_idx + written, Ordering::Release);

    let mut wakers_to_wake: Vec<Waker> = Vec::new();
    for i in 0..written {
      let slot = &self.buffer[(current_head_idx + i) % self.capacity];
      let mut guard = slot.wakers.lock();
      wakers_to_wake.extend(guard.drain(..));
    }
    // Wake outside all slot locks, in one coalesced pass.
    for waker in wakers_to_wake {
      waker.wake();
    }
    written
  }

  /// Batch counterpart of `try_send_internal`: writes up to `limit` items.
  /// Returns `Ok(k)` with the count written (`0` if the buffer is full for
  /// the slowest consumer) or `Err(())` if all consumers are gone.
  fn try_send_batch_internal<I: Iterator<Item = T>>(
    &self,
    iter: &mut I,
    limit: usize,
  ) -> Result<usize, ()> {
    if limit == 0 {
      return Ok(0);
    }
    let space = self.producer_space()?;
    let k = space.min(limit);
    if k == 0 {
      return Ok(0);
    }
    Ok(self.write_batch_unchecked(iter, k))
  }

  /// In-place batch send: drains up to the available space from the front of
  /// `items`. The space check happens *before* the drain begins, so no item
  /// can be lost on a close race. Returns `Ok(k)` or `Err(())` if closed.
  fn try_send_batch_mut_internal(&self, items: &mut Vec<T>) -> Result<usize, ()> {
    if items.is_empty() {
      return Ok(0);
    }
    let space = self.producer_space()?;
    let k = space.min(items.len());
    if k == 0 {
      return Ok(0);
    }
    let mut drain = items.drain(..k);
    let written = self.write_batch_unchecked(&mut drain, k);
    debug_assert_eq!(written, k);
    drop(drain);
    Ok(k)
  }
}

#[derive(Debug)]
pub struct BoundedSyncSender<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send + Clone> Send for BoundedSyncSender<T> {}

#[derive(Debug)]
pub struct BoundedAsyncSender<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send + Clone> Send for BoundedAsyncSender<T> {}

#[derive(Debug)]
pub struct BoundedSyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct BoundedAsyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
  pub(crate) closed: AtomicBool,
}

pub(crate) fn new_channel<T: Send + Clone>(capacity: usize) -> (BoundedSyncSender<T>, BoundedSyncReceiver<T>) {
  assert!(capacity > 0, "SPMC channel capacity must be > 0");
  let shared = Arc::new(SpmcShared::new(capacity));
  let initial_tail = Arc::new(AtomicUsize::new(0));
  shared
    .tails_writer
    .modify(|list| list.push(Arc::clone(&initial_tail)));
  (
    BoundedSyncSender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    BoundedSyncReceiver {
      shared,
      tail: initial_tail,
      closed: AtomicBool::new(false),
    },
  )
}

fn try_recv_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  consumer_tail_idx: &AtomicUsize,
) -> Result<T, TryRecvError> {
  let current_tail_val = consumer_tail_idx.load(Ordering::Relaxed);
  let slot_idx = current_tail_val % shared.capacity;
  let slot = &shared.buffer[slot_idx];
  let slot_seq = slot.sequence.load(Ordering::Acquire);

  if slot_seq == 2 * current_tail_val + 1 {
    let value = unsafe { (*slot.value.get()).assume_init_ref().clone() };
    consumer_tail_idx.store(current_tail_val + 1, Ordering::Release);
    crate::log_event!(
      Some(current_tail_val),
      LOC_C_TRY_RECV,
      EVT_C_TRY_SUCCESS,
      "slot_idx:{}, new_tail:{}",
      slot_idx,
      current_tail_val + 1
    );
    shared.wake_producer();
    Ok(value)
  } else {
    if shared.producer_dropped.load(Ordering::Acquire) {
      let head = shared.head.load(Ordering::Acquire);
      if current_tail_val >= head {
        crate::log_event!(
          Some(current_tail_val),
          LOC_C_TRY_RECV,
          EVT_C_TRY_DISCONNECTED,
          "slot_idx:{}, producer_dropped:true, head:{}",
          slot_idx,
          head
        );
        return Err(TryRecvError::Disconnected);
      }
    }
    crate::log_event!(
      Some(current_tail_val),
      LOC_C_TRY_RECV,
      EVT_C_TRY_EMPTY,
      "slot_idx:{}, got_seq:{}, exp_seq:{}, prod_dropped:{}",
      slot_idx,
      slot_seq,
      2 * current_tail_val + 1,
      shared.producer_dropped.load(Ordering::Relaxed)
    );
    Err(TryRecvError::Empty)
  }
}

/// Batch counterpart of `try_recv_internal`: clones up to `max` available
/// items for this consumer's tail, appending them to `out`, then advances the
/// consumer tail exactly once (Release) and wakes the producer once.
fn try_recv_batch_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  consumer_tail_idx: &AtomicUsize,
  out: &mut Vec<T>,
  max: usize,
) -> Result<usize, TryRecvError> {
  if max == 0 {
    return Ok(0);
  }
  let current_tail_val = consumer_tail_idx.load(Ordering::Relaxed);
  // `head` is published with Release after every batch's sequence/value
  // writes, so an Acquire load is a safe bound on readable items.
  let mut head = shared.head.load(Ordering::Acquire);

  if head <= current_tail_val {
    if shared.producer_dropped.load(Ordering::Acquire) {
      // The producer may have advanced head right before dropping.
      head = shared.head.load(Ordering::Acquire);
      if current_tail_val >= head {
        return Err(TryRecvError::Disconnected);
      }
    } else {
      return Err(TryRecvError::Empty);
    }
  }

  let available = head - current_tail_val;
  let k = available.min(max);
  out.reserve(k);
  for i in 0..k {
    let idx = current_tail_val + i;
    let slot = &shared.buffer[idx % shared.capacity];
    debug_assert_eq!(
      slot.sequence.load(Ordering::Acquire),
      2 * idx + 1,
      "slot sequence must match for items below head"
    );
    out.push(unsafe { (*slot.value.get()).assume_init_ref().clone() });
  }
  consumer_tail_idx.store(current_tail_val + k, Ordering::Release);
  shared.wake_producer();
  Ok(k)
}

/// Async batch receive: mirrors `poll_recv_internal`'s register-then-recheck
/// protocol on the current tail slot's waker list.
fn poll_recv_batch_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  tail: &AtomicUsize,
  cx: &mut Context<'_>,
  out: &mut Vec<T>,
  max: usize,
) -> Poll<Result<usize, RecvError>> {
  if max == 0 {
    return Poll::Ready(Ok(0));
  }
  match try_recv_batch_internal(shared, tail, out, max) {
    Ok(k) => Poll::Ready(Ok(k)),
    Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
    Err(TryRecvError::Empty) => {
      let current_tail_val = tail.load(Ordering::Relaxed);
      let slot_idx = current_tail_val % shared.capacity;
      let slot = &shared.buffer[slot_idx];

      {
        let new_waker = cx.waker();
        let mut wakers_guard = slot.wakers.lock();
        if !wakers_guard.iter().any(|w| w.will_wake(new_waker)) {
          wakers_guard.push(new_waker.clone());
        }
      }

      match try_recv_batch_internal(shared, tail, out, max) {
        Ok(k) => Poll::Ready(Ok(k)),
        Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          if shared.producer_dropped.load(Ordering::Acquire) {
            let head = shared.head.load(Ordering::Acquire);
            if tail.load(Ordering::Relaxed) >= head {
              return Poll::Ready(Err(RecvError::Disconnected));
            }
          }
          Poll::Pending
        }
      }
    }
  }
}

fn poll_recv_internal<T: Send + Clone>(
  shared: &SpmcShared<T>,
  tail: &AtomicUsize,
  cx: &mut Context<'_>,
) -> Poll<Result<T, RecvError>> {
  match try_recv_internal(shared, tail) {
    Ok(value) => Poll::Ready(Ok(value)),
    Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
    Err(TryRecvError::Empty) => {
      let current_tail_val = tail.load(Ordering::Relaxed);
      let slot_idx = current_tail_val % shared.capacity;
      let slot = &shared.buffer[slot_idx];

      {
        let new_waker = cx.waker();
        let mut wakers_guard = slot.wakers.lock();
        if !wakers_guard.iter().any(|w| w.will_wake(new_waker)) {
          wakers_guard.push(new_waker.clone());
        }
      }

      match try_recv_internal(shared, tail) {
        Ok(value) => Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          if shared.producer_dropped.load(Ordering::Acquire) {
            let head = shared.head.load(Ordering::Acquire);
            if tail.load(Ordering::Relaxed) >= head {
              return Poll::Ready(Err(RecvError::Disconnected));
            }
          }
          Poll::Pending
        }
      }
    }
  }
}

impl<T: Send + Clone> BoundedSyncReceiver<T> {
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    try_recv_internal(&self.shared, &self.tail)
  }

  pub fn recv(&self) -> Result<T, RecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    // Only the telemetry macros read this; keep it off the hot path when the
    // `diagnostics` feature is disabled (`String::new()` doesn't allocate).
    #[cfg(feature = "diagnostics")]
    let consumer_name = thread::current().name().unwrap_or("?").to_string();
    #[cfg(not(feature = "diagnostics"))]
    let consumer_name = String::new();
    loop {
      match try_recv_internal(&self.shared, &self.tail) {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Empty) => {
          let current_tail_val = self.tail.load(Ordering::Relaxed);
          let slot_idx = current_tail_val % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];

          crate::log_event!(
            Some(current_tail_val),
            &consumer_name,
            EVT_C_REG_WAKER,
            "slot_idx:{}",
            slot_idx
          );
          let waker_for_slot = sync_waker(thread::current());
          slot.wakers.lock().push(waker_for_slot);

          match try_recv_internal(&self.shared, &self.tail) {
            Ok(value) => {
              crate::log_event!(Some(current_tail_val), &consumer_name, EVT_C_GOT_ON_RECHECK);
              return Ok(value);
            }
            Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
            Err(TryRecvError::Empty) => {
              if self.shared.producer_dropped.load(Ordering::Acquire) {
                let head = self.shared.head.load(Ordering::Acquire);
                if self.tail.load(Ordering::Relaxed) >= head {
                  slot
                    .wakers
                    .lock()
                    .retain(|w| !w.will_wake(&sync_waker(thread::current())));
                  return Err(RecvError::Disconnected);
                }
              }
            }
          }

          crate::log_event!(Some(current_tail_val), &consumer_name, EVT_C_EXEC_PARK);
          crate::increment_counter!(&consumer_name, CTR_C_PARK_ATTEMPTS);
          thread::park();
          telemetry::log_event(
            Some(self.tail.load(Ordering::Relaxed)),
            &consumer_name,
            EVT_C_UNPARKED,
            None,
          );
        }
        Err(TryRecvError::Disconnected) => {
          return Err(RecvError::Disconnected);
        }
      }
    }
  }

  pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvErrorTimeout> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvErrorTimeout::Disconnected);
    }
    let start_time = Instant::now();

    loop {
      match try_recv_internal(&self.shared, &self.tail) {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(RecvErrorTimeout::Disconnected),
        Err(TryRecvError::Empty) => {
          let elapsed = start_time.elapsed();
          if elapsed >= timeout {
            return Err(RecvErrorTimeout::Timeout);
          }
          let remaining_timeout = timeout - elapsed;

          let current_tail_val = self.tail.load(Ordering::Relaxed);
          let slot_idx = current_tail_val % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];
          let waker_for_slot = sync_waker(thread::current());
          slot.wakers.lock().push(waker_for_slot.clone());

          // Re-check after registration
          match try_recv_internal(&self.shared, &self.tail) {
            Ok(value) => {
              // Clean up waker we just pushed
              slot.wakers.lock().retain(|w| !w.will_wake(&waker_for_slot));
              return Ok(value);
            }
            Err(TryRecvError::Disconnected) => {
              // Clean up waker we just pushed
              slot.wakers.lock().retain(|w| !w.will_wake(&waker_for_slot));
              return Err(RecvErrorTimeout::Disconnected);
            }
            Err(TryRecvError::Empty) => {
              if self.shared.producer_dropped.load(Ordering::Acquire) {
                let head = self.shared.head.load(Ordering::Acquire);
                if self.tail.load(Ordering::Relaxed) >= head {
                  // Clean up waker we just pushed
                  slot.wakers.lock().retain(|w| !w.will_wake(&waker_for_slot));
                  return Err(RecvErrorTimeout::Disconnected);
                }
              }
            }
          }

          thread::park_timeout(remaining_timeout);
        }
      }
    }
  }

  /// Attempts to receive up to `max` items without blocking. Each consumer
  /// observes every broadcast item; values are cloned out of the buffer.
  /// Returns 1..=max items in order, `Err(TryRecvError::Empty)`, or
  /// `Err(TryRecvError::Disconnected)` once the producer is gone and this
  /// consumer has drained its view of the buffer.
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number appended. The consumer tail is
  /// advanced once for the whole batch and the producer is woken once.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    try_recv_batch_internal(&self.shared, &self.tail, out, max)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// then draining up to `max` without further waiting.
  pub fn recv_batch(&self, max: usize) -> Result<Vec<T>, RecvError> {
    let mut out = Vec::new();
    self.recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Receives up to `max` items, blocking until at least one is available,
  /// appending them to the end of `out`. Returns the number appended.
  pub fn recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, RecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(RecvError::Disconnected);
    }
    loop {
      match try_recv_batch_internal(&self.shared, &self.tail, out, max) {
        Ok(k) => return Ok(k),
        Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
        Err(TryRecvError::Empty) => {
          // Same slot-waker park protocol as `recv`.
          let current_tail_val = self.tail.load(Ordering::Relaxed);
          let slot_idx = current_tail_val % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];

          let waker_for_slot = sync_waker(thread::current());
          slot.wakers.lock().push(waker_for_slot);

          match try_recv_batch_internal(&self.shared, &self.tail, out, max) {
            Ok(k) => return Ok(k),
            Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
            Err(TryRecvError::Empty) => {
              if self.shared.producer_dropped.load(Ordering::Acquire) {
                let head = self.shared.head.load(Ordering::Acquire);
                if self.tail.load(Ordering::Relaxed) >= head {
                  return Err(RecvError::Disconnected);
                }
              }
            }
          }

          thread::park();
        }
      }
    }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.producer_dropped.load(Ordering::Acquire) && self.is_empty()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    drop_receiver_internal(&self.shared, &self.tail)
  }

  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.tail.load(Ordering::Acquire);
    head.saturating_sub(tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.tail.load(Ordering::Acquire);
    tail >= head
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> BoundedAsyncReceiver<T> {
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    try_recv_internal(&self.shared, &self.tail)
  }

  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
  }

  /// Receives up to `max` items asynchronously. Resolves with between 1 and
  /// `max` cloned items once at least one is available. Cancel-safe.
  pub fn recv_batch(&self, max: usize) -> RecvBatchFuture<'_, T> {
    RecvBatchFuture {
      receiver: self,
      max,
    }
  }

  /// Receives up to `max` items asynchronously, appending them to the end of
  /// `out`. Resolves with the number appended. Cancel-safe.
  pub fn recv_batch_mut<'a>(
    &'a self,
    out: &'a mut Vec<T>,
    max: usize,
  ) -> RecvBatchMutFuture<'a, T> {
    RecvBatchMutFuture {
      receiver: self,
      out,
      max,
    }
  }

  /// Attempts to receive up to `max` items without blocking. Same semantics
  /// as [`BoundedSyncReceiver::try_recv_batch`].
  pub fn try_recv_batch(&self, max: usize) -> Result<Vec<T>, TryRecvError> {
    let mut out = Vec::new();
    self.try_recv_batch_mut(&mut out, max)?;
    Ok(out)
  }

  /// Attempts to receive up to `max` items without blocking, appending them
  /// to the end of `out`. Returns the number appended.
  pub fn try_recv_batch_mut(&self, out: &mut Vec<T>, max: usize) -> Result<usize, TryRecvError> {
    if max == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    try_recv_batch_internal(&self.shared, &self.tail, out, max)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.producer_dropped.load(Ordering::Acquire) && self.is_empty()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity
  }

  pub fn close(&self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&self) {
    drop_receiver_internal(&self.shared, &self.tail)
  }

  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.tail.load(Ordering::Acquire);
    head.saturating_sub(tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = self.shared.head.load(Ordering::Acquire);
    let tail = self.tail.load(Ordering::Acquire);
    tail >= head
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> Clone for BoundedSyncReceiver<T> {
  fn clone(&self) -> Self {
    let new_tail_val = self.tail.load(Ordering::Acquire);
    let new_consumer_tail = Arc::new(AtomicUsize::new(new_tail_val));
    let _lock = self.shared.tails_mutex.lock();
    self
      .shared
      .tails_writer
      .modify(|list| list.push(Arc::clone(&new_consumer_tail)));
    Self {
      shared: Arc::clone(&self.shared),
      tail: new_consumer_tail,
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send + Clone> Clone for BoundedAsyncReceiver<T> {
  fn clone(&self) -> Self {
    let new_tail_val = self.tail.load(Ordering::Acquire);
    let new_consumer_tail = Arc::new(AtomicUsize::new(new_tail_val));
    let _lock = self.shared.tails_mutex.lock();
    self
      .shared
      .tails_writer
      .modify(|list| list.push(Arc::clone(&new_consumer_tail)));
    Self {
      shared: Arc::clone(&self.shared),
      tail: new_consumer_tail,
      closed: AtomicBool::new(false),
    }
  }
}

fn drop_receiver_internal<T: Send + Clone>(shared: &SpmcShared<T>, tail_arc: &Arc<AtomicUsize>) {
  let _lock = shared.tails_mutex.lock();
  shared
    .tails_writer
    .modify(|list| list.retain(|t_arc| !Arc::ptr_eq(t_arc, tail_arc)));
  drop(_lock);
  shared.wake_producer();
}

impl<T: Send + Clone> Drop for BoundedSyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
impl<T: Send + Clone> Drop for BoundedAsyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

impl<T: Send + Clone> BoundedSyncSender<T> {
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    self.shared.try_send_internal(value)
  }

  pub fn send(&self, value: T) -> Result<(), SendError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }

    let mut current_item = Some(value);

    loop {
      let item_to_send = current_item
        .take()
        .expect("Item should always exist at start of send loop");
      match self.shared.try_send_internal(item_to_send) {
        Ok(()) => return Ok(()),
        Err(TrySendError::Closed(_)) => {
          return Err(SendError::Closed);
        }
        Err(TrySendError::Full(returned_item)) => {
          current_item = Some(returned_item);
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      let current_head_idx = self.shared.head.load(Ordering::Relaxed);
      crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_BUFFER_FULL);
      crate::increment_counter!(LOC_P_SEND, CTR_P_PARK_ATTEMPTS);

      crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_ARM_PARK);
      // Safety: we are the sole producer; no consumer can access producer_thread_sync
      // until we store PARK_PARKED below (Release fence).
      unsafe {
        *self.shared.producer_thread_sync.get() = Some(thread::current());
      }
      self
        .shared
        .producer_parked_sync_flag
        .store(PARK_PARKED, Ordering::Release);
      fence(Ordering::SeqCst);

      let min_tail_recheck;
      let buffer_still_full;
      {
        let tails_guard = self.shared.tails_reader.enter();
        if tails_guard.is_empty() {
          crate::log_event!(
            Some(current_head_idx),
            LOC_P_SEND,
            EVT_P_NO_CONSUMERS,
            "during park recheck"
          );
          // Release the read guard before spinning on the parking state machine.
          drop(tails_guard);
          // De-arm: CAS PARK_PARKED→PARK_IDLE ourselves, or wait for a consumer
          // that raced us to PARK_CONSUMING to finish and store PARK_IDLE.
          loop {
            match self.shared.producer_parked_sync_flag.compare_exchange(
              PARK_PARKED,
              PARK_IDLE,
              Ordering::AcqRel,
              Ordering::Acquire,
            ) {
              Ok(_) => {
                // We won the race: clear the thread handle we wrote.
                unsafe {
                  *self.shared.producer_thread_sync.get() = None;
                }
                break;
              }
              Err(PARK_CONSUMING) => hint::spin_loop(),
              Err(_) => break, // Consumer completed handoff; slot already cleared.
            }
          }
          drop(current_item.take());
          return Err(SendError::Closed);
        }
        min_tail_recheck = tails_guard
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("BoundedSyncReceiver list still not empty");
        buffer_still_full =
          self.shared.head.load(Ordering::Relaxed) - min_tail_recheck >= self.shared.capacity;
      }
      crate::log_event!(
        Some(current_head_idx),
        LOC_P_SEND,
        EVT_P_RECHECK_SPACE,
        "min_tail_recheck:{}",
        min_tail_recheck
      );

      if !buffer_still_full {
        crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_PASS);
        // Space appeared: de-arm the parking state machine.
        loop {
          match self.shared.producer_parked_sync_flag.compare_exchange(
            PARK_PARKED,
            PARK_IDLE,
            Ordering::AcqRel,
            Ordering::Acquire,
          ) {
            Ok(_) => {
              crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_CAS_UNARM_SUCCESS);
              unsafe {
                *self.shared.producer_thread_sync.get() = None;
              }
              break;
            }
            Err(PARK_CONSUMING) => {
              // A consumer raced us and is mid-handoff; spin until it stores PARK_IDLE.
              crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_CAS_UNARM_FAIL);
              while self
                .shared
                .producer_parked_sync_flag
                .load(Ordering::Acquire)
                != PARK_IDLE
              {
                hint::spin_loop();
              }
              break;
            }
            Err(_) => break, // Already IDLE.
          }
        }
        continue;
      }

      // Buffer still full: park.
      crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_FAIL_PARK);
      crate::log_event!(Some(current_head_idx), LOC_P_SEND, EVT_P_EXEC_PARK);
      sync_util::park_thread();
      crate::log_event!(
        Some(self.shared.head.load(Ordering::Relaxed)),
        LOC_P_SEND,
        EVT_P_UNPARKED
      );

      // Post-wakeup: resolve any spurious wakeup or mid-CONSUMING races before
      // re-entering the send loop.
      loop {
        match self
          .shared
          .producer_parked_sync_flag
          .load(Ordering::Acquire)
        {
          PARK_IDLE => break, // Normal: consumer completed the full handoff.
          PARK_PARKED => {
            // Spurious wakeup: try to de-arm.
            if self
              .shared
              .producer_parked_sync_flag
              .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Acquire)
              .is_ok()
            {
              unsafe {
                *self.shared.producer_thread_sync.get() = None;
              }
              break;
            }
            // CAS failed: consumer just moved to CONSUMING; spin.
            hint::spin_loop();
          }
          PARK_CONSUMING => hint::spin_loop(),
          _ => break,
        }
      }
    }
  }

  /// Attempts to broadcast a batch without blocking, taking ownership of the
  /// vector. Capacity is bounded by the slowest consumer. `Ok(n)` means every
  /// item was written; otherwise [`TrySendBatchError`] carries the count sent
  /// and the unsent remainder. The whole batch is published with a single
  /// head update and one coalesced waker pass.
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    match self.shared.try_send_batch_internal(&mut iter, total) {
      Err(()) => Err(TrySendBatchError {
        sent: 0,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Closed,
      }),
      Ok(sent) if sent == total => Ok(total),
      Ok(sent) => Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      }),
    }
  }

  /// Broadcasts a batch, blocking whenever the buffer is full for the slowest
  /// consumer, until every item is written or all consumers drop.
  pub fn send_batch(&self, items: Vec<T>) -> Result<usize, SendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendBatchError {
        sent: 0,
        unsent: items,
      });
    }
    let mut iter = items.into_iter();
    let mut sent = 0;
    loop {
      match self.shared.try_send_batch_internal(&mut iter, total - sent) {
        Err(()) => {
          return Err(SendBatchError {
            sent,
            unsent: iter.collect(),
          })
        }
        Ok(k) => {
          sent += k;
          if sent == total {
            return Ok(total);
          }
        }
      }
      if self.park_until_not_full().is_err() {
        return Err(SendBatchError {
          sent,
          unsent: iter.collect(),
        });
      }
    }
  }

  /// Attempts to broadcast a batch in place without blocking, draining sent
  /// items from the front of `items`. Returns `Ok(k)` (`0` if the buffer is
  /// full); `Err(SendError::Closed)` only if all consumers are gone and zero
  /// items were sent by this call.
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    self
      .shared
      .try_send_batch_mut_internal(items)
      .map_err(|()| SendError::Closed)
  }

  /// Broadcasts a batch in place, blocking whenever the buffer is full for
  /// the slowest consumer, draining sent items from the front of `items`. On
  /// `Err(SendError::Closed)`, the unsent items remain in `items`.
  pub fn send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    let mut sent = 0;
    loop {
      match self.shared.try_send_batch_mut_internal(items) {
        Err(()) => return Err(SendError::Closed),
        Ok(k) => {
          sent += k;
          if items.is_empty() {
            return Ok(sent);
          }
        }
      }
      if self.park_until_not_full().is_err() {
        return Err(SendError::Closed);
      }
    }
  }

  /// Arms the producer parking state machine and parks until space frees up
  /// for the slowest consumer or all consumers drop. Returns `Err(())` if all
  /// consumers are gone. Follows the same arm / re-check / park / resolve
  /// protocol as `BoundedSyncSender::send`.
  fn park_until_not_full(&self) -> Result<(), ()> {
    // Arm: write our thread handle and publish PARK_PARKED.
    unsafe {
      *self.shared.producer_thread_sync.get() = Some(thread::current());
    }
    self
      .shared
      .producer_parked_sync_flag
      .store(PARK_PARKED, Ordering::Release);
    fence(Ordering::SeqCst);

    // Re-check under the tails read handle.
    let (no_consumers, buffer_still_full) = {
      let tails_guard = self.shared.tails_reader.enter();
      if tails_guard.is_empty() {
        (true, false)
      } else {
        let min_tail = tails_guard
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("BoundedSyncReceiver list still not empty");
        (
          false,
          self.shared.head.load(Ordering::Relaxed) - min_tail >= self.shared.capacity,
        )
      }
    };

    if no_consumers || !buffer_still_full {
      // De-arm: reclaim the parking cell, or wait out a consumer mid-handoff.
      loop {
        match self.shared.producer_parked_sync_flag.compare_exchange(
          PARK_PARKED,
          PARK_IDLE,
          Ordering::AcqRel,
          Ordering::Acquire,
        ) {
          Ok(_) => {
            unsafe {
              *self.shared.producer_thread_sync.get() = None;
            }
            break;
          }
          Err(PARK_CONSUMING) => hint::spin_loop(),
          Err(_) => break, // Consumer completed the handoff (IDLE).
        }
      }
      return if no_consumers { Err(()) } else { Ok(()) };
    }

    // Buffer still full: park.
    sync_util::park_thread();

    // Post-wakeup: resolve spurious wakeups / mid-CONSUMING races.
    loop {
      match self
        .shared
        .producer_parked_sync_flag
        .load(Ordering::Acquire)
      {
        PARK_IDLE => break,
        PARK_PARKED => {
          if self
            .shared
            .producer_parked_sync_flag
            .compare_exchange(PARK_PARKED, PARK_IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
          {
            unsafe {
              *self.shared.producer_thread_sync.get() = None;
            }
            break;
          }
          hint::spin_loop();
        }
        PARK_CONSUMING => hint::spin_loop(),
        _ => break,
      }
    }
    Ok(())
  }

  pub fn is_closed(&self) -> bool {
    self.shared.tails_reader.enter().is_empty()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity
  }

  pub fn close(&mut self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    self.shared.wake_all_consumers_from_slots();
  }

  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Relaxed);
    let tails_guard = self.shared.tails_reader.enter();
    if tails_guard.is_empty() {
      return 0;
    }
    let min_tail = tails_guard
      .iter()
      .map(|t_arc| t_arc.load(Ordering::Acquire))
      .min()
      .unwrap_or(head);
    head.saturating_sub(min_tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> BoundedAsyncSender<T> {
  pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendError::Closed(value));
    }
    self.shared.try_send_internal(value)
  }

  pub fn send(&self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
      _phantom: PhantomPinned,
    }
  }

  /// Broadcasts a batch asynchronously, taking ownership of the vector.
  /// Resolves with `Ok(n)` once every item is written, or [`SendBatchError`]
  /// if all consumers drop. If the future is dropped after partial progress,
  /// the unsent remainder is dropped; use
  /// [`send_batch_mut`](Self::send_batch_mut) for cancel safety.
  pub fn send_batch(&self, items: Vec<T>) -> SendBatchFuture<'_, T> {
    let total = items.len();
    SendBatchFuture {
      producer: self,
      iter: items.into_iter(),
      total,
      sent: 0,
      _phantom: PhantomPinned,
    }
  }

  /// Broadcasts a batch asynchronously in place, draining sent items from the
  /// front of `items`. Cancel-safe: unsent items remain in `items`.
  pub fn send_batch_mut<'a>(&'a self, items: &'a mut Vec<T>) -> SendBatchMutFuture<'a, T> {
    SendBatchMutFuture {
      producer: self,
      items,
      sent: 0,
      _phantom: PhantomPinned,
    }
  }

  /// Attempts to broadcast a batch without blocking. Same semantics as
  /// [`BoundedSyncSender::try_send_batch`].
  pub fn try_send_batch(&self, items: Vec<T>) -> Result<usize, TrySendBatchError<T>> {
    let total = items.len();
    if total == 0 {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(TrySendBatchError {
        sent: 0,
        unsent: items,
        reason: BatchSendErrorReason::Closed,
      });
    }
    let mut iter = items.into_iter();
    match self.shared.try_send_batch_internal(&mut iter, total) {
      Err(()) => Err(TrySendBatchError {
        sent: 0,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Closed,
      }),
      Ok(sent) if sent == total => Ok(total),
      Ok(sent) => Err(TrySendBatchError {
        sent,
        unsent: iter.collect(),
        reason: BatchSendErrorReason::Full,
      }),
    }
  }

  /// Attempts to broadcast a batch in place without blocking, draining sent
  /// items from the front of `items`. Same semantics as
  /// [`BoundedSyncSender::try_send_batch_mut`].
  pub fn try_send_batch_mut(&self, items: &mut Vec<T>) -> Result<usize, SendError> {
    if items.is_empty() {
      return Ok(0);
    }
    if self.closed.load(Ordering::Relaxed) {
      return Err(SendError::Closed);
    }
    self
      .shared
      .try_send_batch_mut_internal(items)
      .map_err(|()| SendError::Closed)
  }

  pub fn is_closed(&self) -> bool {
    self.shared.tails_reader.enter().is_empty()
  }

  pub fn capacity(&self) -> usize {
    self.shared.capacity
  }

  pub fn close(&mut self) -> Result<(), CloseError> {
    if self
      .closed
      .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
      .is_ok()
    {
      self.close_internal();
      Ok(())
    } else {
      Err(CloseError)
    }
  }

  fn close_internal(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    self.shared.wake_all_consumers_from_slots();
  }

  #[inline]
  pub fn len(&self) -> usize {
    let head = self.shared.head.load(Ordering::Relaxed);
    let tails_guard = self.shared.tails_reader.enter();
    if tails_guard.is_empty() {
      return 0;
    }
    let min_tail = tails_guard
      .iter()
      .map(|t_arc| t_arc.load(Ordering::Acquire))
      .min()
      .unwrap_or(head);
    head.saturating_sub(min_tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> Drop for BoundedSyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

impl<T: Send + Clone> Drop for BoundedAsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send + Clone> {
  producer: &'a BoundedAsyncSender<T>,
  value: Option<T>,
  _phantom: PhantomPinned,
}

impl<'a, T: Send + Clone> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    if this.producer.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(SendError::Closed));
    }
    let shared = &this.producer.shared;

    loop {
      let item_to_send = match this.value.take() {
        Some(v) => v,
        None => return Poll::Ready(Ok(())), // Already completed
      };

      match shared.try_send_internal(item_to_send) {
        Ok(()) => return Poll::Ready(Ok(())),
        Err(TrySendError::Closed(_)) => {
          return Poll::Ready(Err(SendError::Closed));
        }
        Err(TrySendError::Full(returned_item)) => {
          this.value = Some(returned_item);
        }
        Err(TrySendError::Sent(_)) => unreachable!(),
      }

      let current_head_idx = shared.head.load(Ordering::Relaxed);
      crate::log_event!(Some(current_head_idx), "AsyncP::poll", EVT_P_BUFFER_FULL);
      crate::increment_counter!("AsyncP::poll", CTR_P_PARK_ATTEMPTS);
      shared.producer_waker.register(cx.waker());

      let min_tail_recheck;
      {
        let tails_guard = shared.tails_reader.enter();
        if tails_guard.is_empty() {
          this.value = None;
          crate::log_event!(
            Some(current_head_idx),
            "AsyncP::poll",
            EVT_P_NO_CONSUMERS,
            "during park recheck"
          );
          return Poll::Ready(Err(SendError::Closed));
        }
        min_tail_recheck = tails_guard
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("BoundedSyncReceiver list still not empty");
      }
      crate::log_event!(
        Some(current_head_idx),
        "AsyncP::poll",
        EVT_P_RECHECK_SPACE,
        "min_tail_recheck:{}",
        min_tail_recheck
      );

      if shared.head.load(Ordering::Relaxed) - min_tail_recheck < shared.capacity {
        crate::log_event!(Some(current_head_idx), "AsyncP::poll", EVT_P_RECHECK_PASS);
        continue;
      }
      return Poll::Pending;
    }
  }
}

/// Future returned by [`BoundedAsyncSender::send_batch`].
///
/// If dropped before completion, the unsent remainder is dropped.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchFuture<'a, T: Send + Clone> {
  producer: &'a BoundedAsyncSender<T>,
  iter: std::vec::IntoIter<T>,
  total: usize,
  sent: usize,
  _phantom: PhantomPinned,
}

impl<'a, T: Send + Clone> Future for SendBatchFuture<'a, T> {
  type Output = Result<usize, SendBatchError<T>>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.producer.shared;
    loop {
      if this.sent == this.total {
        return Poll::Ready(Ok(this.total));
      }
      if this.producer.closed.load(Ordering::Relaxed) {
        return Poll::Ready(Err(SendBatchError {
          sent: this.sent,
          unsent: this.iter.by_ref().collect(),
        }));
      }

      match shared.try_send_batch_internal(&mut this.iter, this.total - this.sent) {
        Err(()) => {
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }))
        }
        Ok(k) => {
          this.sent += k;
          if this.sent == this.total {
            return Poll::Ready(Ok(this.total));
          }
        }
      }

      // Buffer full for the slowest consumer: register and re-check (same
      // protocol as the single-item `SendFuture`).
      shared.producer_waker.register(cx.waker());

      let min_tail_recheck;
      {
        let tails_guard = shared.tails_reader.enter();
        if tails_guard.is_empty() {
          return Poll::Ready(Err(SendBatchError {
            sent: this.sent,
            unsent: this.iter.by_ref().collect(),
          }));
        }
        min_tail_recheck = tails_guard
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("BoundedSyncReceiver list still not empty");
      }

      if shared.head.load(Ordering::Relaxed) - min_tail_recheck < shared.capacity {
        continue;
      }
      return Poll::Pending;
    }
  }
}

/// Future returned by [`BoundedAsyncSender::send_batch_mut`].
///
/// Cancel-safe: unsent items remain in the caller's vector.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendBatchMutFuture<'a, T: Send + Clone> {
  producer: &'a BoundedAsyncSender<T>,
  items: &'a mut Vec<T>,
  sent: usize,
  _phantom: PhantomPinned,
}

impl<'a, T: Send + Clone> Future for SendBatchMutFuture<'a, T> {
  type Output = Result<usize, SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };
    let shared = &this.producer.shared;
    loop {
      if this.items.is_empty() {
        return Poll::Ready(Ok(this.sent));
      }
      if this.producer.closed.load(Ordering::Relaxed) {
        return Poll::Ready(Err(SendError::Closed));
      }

      match shared.try_send_batch_mut_internal(this.items) {
        Err(()) => return Poll::Ready(Err(SendError::Closed)),
        Ok(k) => {
          this.sent += k;
          if this.items.is_empty() {
            return Poll::Ready(Ok(this.sent));
          }
        }
      }

      shared.producer_waker.register(cx.waker());

      let min_tail_recheck;
      {
        let tails_guard = shared.tails_reader.enter();
        if tails_guard.is_empty() {
          return Poll::Ready(Err(SendError::Closed));
        }
        min_tail_recheck = tails_guard
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("BoundedSyncReceiver list still not empty");
      }

      if shared.head.load(Ordering::Relaxed) - min_tail_recheck < shared.capacity {
        continue;
      }
      return Poll::Pending;
    }
  }
}

/// Future returned by [`BoundedAsyncReceiver::recv_batch`].
///
/// Cancel-safe: items are only consumed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvBatchFuture<'a, T: Send + Clone> {
  receiver: &'a BoundedAsyncReceiver<T>,
  max: usize,
}

impl<'a, T: Send + Clone> Future for RecvBatchFuture<'a, T> {
  type Output = Result<Vec<T>, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let mut out = Vec::new();
    match poll_recv_batch_internal(
      &self.receiver.shared,
      &self.receiver.tail,
      cx,
      &mut out,
      self.max,
    ) {
      Poll::Ready(Ok(_)) => Poll::Ready(Ok(out)),
      Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
      Poll::Pending => Poll::Pending,
    }
  }
}

/// Future returned by [`BoundedAsyncReceiver::recv_batch_mut`].
///
/// Cancel-safe: items are only consumed in the poll that resolves the future.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvBatchMutFuture<'a, T: Send + Clone> {
  receiver: &'a BoundedAsyncReceiver<T>,
  out: &'a mut Vec<T>,
  max: usize,
}

impl<'a, T: Send + Clone> Future for RecvBatchMutFuture<'a, T> {
  type Output = Result<usize, RecvError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = &mut *self;
    if this.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    let max = this.max;
    poll_recv_batch_internal(
      &this.receiver.shared,
      &this.receiver.tail,
      cx,
      this.out,
      max,
    )
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send + Clone> {
  receiver: &'a BoundedAsyncReceiver<T>,
}

impl<'a, T: Send + Clone> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    if self.receiver.closed.load(Ordering::Relaxed) {
      return Poll::Ready(Err(RecvError::Disconnected));
    }
    poll_recv_internal(&self.receiver.shared, &self.receiver.tail, cx)
  }
}

impl<T: Send + Clone> Stream for BoundedAsyncReceiver<T> {
  type Item = T;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    if self.closed.load(Ordering::Relaxed) {
      return Poll::Ready(None);
    }
    let receiver = self.get_mut();
    match poll_recv_internal(&receiver.shared, &receiver.tail, cx) {
      Poll::Ready(Ok(value)) => Poll::Ready(Some(value)),
      Poll::Ready(Err(_)) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }
}

unsafe impl<T: Send + Clone> Send for SpmcShared<T> {}
unsafe impl<T: Send + Clone> Sync for SpmcShared<T> {}
unsafe impl<T: Send> Send for Slot<T> {}
unsafe impl<T: Send> Sync for Slot<T> {}
