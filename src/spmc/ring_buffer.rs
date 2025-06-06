// src/spmc/ring_buffer.rs

use crate::async_util::AtomicWaker;
use crate::error::{RecvError, SendError, TryRecvError}; // TryRecvError is important here
use crate::internal::cache_padded::CachePadded;
use crate::sync_util;
use crate::telemetry;

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
const LOC_P_SEND: &str = "Producer::send";
const LOC_C_RECV: &str = "Receiver::recv";
const LOC_C_TRY_RECV: &str = "try_recv_internal";
const LOC_WAKEP: &str = "SpmcShared::wake_producer";
const LOC_SYNCWAKER: &str = "sync_waker";

const EVT_P_ENTER_LOOP: &str = "P:EnterLoop";
const EVT_P_GOT_MIN_TAIL: &str = "P:GotMinTail";
const EVT_P_BUFFER_FULL: &str = "P:BufferFull";
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
const EVT_P_NO_CONSUMERS: &str = "P:NoConsumers";
const EVT_P_DROPPED_WAKE_ALL: &str = "P:DroppedWakeAllConsumers"; // New event

const EVT_C_TRY_EMPTY: &str = "C:TryRecvEmpty";
const EVT_C_TRY_DISCONNECTED: &str = "C:TryRecvDisconnected"; // New event
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

const CTR_P_PARK_ATTEMPTS: &str = "ProducerParkAttempts";
const CTR_C_PARK_ATTEMPTS: &str = "ConsumerParkAttempts";
const CTR_WAKEP_CALLS: &str = "WakeProducerCalls";
const CTR_SLOT_WAKES: &str = "SlotWakesIssued";
const CTR_GLOBAL_CONSUMER_WAKES_ON_P_DROP: &str = "GlobalConsumerWakesOnPDrop"; // New counter


// sync_waker function (unchanged)
fn sync_waker(thread: Thread) -> Waker {
  const VTABLE: RawWakerVTable = RawWakerVTable::new(
    |data| unsafe { // clone
      telemetry::log_event(None, LOC_SYNCWAKER, EVT_SYNCWAKER_CLONE, Some(format!("for {:?}", (*(data as *const Thread)).id())));
      let thread_ptr = Box::into_raw(Box::new((*(data as *const Thread)).clone()));
      RawWaker::new(thread_ptr as *const (), &VTABLE)
    },
    |data| unsafe { // wake (consumes)
      let thread_to_wake = Box::from_raw(data as *mut Thread);
      telemetry::log_event(None, LOC_SYNCWAKER, EVT_SYNCWAKER_WAKE, Some(format!("consuming for {:?}", thread_to_wake.id())));
      thread_to_wake.unpark();
    },
    |data| unsafe { // wake_by_ref
      let thread_to_wake = &*(data as *const Thread);
      telemetry::log_event(None, LOC_SYNCWAKER, EVT_SYNCWAKER_WAKE, Some(format!("by_ref for {:?}", thread_to_wake.id())));
      thread_to_wake.unpark();
    },
    |data| unsafe { // drop
      let thread_to_drop = Box::from_raw(data as *mut Thread);
      telemetry::log_event(None, LOC_SYNCWAKER, EVT_SYNCWAKER_DROP, Some(format!("for {:?}", thread_to_drop.id())));
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
struct ConsumerCursors {
  list: Vec<Arc<AtomicUsize>>,
}

pub(crate) struct SpmcShared<T: Send + Clone> {
  buffer: Box<[Slot<T>]>,
  capacity: usize,
  head: CachePadded<UnsafeCell<usize>>,
  tails: Mutex<ConsumerCursors>,
  producer_parked: AtomicBool,
  producer_thread: Mutex<Option<Thread>>,
  producer_waker: AtomicWaker,
  producer_dropped: AtomicBool, // New field
}

impl<T: Send + Clone> fmt::Debug for SpmcShared<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("SpmcShared")
      .field("capacity", &self.capacity)
      .field("head", &unsafe { *self.head.get() })
      .field("tails_count", &self.tails.lock().unwrap().list.len())
      .field("producer_parked", &self.producer_parked.load(Ordering::Relaxed))
      .field("producer_dropped", &self.producer_dropped.load(Ordering::Relaxed)) // Debug print new field
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
      tails: Mutex::new(ConsumerCursors { list: Vec::new() }),
      producer_parked: AtomicBool::new(false),
      producer_thread: Mutex::new(None),
      producer_waker: AtomicWaker::new(),
      producer_dropped: AtomicBool::new(false), // Initialize new field
    }
  }

  fn wake_producer(&self) {
    // (wake_producer logic unchanged)
    telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_ENTER, Some(format!("prod_parked_flag:{}", self.producer_parked.load(Ordering::Relaxed))));
    telemetry::increment_counter(LOC_WAKEP, CTR_WAKEP_CALLS);

    if self.producer_parked.load(Ordering::Acquire) {
      telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_SYNC_ARMED, None);
      if self.producer_parked.compare_exchange(
        true,
        false,
        Ordering::AcqRel,
        Ordering::Acquire,
      ).is_ok() {
        telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_CAS_OK, None);
        if let Some(thread_to_unpark) = self.producer_thread.lock().unwrap().take() {
          telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_UNPARK_OK, Some(format!("tid:{:?}", thread_to_unpark.id())));
          thread_to_unpark.unpark();
        } else {
          telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_UNPARK_NOTHR, None);
        }
        telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_WAKE_ASYNC, Some("after sync unpark".into()));
        self.producer_waker.wake();
      } else {
        telemetry::log_event(None, LOC_WAKEP, "CASFail (already false)", None);
      }
    } else {
      telemetry::log_event(None, LOC_WAKEP, EVT_WAKEP_WAKE_ASYNC, Some("sync not parked".into()));
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
}

#[derive(Debug)]
pub struct Producer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>, // Makes it !Sync
}
unsafe impl<T: Send + Clone> Send for Producer<T> {}

#[derive(Debug)]
pub struct AsyncProducer<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) _phantom: PhantomData<*const ()>, // Makes it !Sync
}
unsafe impl<T: Send + Clone> Send for AsyncProducer<T> {}

// Receiver structs (unchanged)
#[derive(Debug)]
pub struct Receiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}

#[derive(Debug)]
pub struct AsyncReceiver<T: Send + Clone> {
  pub(crate) shared: Arc<SpmcShared<T>>,
  pub(crate) tail: Arc<AtomicUsize>,
}


pub(crate) fn new_channel<T: Send + Clone>(capacity: usize) -> (Producer<T>, Receiver<T>) {
  // (new_channel logic unchanged)
  assert!(capacity > 0, "SPMC channel capacity must be > 0");
  let shared = Arc::new(SpmcShared::new(capacity));
  let initial_tail = Arc::new(AtomicUsize::new(0));
  shared.tails.lock().unwrap().list.push(Arc::clone(&initial_tail));
  (
    Producer {
      shared: Arc::clone(&shared),
      _phantom: PhantomData,
    },
    Receiver {
      shared,
      tail: initial_tail,
    },
  )
}

// Modified try_recv_internal
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
    telemetry::log_event(Some(current_tail_val), LOC_C_TRY_RECV, EVT_C_TRY_SUCCESS, Some(format!("slot_idx:{}, new_tail:{}", slot_idx, current_tail_val + 1)));
    shared.wake_producer();
    Ok(value)
  } else {
    // Item not ready. Check if producer is dropped.
    if shared.producer_dropped.load(Ordering::Acquire) {
        // If producer is dropped and item is not ready, channel is disconnected.
        // Note: There's a subtle race here. If producer drops, sets flag,
        // wakes consumers, consumer A wakes, reads an item, consumer B wakes,
        // sees flag but item was for A. Consumer B should then re-evaluate.
        // The sequence check (slot_seq == 2 * current_tail_val + 1) should still be primary.
        // This `producer_dropped` check is for when the slot is *truly* empty for this consumer
        // AND no more items will ever come.
        let head = unsafe { *shared.head.get() };
        if current_tail_val >= head { // Ensure all written items are consumed
            telemetry::log_event(Some(current_tail_val), LOC_C_TRY_RECV, EVT_C_TRY_DISCONNECTED, Some(format!("slot_idx:{}, producer_dropped:true, head:{}", slot_idx, head)));
            return Err(TryRecvError::Disconnected);
        }
    }
    // If not disconnected, then it's just empty.
    telemetry::log_event(Some(current_tail_val), LOC_C_TRY_RECV, EVT_C_TRY_EMPTY, Some(format!("slot_idx:{}, got_seq:{}, exp_seq:{}, prod_dropped:{}", slot_idx, slot_seq, 2 * current_tail_val + 1, shared.producer_dropped.load(Ordering::Relaxed))));
    Err(TryRecvError::Empty)
  }
}

// Receiver method implementations (unchanged, they rely on try_recv_internal)
impl<T: Send + Clone> Receiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }

  pub fn recv(&mut self) -> Result<T, RecvError> {
    let consumer_name = thread::current().name().unwrap_or("?").to_string();
    loop {
      match try_recv_internal(&self.shared, &self.tail) {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Empty) => {
          let current_tail_val = self.tail.load(Ordering::Relaxed);
          let slot_idx = current_tail_val % self.shared.capacity;
          let slot = &self.shared.buffer[slot_idx];
          
          telemetry::log_event(Some(current_tail_val), &consumer_name, EVT_C_REG_WAKER, Some(format!("slot_idx:{}", slot_idx)));
          let waker_for_slot = sync_waker(thread::current());
          slot.wakers.lock().unwrap().push(waker_for_slot);

          // Re-check after registering waker to prevent lost wakeups
          match try_recv_internal(&self.shared, &self.tail) {
              Ok(value) => {
                telemetry::log_event(Some(current_tail_val), &consumer_name, EVT_C_GOT_ON_RECHECK, None);
                // Waker was registered but not used, remove it?
                // It's simpler to let it be, it will be a no-op wake.
                return Ok(value);
              }
              Err(TryRecvError::Disconnected) => return Err(RecvError::Disconnected),
              Err(TryRecvError::Empty) => {
                // Still empty, proceed to park
              }
          }
          
          telemetry::log_event(Some(current_tail_val), &consumer_name, EVT_C_EXEC_PARK, None);
          telemetry::increment_counter(&consumer_name, CTR_C_PARK_ATTEMPTS);
          thread::park();
          telemetry::log_event(Some(self.tail.load(Ordering::Relaxed)), &consumer_name, EVT_C_UNPARKED, None);
        }
        Err(TryRecvError::Disconnected) => {
          return Err(RecvError::Disconnected);
        }
      }
    }
  }
}

impl<T: Send + Clone> AsyncReceiver<T> {
  pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
    try_recv_internal(&self.shared, &self.tail)
  }
  pub fn recv(&mut self) -> RecvFuture<'_, T> {
    RecvFuture { receiver: self }
  }
}


// Clone implementations (unchanged)
fn clone_receiver_internal<T: Send + Clone>(
  shared: &Arc<SpmcShared<T>>,
  current_consumer_tail: &Arc<AtomicUsize>,
) -> Arc<AtomicUsize> {
  let new_tail_val = current_consumer_tail.load(Ordering::Acquire);
  let new_consumer_tail = Arc::new(AtomicUsize::new(new_tail_val));
  shared.tails.lock().unwrap().list.push(Arc::clone(&new_consumer_tail));
  new_consumer_tail
}

impl<T: Send + Clone> Clone for Receiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver_internal(&self.shared, &self.tail),
    }
  }
}
impl<T: Send + Clone> Clone for AsyncReceiver<T> {
  fn clone(&self) -> Self {
    Self {
      shared: Arc::clone(&self.shared),
      tail: clone_receiver_internal(&self.shared, &self.tail),
    }
  }
}

// Drop implementations for Receivers (unchanged)
fn drop_receiver_internal<T: Send + Clone>(shared: &SpmcShared<T>, tail_arc: &Arc<AtomicUsize>) {
  let mut tails_guard = shared.tails.lock().unwrap();
  tails_guard.list.retain(|t_arc| !Arc::ptr_eq(t_arc, tail_arc));
  drop(tails_guard);
  shared.wake_producer(); // Important to wake producer if it was blocked by this consumer
}

impl<T: Send + Clone> Drop for Receiver<T> {
  fn drop(&mut self) {
    drop_receiver_internal(&self.shared, &self.tail)
  }
}
impl<T: Send + Clone> Drop for AsyncReceiver<T> {
  fn drop(&mut self) {
    drop_receiver_internal(&self.shared, &self.tail)
  }
}


// Producer method implementations (unchanged)
impl<T: Send + Clone> Producer<T> {
  pub fn send(&mut self, value: T) -> Result<(), SendError> {
    // (send logic unchanged, but it now implicitly benefits from producer_dropped
    // because if consumers see producer_dropped=true, they might unblock the producer
    // by transitioning to Disconnected and no longer waiting for items.)
    // Producer also returns SendError::Closed if no consumers.
    let mut current_head_idx = unsafe { *self.shared.head.get() };
    telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_ENTER_LOOP, None);

    loop {
      let min_tail_idx;
      {
        let tails_guard = self.shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_NO_CONSUMERS, None);
          return Err(SendError::Closed); // No consumers, channel is closed for sending.
        }
        min_tail_idx = tails_guard.list.iter()
            .map(|t_arc| t_arc.load(Ordering::Acquire))
            .min()
            .expect("Consumer list checked for non-empty");
        telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_GOT_MIN_TAIL, Some(format!("min_tail:{}", min_tail_idx)));
      }

      if current_head_idx - min_tail_idx < self.shared.capacity {
        telemetry::log_event(Some(current_head_idx), LOC_P_SEND, "BufferHasSpace", Some(format!("H:{}-MT:{} < Cap:{}", current_head_idx, min_tail_idx, self.shared.capacity)));
        break;
      }

      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_BUFFER_FULL, Some(format!("H:{}-MT:{} >= Cap:{}", current_head_idx, min_tail_idx, self.shared.capacity)));
      telemetry::increment_counter(LOC_P_SEND, CTR_P_PARK_ATTEMPTS);
      
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_ARM_PARK, None);
      *self.shared.producer_thread.lock().unwrap() = Some(thread::current());
      self.shared.producer_parked.store(true, Ordering::Release);

      let min_tail_recheck;
      {
        let tails_guard = self.shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_NO_CONSUMERS, Some("during park recheck".into()));
          self.shared.producer_parked.store(false, Ordering::Relaxed); // Unarm
          self.shared.producer_thread.lock().unwrap().take();
          return Err(SendError::Closed);
        }
        min_tail_recheck = tails_guard.list.iter()
            .map(|t_arc| t_arc.load(Ordering::Acquire))
            .min()
            .expect("Consumer list still not empty");
      }
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_SPACE, Some(format!("min_tail_recheck:{}", min_tail_recheck)));

      if current_head_idx - min_tail_recheck < self.shared.capacity {
        telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_PASS, None);
        if self.shared.producer_parked.compare_exchange(
          true, false, Ordering::AcqRel, Ordering::Relaxed
        ).is_ok() {
          telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_CAS_UNARM_SUCCESS, None);
          self.shared.producer_thread.lock().unwrap().take();
        } else {
          telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_CAS_UNARM_FAIL, None);
        }
        current_head_idx = unsafe { *self.shared.head.get() }; // Re-fetch and retry loop
        continue;
      }
      
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_RECHECK_FAIL_PARK, Some(format!("H:{}, MT_recheck:{}", current_head_idx, min_tail_recheck)));
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_EXEC_PARK, None);
      sync_util::park_thread();
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_UNPARKED, None);
      current_head_idx = unsafe { *self.shared.head.get() }; 
    }

    let slot_idx = current_head_idx % self.shared.capacity;
    let slot = &self.shared.buffer[slot_idx];
    
    telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_WRITE_ITEM, Some(format!("slot_idx:{}", slot_idx)));
    unsafe {
      (*slot.value.get()).write(value);
      slot.sequence.store(2 * current_head_idx + 1, Ordering::Release);
      telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_SEQ_STORED, Some(format!("slot_idx:{}, new_seq:{}", slot_idx, 2 * current_head_idx + 1)));
    }

    telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_WAKE_SLOT, Some(format!("slot_idx:{}", slot_idx)));
    telemetry::increment_counter(LOC_P_SEND, CTR_SLOT_WAKES);
    for waker in slot.wakers.lock().unwrap().drain(..) {
      waker.wake();
    }
    
    unsafe { *self.shared.head.get() = current_head_idx + 1; }
    telemetry::log_event(Some(current_head_idx), LOC_P_SEND, EVT_P_ADVANCE_HEAD, Some(format!("new_head:{}", current_head_idx + 1)));
    Ok(())
  }
}

impl<T: Send + Clone> AsyncProducer<T> {
  pub fn send(&mut self, value: T) -> SendFuture<'_, T> {
    SendFuture {
      producer: self,
      value: Some(value),
    }
  }
}

// New Drop impls for Producer and AsyncProducer
impl<T: Send + Clone> Drop for Producer<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    // Wake all consumers that might be parked on any slot
    self.shared.wake_all_consumers_from_slots();
  }
}

impl<T: Send + Clone> Drop for AsyncProducer<T> {
  fn drop(&mut self) {
    self.shared.producer_dropped.store(true, Ordering::Release);
    self.shared.wake_all_consumers_from_slots();
  }
}


// SendFuture (largely unchanged, but benefits from fixed consumer behavior)
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T: Send + Clone> {
  producer: &'a mut AsyncProducer<T>,
  value: Option<T>,
}

impl<'a, T: Send + Clone + Unpin> Future for SendFuture<'a, T> {
  type Output = Result<(), SendError>;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.as_mut().get_mut();
    let shared = &this.producer.shared;
    let mut current_head_idx = unsafe { *shared.head.get() };

    loop {
      telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_ENTER_LOOP, None);
      let min_tail_idx;
      {
        let tails_guard = shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          this.value = None; // Drop the value if can't send
          telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_NO_CONSUMERS, None);
          return Poll::Ready(Err(SendError::Closed));
        }
        min_tail_idx = tails_guard.list.iter()
            .map(|t_arc| t_arc.load(Ordering::Acquire))
            .min()
            .expect("Consumer list checked non-empty");
        telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_GOT_MIN_TAIL, Some(format!("min_tail:{}", min_tail_idx)));
      }

      if current_head_idx - min_tail_idx < shared.capacity {
        telemetry::log_event(Some(current_head_idx), "AsyncP::poll", "BufferHasSpace", None);
        break;
      }

      telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_BUFFER_FULL, Some(format!("H:{}-MT:{} >= Cap:{}", current_head_idx, min_tail_idx, shared.capacity)));
      telemetry::increment_counter("AsyncP::poll", CTR_P_PARK_ATTEMPTS);
      shared.producer_waker.register(cx.waker());
      
      let min_tail_recheck; 
      {
        let tails_guard = shared.tails.lock().unwrap();
        if tails_guard.list.is_empty() {
          this.value = None;
          telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_NO_CONSUMERS, Some("during park recheck".into()));
          return Poll::Ready(Err(SendError::Closed));
        }
        min_tail_recheck = tails_guard.list.iter()
            .map(|t_arc| t_arc.load(Ordering::Acquire))
            .min()
            .expect("Consumer list still not empty");
      }
      telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_RECHECK_SPACE, Some(format!("min_tail_recheck:{}", min_tail_recheck)));
      
      if current_head_idx - min_tail_recheck < shared.capacity {
        telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_RECHECK_PASS, None);
        current_head_idx = unsafe { *shared.head.get() }; 
        continue;
      }
      return Poll::Pending;
    }

    let value_to_write = this.value.take().expect("SendFuture polled after completion");
    let slot_idx = current_head_idx % shared.capacity;
    let slot = &shared.buffer[slot_idx];
    
    telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_WRITE_ITEM, Some(format!("slot_idx:{}", slot_idx)));
    unsafe {
      (*slot.value.get()).write(value_to_write);
      slot.sequence.store(2 * current_head_idx + 1, Ordering::Release);
      telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_SEQ_STORED, Some(format!("slot_idx:{}, new_seq:{}", slot_idx, 2 * current_head_idx + 1)));
    }

    telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_WAKE_SLOT, Some(format!("slot_idx:{}", slot_idx)));
    telemetry::increment_counter("AsyncP::poll", CTR_SLOT_WAKES);
    for waker_in_slot in slot.wakers.lock().unwrap().drain(..) {
      waker_in_slot.wake();
    }
    
    unsafe { *shared.head.get() = current_head_idx + 1; }
    telemetry::log_event(Some(current_head_idx), "AsyncP::poll", EVT_P_ADVANCE_HEAD, Some(format!("new_head:{}", current_head_idx + 1)));
    Poll::Ready(Ok(()))
  }
}


// RecvFuture (largely unchanged, but benefits from fixed try_recv_internal)
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct RecvFuture<'a, T: Send + Clone> {
  receiver: &'a mut AsyncReceiver<T>,
}

impl<'a, T: Send + Clone> Future for RecvFuture<'a, T> {
  type Output = Result<T, RecvError>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let future_self = self.get_mut();
    let consumer_name = format!("C_AsyncFuture(tail:{})", future_self.receiver.tail.load(Ordering::Relaxed));

    match try_recv_internal(&future_self.receiver.shared, &future_self.receiver.tail) {
      Ok(value) => Poll::Ready(Ok(value)),
      Err(TryRecvError::Empty) => {
        let current_tail_val = future_self.receiver.tail.load(Ordering::Relaxed);
        let slot_idx = current_tail_val % future_self.receiver.shared.capacity;
        let slot = &future_self.receiver.shared.buffer[slot_idx];
        
        telemetry::log_event(Some(current_tail_val), &consumer_name, EVT_C_REG_WAKER, Some(format!("slot_idx:{}", slot_idx)));
        slot.wakers.lock().unwrap().push(cx.waker().clone());
        
        // Re-check after registering waker
        match try_recv_internal(&future_self.receiver.shared, &future_self.receiver.tail) {
          Ok(value) => {
            telemetry::log_event(Some(current_tail_val), &consumer_name, EVT_C_GOT_ON_RECHECK, None);
            Poll::Ready(Ok(value))
          },
          Err(TryRecvError::Empty) => Poll::Pending,
          Err(TryRecvError::Disconnected) => Poll::Ready(Err(RecvError::Disconnected)),
        }
      }
      Err(TryRecvError::Disconnected) => {
        Poll::Ready(Err(RecvError::Disconnected))
      }
    }
  }
}

// --- UNSAFE TRAIT IMPLEMENTATIONS ---
// These are justified by the internal synchronization mechanisms (atomics, mutexes,
// and the SPMC ring buffer protocol itself for UnsafeCell access).

// SpmcShared<T> is Send + Sync if T is Send + Clone because:
// - `buffer`: `Box<[Slot<T>]>` is Send + Sync if `Slot<T>` is Send + Sync.
// - `capacity`: `usize` is Send + Sync.
// - `head`: `CachePadded<UnsafeCell<usize>>`. Access to the UnsafeCell's data
//   is only done by the single producer, or is read indirectly. No concurrent
//   raw pointer access by multiple threads.
// - `tails`: `Mutex<ConsumerCursors>` is Send + Sync.
// - `producer_parked`: `AtomicBool` is Send + Sync.
// - `producer_thread`: `Mutex<Option<Thread>>` is Send + Sync.
// - `producer_waker`: `AtomicWaker` is Send + Sync.
unsafe impl<T: Send + Clone> Send for SpmcShared<T> {}
unsafe impl<T: Send + Clone> Sync for SpmcShared<T> {}

// Slot<T> is Send if T is Send.
// Slot<T> is Sync if T is Send because access to `value: UnsafeCell<MaybeUninit<T>>`
// is strictly synchronized by the `sequence: AtomicUsize`. A thread will only
// access `value.get()` for a specific "turn" if the sequence number grants it
// exclusive access for that operation (write for producer, read for consumer).
unsafe impl<T: Send> Send for Slot<T> {}
unsafe impl<T: Send> Sync for Slot<T> {} // Note: T only needs to be Send here, Sync for T is not required
                                          // for Slot<T> to be Sync due to external sync via `sequence`.
                                          // If T were !Send, Slot<T> could not be Send or Sync.