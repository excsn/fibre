// src/spmc/ring_buffer.rs

use futures_core::Stream;

use crate::async_util::AtomicWaker;
use crate::error::{CloseError, RecvError, SendError, TryRecvError, TrySendError};
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;
use crate::telemetry;

use core::marker::PhantomPinned;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, Thread};

// --- Telemetry Constants (unchanged) ---
const LOC_P_SEND: &str = "Sender::send";
const LOC_P_TRY_SEND: &str = "Sender::try_send";
const LOC_C_RECV: &str = "Receiver::recv";
const LOC_C_TRY_RECV: &str = "try_recv_internal";
const LOC_WAKEP: &str = "SpmcShared::wake_producer";
const LOC_SYNCWAKER: &str = "sync_waker";

const EVT_P_ENTER_LOOP: &str = "P:EnterLoop";
const EVT_P_GOT_MIN_TAIL: &str = "P:GotMinTail";
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

// sync_waker function (unchanged)
fn sync_waker(thread: Thread) -> Waker {
  const VTABLE: RawWakerVTable = RawWakerVTable::new(
    |data| unsafe {
      // clone
      telemetry::log_event(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_CLONE,
        Some(format!("for {:?}", (*(data as *const Thread)).id())),
      );
      let thread_ptr = Box::into_raw(Box::new((*(data as *const Thread)).clone()));
      RawWaker::new(thread_ptr as *const (), &VTABLE)
    },
    |data| unsafe {
      // wake (consumes)
      let thread_to_wake = Box::from_raw(data as *mut Thread);
      telemetry::log_event(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_WAKE,
        Some(format!("consuming for {:?}", thread_to_wake.id())),
      );
      thread_to_wake.unpark();
    },
    |data| unsafe {
      // wake_by_ref
      let thread_to_wake = &*(data as *const Thread);
      telemetry::log_event(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_WAKE,
        Some(format!("by_ref for {:?}", thread_to_wake.id())),
      );
      thread_to_wake.unpark();
    },
    |data| unsafe {
      // drop
      let thread_to_drop = Box::from_raw(data as *mut Thread);
      telemetry::log_event(
        None,
        LOC_SYNCWAKER,
        EVT_SYNCWAKER_DROP,
        Some(format!("for {:?}", thread_to_drop.id())),
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
    if *self.sequence.get_mut() % 2 == 1 {
      unsafe { self.value.get_mut().assume_init_drop() };
    }
  }
}

#[derive(Debug)]
struct ReceiverCursors {
  list: Vec<Arc<AtomicUsize>>,
}

pub(crate) struct SpmcShared<T: Send + Clone> {
  buffer: Box<[Slot<T>]>,
  capacity: usize,
  head: CachePadded<UnsafeCell<usize>>, // Producer's current write index (logical, unbounded)
  tails: Mutex<ReceiverCursors>,        // List of all active consumer tail pointers
  producer_parked: AtomicBool,
  producer_thread: Mutex<Option<Thread>>,
  producer_waker: AtomicWaker,
  producer_dropped: AtomicBool,
}

impl<T: Send + Clone> fmt::Debug for SpmcShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcShared")
      .field("capacity", &self.capacity)
      .field("head", &unsafe { *self.head.get() })
      .field("tails_count", &self.tails.lock().unwrap().list.len())
      .field(
        "producer_parked",
        &self.producer_parked.load(Ordering::Relaxed),
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
      buffer.push(Slot {
        sequence: AtomicUsize::new(2 * i),
        value: UnsafeCell::new(MaybeUninit::uninit()),
        wakers: Mutex::new(Vec::new()),
      });
    }
    SpmcShared {
      buffer: buffer.into_boxed_slice(),
      capacity,
      head: CachePadded::new(UnsafeCell::new(0)),
      tails: Mutex::new(ReceiverCursors { list: Vec::new() }),
      producer_parked: AtomicBool::new(false),
      producer_thread: Mutex::new(None),
      producer_waker: AtomicWaker::new(),
      producer_dropped: AtomicBool::new(false),
    }
  }

  fn wake_producer(&self) {
    telemetry::log_event(
      None,
      LOC_WAKEP,
      EVT_WAKEP_ENTER,
      Some(format!(
        "prod_parked_flag:{}",
        self.producer_parked.load(Ordering::Relaxed)
      )),
    );
    telemetry::increment_counter(LOC_WAKEP, CTR_WAKEP_CALLS);

    if self.producer_parked.load(Ordering::Acquire) {
      telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_SYNC_ARMED, None);
      if self
        .producer_parked
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
      {
        telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_CAS_OK, None);
        if let Some(thread_to_unpark) = self.producer_thread.lock().unwrap().take() {
          telemetry::log_event(
            None,
            LOC_WAKEP,
            EVT_WAKEP_UNPARK_OK,
            Some(format!("tid:{:?}", thread_to_unpark.id())),
          );
          thread_to_unpark.unpark();
        } else {
          telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_UNPARK_NOTHR, None);
        }
        telemetry::log_event(
          None,
          LOC_WAKEP,
          EVT_WAKEP_WAKE_ASYNC,
          Some("after sync unpark".into()),
        );
        self.producer_waker.wake();
      } else {
        telemetry::log_event(None, LOC_WAKEP, "CASFail (already false)", None);
      }
    } else {
      telemetry::log_event(
        None,
        LOC_WAKEP,
        EVT_WAKEP_WAKE_ASYNC,
        Some("sync not parked".into()),
      );
      self.producer_waker.wake();
    }
  }

  fn wake_all_consumers_from_slots(&self) {
    telemetry::log_event(None, "SpmcShared", EVT_P_DROPPED_WAKE_ALL, None);
    telemetry::increment_counter("SpmcShared", CTR_GLOBAL_CONSUMER_WAKES_ON_P_DROP);
    for slot_idx in 0..self.capacity {
      let slot = &self.buffer[slot_idx];
      let mut wakers_guard = slot.wakers.lock().unwrap();
      for waker in wakers_guard.drain(..) {
        waker.wake();
      }
    }
  }

  fn try_send_internal(&self, value: T) -> Result<(), TrySendError<T>> {
    let current_head_idx = unsafe { *self.head.get() };
    let min_tail_idx;
    {
      let tails_guard = self.tails.lock().unwrap();
      if tails_guard.list.is_empty() {
        telemetry::log_event(
          Some(current_head_idx),
          LOC_P_TRY_SEND,
          EVT_P_NO_CONSUMERS,
          None,
        );
        return Err(TrySendError::Closed(value));
      }
      min_tail_idx = tails_guard
        .list
        .iter()
        .map(|t_arc| t_arc.load(Ordering::Acquire))
        .min()
        .expect("Receiver list checked for non-empty");
    }

    if current_head_idx - min_tail_idx >= self.capacity {
      telemetry::log_event(
        Some(current_head_idx),
        LOC_P_TRY_SEND,
        EVT_P_TRY_SEND_FULL,
        Some(format!(
          "H:{}-MT:{} >= Cap:{}",
          current_head_idx, min_tail_idx, self.capacity
        )),
      );
      return Err(TrySendError::Full(value));
    }

    let slot_idx = current_head_idx % self.capacity;
    let slot = &self.buffer[slot_idx];

    telemetry::log_event(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_WRITE_ITEM,
      Some(format!("slot_idx:{}", slot_idx)),
    );
    unsafe {
      (*slot.value.get()).write(value);
      slot
        .sequence
        .store(2 * current_head_idx + 1, Ordering::Release);
      telemetry::log_event(
        Some(current_head_idx),
        LOC_P_TRY_SEND,
        EVT_P_SEQ_STORED,
        Some(format!(
          "slot_idx:{}, new_seq:{}",
          slot_idx,
          2 * current_head_idx + 1
        )),
      );
    }

    telemetry::log_event(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_WAKE_SLOT,
      Some(format!("slot_idx:{}", slot_idx)),
    );
    telemetry::increment_counter(LOC_P_TRY_SEND, CTR_SLOT_WAKES);
    for waker in slot.wakers.lock().unwrap().drain(..) {
      waker.wake();
    }

    unsafe {
      *self.head.get() = current_head_idx + 1;
    }
    telemetry::log_event(
      Some(current_head_idx),
      LOC_P_TRY_SEND,
      EVT_P_ADVANCE_HEAD,
      Some(format!("new_head:{}", current_head_idx + 1)),
    );
    Ok(())
  }
}

#[derive(Debug)]
pub struct Sender<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send + Clone> Send for Sender<T> {}

#[derive(Debug)]
pub struct AsyncSender<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) closed: AtomicBool,
}
unsafe impl<T: Send + Clone> Send for AsyncSender<T> {}

#[derive(Debug)]
pub struct Receiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
  pub(crate) closed: AtomicBool,
}

#[derive(Debug)]
pub struct AsyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
  pub(crate) closed: AtomicBool,
}

pub(crate) fn new_channel<T: Send + Clone>(capacity: usize) -> (Sender<T>, Receiver<T>) {
  assert!(capacity > 0, "SPMC channel capacity must be > 0");
  let shared = Arc::new(SpmcShared::new(capacity));
  let initial_tail = Arc::new(AtomicUsize::new(0));
  shared
    .tails
    .lock()
    .unwrap()
    .list
    .push(Arc::clone(&initial_tail));
  (
    Sender {
      shared: Arc::clone(&shared),
      closed: AtomicBool::new(false),
    },
    Receiver {
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
    telemetry::log_event(
      Some(current_tail_val),
      LOC_C_TRY_RECV,
      EVT_C_TRY_SUCCESS,
      Some(format!(
        "slot_idx:{}, new_tail:{}",
        slot_idx,
        current_tail_val + 1
      )),
    );
    shared.wake_producer();
    Ok(value)
  } else {
    if shared.producer_dropped.load(Ordering::Acquire) {
      let head = unsafe { *shared.head.get() };
      if current_tail_val >= head {
        telemetry::log_event(
          Some(current_tail_val),
          LOC_C_TRY_RECV,
          EVT_C_TRY_DISCONNECTED,
          Some(format!(
            "slot_idx:{}, producer_dropped:true, head:{}",
            slot_idx, head
          )),
        );
        return Err(TryRecvError::Disconnected);
      }
    }
    telemetry::log_event(
      Some(current_tail_val),
      LOC_C_TRY_RECV,
      EVT_C_TRY_EMPTY,
      Some(format!(
        "slot_idx:{}, got_seq:{}, exp_seq:{}, prod_dropped:{}",
        slot_idx,
        slot_seq,
        2 * current_tail_val + 1,
        shared.producer_dropped.load(Ordering::Relaxed)
      )),
    );
    Err(TryRecvError::Empty)
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

      slot.wakers.lock().unwrap().push(cx.waker().clone());

      match try_recv_internal(shared, tail) {
        Ok(value) => Poll::Ready(Ok(value)),
        Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
        Err(TryRecvError::Empty) => {
          if shared.producer_dropped.load(Ordering::Acquire) {
            let head = unsafe { *shared.head.get() };
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

impl<T: Send + Clone> Receiver<T> {
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
    let consumer_name = thread::current().name().unwrap_or("?").to_string();
    loop {
      match try_recv_internal(&self.shared, &self.tail) {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Empty) => {
          let current_tail_val = self.tail.load(Ordering::Relaxed);
          let slot_idx = current_tail_val % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];

          telemetry::log_event(
            Some(current_tail_val),
            &consumer_name,
            EVT_C_REG_WAKER,
            Some(format!("slot_idx:{}", slot_idx)),
          );
          let waker_for_slot = sync_waker(thread::current());
          slot.wakers.lock().unwrap().push(waker_for_slot);

          match try_recv_internal(&self.shared, &self.tail) {
            Ok(value) => {
              telemetry::log_event(
                Some(current_tail_val),
                &consumer_name,
                EVT_C_GOT_ON_RECHECK,
                None,
              );
              return Ok(value);
            }
            Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
            Err(TryRecvError::Empty) => {
              if self.shared.producer_dropped.load(Ordering::Acquire) {
                let head = unsafe { *self.shared.head.get() };
                if self.tail.load(Ordering::Relaxed) >= head {
                  slot
                    .wakers
                    .lock()
                    .unwrap()
                    .retain(|w| !w.will_wake(&sync_waker(thread::current())));
                  return Err(RecvError::Disconnected);
                }
              }
            }
          }

          telemetry::log_event(
            Some(current_tail_val),
            &consumer_name,
            EVT_C_EXEC_PARK,
            None,
          );
          telemetry::increment_counter(&consumer_name, CTR_C_PARK_ATTEMPTS);
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
    let head = unsafe { *self.shared.head.get() };
    let tail = self.tail.load(Ordering::Acquire);
    head.saturating_sub(tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = unsafe { *self.shared.head.get() };
    let tail = self.tail.load(Ordering::Acquire);
    tail >= head
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> AsyncReceiver<T> {
  pub fn try_recv(&self) -> Result<T, TryRecvError> {
    if self.closed.load(Ordering::Relaxed) {
      return Err(TryRecvError::Disconnected);
    }
    try_recv_internal(&self.shared, &self.tail)
  }

  pub fn recv(&self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
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
    let head = unsafe { *self.shared.head.get() };
    let tail = self.tail.load(Ordering::Acquire);
    head.saturating_sub(tail)
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    let head = unsafe { *self.shared.head.get() };
    let tail = self.tail.load(Ordering::Acquire);
    tail >= head
  }

  #[inline]
  pub fn is_full(&self) -> bool {
    self.len() == self.shared.capacity
  }
}

impl<T: Send + Clone> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    let new_tail_val = self.tail.load(Ordering::Acquire);
    let new_consumer_tail = Arc::new(AtomicUsize::new(new_tail_val));
    self
      .shared
      .tails
      .lock()
      .unwrap()
      .list
      .push(Arc::clone(&new_consumer_tail));
    Self {
      shared: Arc::clone(&self.shared),
      tail: new_consumer_tail,
      closed: AtomicBool::new(false),
    }
  }
}
impl<T: Send + Clone> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    let new_tail_val = self.tail.load(Ordering::Acquire);
    let new_consumer_tail = Arc::new(AtomicUsize::new(new_tail_val));
    self
      .shared
      .tails
      .lock()
      .unwrap()
      .list
      .push(Arc::clone(&new_consumer_tail));
    Self {
      shared: Arc::clone(&self.shared),
      tail: new_consumer_tail,
      closed: AtomicBool::new(false),
    }
  }
}

fn drop_receiver_internal<T: Send + Clone>(shared: &SpmcShared<T>, tail_arc: &Arc<AtomicUsize>) {
  let mut tails_guard = shared.tails.lock().unwrap();
  tails_guard
    .list
    .retain(|t_arc| !Arc::ptr_eq(t_arc, tail_arc));
  drop(tails_guard);
  shared.wake_producer();
}

impl<T: Send + Clone> Drop for Receiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}
impl<T: Send + Clone> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

impl<T: Send + Clone> Sender<T> {
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

      let current_head_idx = unsafe { *self.shared.head.get() };
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_BUFFER_FULL, None);
      telemetry::increment_counter(LOC_P_SEND, CTR_P_PARK_ATTEMPTS);

      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_ARM_PARK, None);
      *self.shared.producer_thread.lock().unwrap() = Some(thread::current());
      self.shared.producer_parked.store(true, Ordering::Release);

      let min_tail_recheck;
      {
        let tails_guard = self.shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          telemetry::log_event(
            Some(current_head_idx),
            LOC_P_SEND,
            EVT_P_NO_CONSUMERS,
            Some("during park recheck".into()),
          );
          self.shared.producer_parked.store(false, Ordering::Relaxed);
          self.shared.producer_thread.lock().unwrap().take();
          drop(current_item.take());
          return Err(SendError::Closed);
        }
        min_tail_recheck = tails_guard
          .list
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("Receiver list still not empty");
      }
      telemetry::log_event(
        Some(current_head_idx),
        LOC_P_SEND,
        EVT_P_RECHECK_SPACE,
        Some(format!("min_tail_recheck:{}", min_tail_recheck)),
      );

      if unsafe { *self.shared.head.get() } - min_tail_recheck < self.shared.capacity {
        telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_PASS, None);
        if self
          .shared
          .producer_parked
          .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
          .is_ok()
        {
          telemetry::log_event(
            Some(current_head_idx),
            LOC_P_SEND,
            EVT_P_CAS_UNARM_SUCCESS,
            None,
          );
          self.shared.producer_thread.lock().unwrap().take();
        } else {
          telemetry::log_event(
            Some(current_head_idx),
            LOC_P_SEND,
            EVT_P_CAS_UNARM_FAIL,
            None,
          );
        }
        continue;
      }

      telemetry::log_event(
        Some(current_head_idx),
        LOC_P_SEND,
        EVT_P_RECHECK_FAIL_PARK,
        None,
      );
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_EXEC_PARK, None);
      sync_util::park_thread();
      telemetry::log_event(
        Some(unsafe { *self.shared.head.get() }),
        LOC_P_SEND,
        EVT_P_UNPARKED,
        None,
      );
    }
  }

  pub fn is_closed(&self) -> bool {
    self.shared.tails.lock().unwrap().list.is_empty()
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
    let head = unsafe { *self.shared.head.get() };
    let tails_guard = self.shared.tails.lock().unwrap();
    if tails_guard.list.is_empty() {
      return 0;
    }
    let min_tail = tails_guard
      .list
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

impl<T: Send + Clone> AsyncSender<T> {
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

  pub fn is_closed(&self) -> bool {
    self.shared.tails.lock().unwrap().list.is_empty()
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
    let head = unsafe { *self.shared.head.get() };
    let tails_guard = self.shared.tails.lock().unwrap();
    if tails_guard.list.is_empty() {
      return 0;
    }
    let min_tail = tails_guard
      .list
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

impl<T: Send + Clone> Drop for Sender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

impl<T: Send + Clone> Drop for AsyncSender<T> {
  fn drop(&mut self) {
    if !self.closed.swap(true, Ordering::AcqRel) {
      self.close_internal();
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send + Clone> {
  producer: &'a AsyncSender<T>,
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

      let current_head_idx = unsafe { *shared.head.get() };
      telemetry::log_event(
        Some(current_head_idx),
        "AsyncP::poll",
        EVT_P_BUFFER_FULL,
        None,
      );
      telemetry::increment_counter("AsyncP::poll", CTR_P_PARK_ATTEMPTS);
      shared.producer_waker.register(cx.waker());

      let min_tail_recheck;
      {
        let tails_guard = shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          this.value = None;
          telemetry::log_event(
            Some(current_head_idx),
            "AsyncP::poll",
            EVT_P_NO_CONSUMERS,
            Some("during park recheck".into()),
          );
          return Poll::Ready(Err(SendError::Closed));
        }
        min_tail_recheck = tails_guard
          .list
          .iter()
          .map(|t_arc| t_arc.load(Ordering::Acquire))
          .min()
          .expect("Receiver list still not empty");
      }
      telemetry::log_event(
        Some(current_head_idx),
        "AsyncP::poll",
        EVT_P_RECHECK_SPACE,
        Some(format!("min_tail_recheck:{}", min_tail_recheck)),
      );

      if unsafe { *shared.head.get() } - min_tail_recheck < shared.capacity {
        telemetry::log_event(
          Some(current_head_idx),
          "AsyncP::poll",
          EVT_P_RECHECK_PASS,
          None,
        );
        continue;
      }
      return Poll::Pending;
    }
  }
}

#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct RecvFuture<'a, T: Send + Clone> {
  receiver: &'a AsyncReceiver<T>,
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

impl<T: Send + Clone> Stream for AsyncReceiver<T> {
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
