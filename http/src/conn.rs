//! A connection: one engine, one transport and the turn that moves bytes between them.
//!
//! A caller waiting on an exchange takes the lead and reads the socket for every exchange on the connection. Other callers park until the leader hands them an event or leaves. The lock is never held while waiting on the socket.

use std::collections::{HashMap, VecDeque};
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Poll, Waker};
use std::thread::{self, Thread};
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use fibre::sync::HybridMutex;

use crate::Error;
use crate::engine::{Capacity, ConnEvent, Engine, Event, ExchangeId, NetAction, OpenError, Output, RequestHead};
use crate::io::Io;
use crate::wait::{self, Interest, Registration};

const TURN_READ: usize = 64 * 1024;
const READ_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Proto {
  H1,
  H2,
}

#[derive(Default)]
enum Waiter {
  #[default]
  None,
  Thread(Thread),
  Task(Waker),
}

impl Waiter {
  fn wake(&mut self) {
    match std::mem::take(self) {
      Waiter::Thread(t) => t.unpark(),
      Waiter::Task(w) => w.wake(),
      Waiter::None => {}
    }
  }

  fn is_some(&self) -> bool {
    !matches!(self, Waiter::None)
  }
}

#[derive(Default)]
struct Slot {
  events: VecDeque<Event>,
  writable: bool,
  waiter: Waiter,
}

pub(crate) struct State {
  engine: Box<dyn Engine + Send>,
  io: Io,
  input: BytesMut,
  pending: BytesMut,
  unflushed: bool,
  out: Output,
  slots: HashMap<ExchangeId, Slot>,
  leader: bool,
  closed: bool,
  going_away: bool,
  error: Option<Error>,
}

impl State {
  fn apply(&mut self) {
    let out = std::mem::take(&mut self.out);
    for action in out.net {
      match action {
        NetAction::Send(b) => self.pending.extend_from_slice(&b),
        NetAction::Close => self.closed = true,
        _ => {}
      }
    }
    for (id, event) in out.events {
      if let Some(slot) = self.slots.get_mut(&id) {
        match event {
          Event::Writable => slot.writable = true,
          event => slot.events.push_back(event),
        }
        slot.waiter.wake();
      }
    }
    for event in out.conn {
      match event {
        ConnEvent::GoingAway => self.going_away = true,
        ConnEvent::Closed(e) => {
          self.closed = true;
          if self.error.is_none() {
            self.error = e;
          }
        }
      }
    }
    if self.closed {
      for slot in self.slots.values_mut() {
        slot.waiter.wake();
      }
    }
  }

  fn broken(&mut self, e: std::io::Error) {
    if self.error.is_none() {
      self.error = Some(Error::from(e));
    }
    self.engine.on_eof(&mut self.out);
    self.apply();
    self.closed = true;
    self.pending.clear();
    for slot in self.slots.values_mut() {
      slot.waiter.wake();
    }
  }

  fn flush_now(&mut self) {
    if self.closed {
      self.pending.clear();
      self.unflushed = false;
      return;
    }
    match self.io.flush(&mut self.pending) {
      Ok(done) => self.unflushed = !done,
      Err(e) => self.broken(e),
    }
  }

  fn turn(&mut self) -> Interest {
    self.flush_now();
    let mut buf = [0u8; READ_CHUNK];
    let mut taken = 0;
    let mut got = false;
    while taken < TURN_READ && !self.closed {
      match self.io.read(&mut buf) {
        Ok(0) => {
          if got {
            self.engine.on_bytes(&mut self.input, &mut self.out);
            got = false;
          }
          self.engine.on_eof(&mut self.out);
          self.apply();
          self.closed = true;
          break;
        }
        Ok(n) => {
          self.input.extend_from_slice(&buf[..n]);
          taken += n;
          got = true;
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
        Err(e) => {
          self.broken(e);
          break;
        }
      }
    }
    if got {
      self.engine.on_bytes(&mut self.input, &mut self.out);
      self.apply();
    }
    self.flush_now();
    Interest { read: true, write: self.unflushed }
  }

  fn closed_error(&self) -> Error {
    self.error.clone().unwrap_or(Error::Closed)
  }

  /// The next event for `id`. A finished or broken exchange gives a `Reset`.
  fn take(&mut self, id: ExchangeId) -> Option<Event> {
    let Some(slot) = self.slots.get_mut(&id) else { return Some(Event::Reset(self.closed_error())) };
    if let Some(event) = slot.events.pop_front() {
      match &event {
        Event::Data(b) => {
          let n = b.len();
          self.engine.consumed(id, n, &mut self.out);
          self.apply();
          self.flush_now();
        }
        Event::End | Event::Reset(_) => {
          self.slots.remove(&id);
        }
        _ => {}
      }
      return Some(event);
    }
    if self.closed {
      self.slots.remove(&id);
      return Some(Event::Reset(self.closed_error()));
    }
    None
  }

  fn writable_or_done(&mut self, id: ExchangeId) -> Option<()> {
    match self.slots.get_mut(&id) {
      Some(slot) if slot.writable => {
        slot.writable = false;
        Some(())
      }
      Some(slot) if !slot.events.is_empty() => Some(()),
      Some(_) if !self.closed => None,
      _ => Some(()),
    }
  }

  fn flushed(&mut self, _id: ExchangeId) -> Option<()> {
    self.flush_now();
    (!self.unflushed || self.closed).then_some(())
  }

  fn wake_any(&mut self) {
    if let Some(slot) = self.slots.values_mut().find(|s| s.waiter.is_some()) {
      slot.waiter.wake();
    }
  }

  fn needs_wake(&self, id: ExchangeId) -> bool {
    !self.leader || self.closed || self.slots.get(&id).is_none_or(|s| !s.events.is_empty() || s.writable)
  }
}

pub(crate) struct Conn {
  reg: Registration,
  st: HybridMutex<State>,
  fd: RawFd,
  pub(crate) proto: Proto,
  closed: AtomicBool,
  /// Exchanges open and the most allowed, as of the last operation. `has_room` reads these without the lock.
  open: AtomicUsize,
  max: AtomicUsize,
  idle_since: parking_lot::Mutex<Instant>,
}

type Check<R> = fn(&mut State, ExchangeId) -> Option<R>;

impl Conn {
  pub(crate) fn new(engine: Box<dyn Engine + Send>, io: Io, proto: Proto, reg: Registration) -> Arc<Conn> {
    let fd = io.fd();
    Arc::new(Conn {
      reg,
      st: HybridMutex::new(State {
        engine,
        io,
        input: BytesMut::new(),
        pending: BytesMut::new(),
        unflushed: false,
        out: Output::default(),
        slots: HashMap::new(),
        leader: false,
        closed: false,
        going_away: false,
        error: None,
      }),
      fd,
      proto,
      closed: AtomicBool::new(false),
      open: AtomicUsize::new(0),
      max: AtomicUsize::new(1),
      idle_since: parking_lot::Mutex::new(Instant::now()),
    })
  }

  pub(crate) fn is_closed(&self) -> bool {
    self.closed.load(Ordering::Acquire)
  }

  fn note(&self, st: &State) {
    if st.closed {
      self.closed.store(true, Ordering::Release);
    }
    let (open, max) = match st.engine.capacity() {
      Capacity::Shared { open, max } => (open, max),
      Capacity::Unique { available } => (usize::from(!available), 1),
    };
    let max = if st.closed || st.going_away { 0 } else { max };
    self.open.store(open, Ordering::Release);
    self.max.store(max, Ordering::Release);
  }

  /// Whether an HTTP/2 connection can take another exchange now. A stale answer only costs a retry, since `open` checks again.
  pub(crate) fn has_room(&self) -> bool {
    !self.is_closed() && self.open.load(Ordering::Acquire) < self.max.load(Ordering::Acquire)
  }

  pub(crate) fn mark_idle(&self) {
    *self.idle_since.lock() = Instant::now();
  }

  /// Whether an idle HTTP/1 connection can be handed out. It is peeked only when idle at least `peek_after`.
  pub(crate) fn usable(&self, peek_after: std::time::Duration, idle_timeout: std::time::Duration) -> bool {
    if self.is_closed() {
      return false;
    }
    let idle = self.idle_since.lock().elapsed();
    if idle >= idle_timeout {
      return false;
    }
    if idle >= peek_after {
      let Some(st) = self.st.try_lock() else { return false };
      return !st.closed && !st.io.idle_unusable();
    }
    true
  }

  fn open_locked(&self, st: &mut State, head: RequestHead) -> Result<ExchangeId, OpenError> {
    if st.closed {
      return Err(OpenError::Closed);
    }
    let result = st.engine.open(head, &mut st.out);
    if let Ok(id) = result {
      st.slots.insert(id, Slot::default());
    }
    st.apply();
    st.flush_now();
    self.note(st);
    result
  }

  pub(crate) fn open_sync(&self, head: RequestHead) -> Result<ExchangeId, OpenError> {
    let mut st = self.st.lock();
    self.open_locked(&mut st, head)
  }

  pub(crate) async fn open_async(&self, head: RequestHead) -> Result<ExchangeId, OpenError> {
    let mut st = self.st.lock_async().await;
    self.open_locked(&mut st, head)
  }

  fn offer(&self, st: &mut State, id: ExchangeId, data: &Bytes, off: &mut usize) -> Result<(), Error> {
    if st.closed {
      return Err(st.closed_error());
    }
    let n = st.engine.send_body(id, data.slice(*off..), true, &mut st.out);
    *off += n;
    st.apply();
    st.flush_now();
    self.note(st);
    Ok(())
  }

  pub(crate) fn send_body_sync(&self, id: ExchangeId, data: Bytes, deadline: Option<Instant>) -> Result<(), Error> {
    let mut off = 0;
    let mut first = true;
    loop {
      let unflushed = {
        let mut st = self.st.lock();
        if first || off < data.len() {
          self.offer(&mut st, id, &data, &mut off)?;
          first = false;
        }
        st.unflushed
      };
      if off == data.len() && !unflushed {
        return Ok(());
      }
      if off < data.len() {
        self.drive_sync(id, deadline, State::writable_or_done)?;
      } else {
        self.drive_sync(id, deadline, State::flushed)?;
      }
    }
  }

  pub(crate) async fn send_body_async(&self, id: ExchangeId, data: Bytes, deadline: Option<Instant>) -> Result<(), Error> {
    let mut off = 0;
    let mut first = true;
    loop {
      let unflushed = {
        let mut st = self.st.lock_async().await;
        if first || off < data.len() {
          self.offer(&mut st, id, &data, &mut off)?;
          first = false;
        }
        st.unflushed
      };
      if off == data.len() && !unflushed {
        return Ok(());
      }
      if off < data.len() {
        self.drive_async(id, deadline, State::writable_or_done).await?;
      } else {
        self.drive_async(id, deadline, State::flushed).await?;
      }
    }
  }

  pub(crate) fn next_event_sync(&self, id: ExchangeId, deadline: Option<Instant>) -> Result<Event, Error> {
    self.drive_sync(id, deadline, State::take)
  }

  pub(crate) async fn next_event_async(&self, id: ExchangeId, deadline: Option<Instant>) -> Result<Event, Error> {
    self.drive_async(id, deadline, State::take).await
  }

  /// The next event for `id` if one is already queued. Never waits.
  pub(crate) fn try_event(&self, id: ExchangeId) -> Option<Event> {
    let mut st = self.st.lock();
    if !st.slots.get(&id).is_some_and(|s| !s.events.is_empty()) {
      return None;
    }
    let event = st.take(id);
    self.note(&st);
    event
  }

  pub(crate) fn reset(&self, id: ExchangeId) {
    let mut st = self.st.lock();
    let st = &mut *st;
    if st.slots.remove(&id).is_some() {
      st.engine.reset(id, &mut st.out);
      st.apply();
      st.flush_now();
    }
    self.note(&st);
  }

  fn drive_sync<R>(&self, id: ExchangeId, deadline: Option<Instant>, check: Check<R>) -> Result<R, Error> {
    loop {
      let mut st = self.st.lock();
      if let Some(r) = check(&mut st, id) {
        return Ok(r);
      }
      if !st.leader {
        st.leader = true;
        loop {
          let interest = st.turn();
          self.note(&st);
          if let Some(r) = check(&mut st, id) {
            st.leader = false;
            st.wake_any();
            return Ok(r);
          }
          drop(st);
          let ready = wait::block(self.fd, interest, deadline);
          st = self.st.lock();
          match ready {
            Ok(true) => {}
            Ok(false) => {
              st.leader = false;
              st.wake_any();
              return Err(Error::Timeout);
            }
            Err(e) => {
              st.leader = false;
              st.broken(e);
              self.note(&st);
              return Err(st.closed_error());
            }
          }
        }
      }
      if let Some(slot) = st.slots.get_mut(&id) {
        slot.waiter = Waiter::Thread(thread::current());
      }
      drop(st);
      match deadline {
        Some(d) => {
          let now = Instant::now();
          if now >= d {
            return Err(Error::Timeout);
          }
          thread::park_timeout(d - now);
        }
        None => thread::park(),
      }
    }
  }

  async fn drive_async<R>(&self, id: ExchangeId, deadline: Option<Instant>, check: Check<R>) -> Result<R, Error> {
    loop {
      let mut st = self.st.lock_async().await;
      if let Some(r) = check(&mut st, id) {
        return Ok(r);
      }
      if !st.leader {
        st.leader = true;
        let mut lead = Lead { conn: self, active: true };
        loop {
          let interest = st.turn();
          self.note(&st);
          if let Some(r) = check(&mut st, id) {
            st.leader = false;
            st.wake_any();
            lead.active = false;
            return Ok(r);
          }
          drop(st);
          let ready = self.reg.wait(self.fd, interest, deadline).await;
          st = self.st.lock_async().await;
          match ready {
            Ok(true) => {}
            Ok(false) => {
              st.leader = false;
              st.wake_any();
              lead.active = false;
              return Err(Error::Timeout);
            }
            Err(e) => {
              st.leader = false;
              st.broken(e);
              self.note(&st);
              lead.active = false;
              return Err(st.closed_error());
            }
          }
        }
      }
      drop(st);
      let parked = std::future::poll_fn(|cx| {
        let Some(mut st) = self.st.try_lock() else {
          cx.waker().wake_by_ref();
          return Poll::Pending;
        };
        if st.needs_wake(id) {
          return Poll::Ready(());
        }
        if let Some(slot) = st.slots.get_mut(&id) {
          slot.waiter = Waiter::Task(cx.waker().clone());
        }
        Poll::Pending
      });
      match deadline {
        Some(d) => {
          if tokio::time::timeout_at(d.into(), parked).await.is_err() {
            return Err(Error::Timeout);
          }
        }
        None => parked.await,
      }
    }
  }
}

/// Gives up the lead if an async leader is dropped while waiting on the socket.
struct Lead<'a> {
  conn: &'a Conn,
  active: bool,
}

impl Drop for Lead<'_> {
  fn drop(&mut self) {
    if self.active {
      let mut st = self.conn.st.lock();
      st.leader = false;
      st.wake_any();
    }
  }
}
