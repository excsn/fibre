//! A connection: one engine, one transport and the turn that moves bytes between them.
//!
//! A caller waiting on an exchange takes the lead and reads the socket for every exchange on the connection. Other callers park until the leader hands them an event or leaves. The lock is never held while waiting on the socket.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use fibre::sync::HybridMutex;

use crate::Error;
use crate::engine::{Capacity, ConnEvent, Engine, Event, ExchangeId, NetAction, OpenError, Output, RequestHead};
use crate::io::Io;
use crate::notify::{Signal, Waiter};
use crate::sync::{AtomicBool, AtomicUsize, Ordering, thread};
use crate::wait::{self, Interest, Registration, Waitable};

const TURN_READ: usize = 64 * 1024;
const READ_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Proto {
  H1,
  H2,
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
  /// Socket reads land here first, allocated once per connection.
  read_buf: Box<[u8]>,
  pending: BytesMut,
  /// Bytes ready for the socket, written by whoever holds `writing`, with the lock released.
  wire: BytesMut,
  writing: bool,
  /// Body bytes were consumed since the engine last ran.
  consumed: bool,
  out: Output,
  slots: HashMap<ExchangeId, Slot>,
  leader: bool,
  /// When the last leader left with others still waiting, for `diagnostics`.
  #[cfg(feature = "diagnostics")]
  released_at: Option<Instant>,
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

  /// Seals pending output into `wire`. The socket write happens in `Conn::release`, outside the lock.
  fn flush_now(&mut self) {
    if self.closed {
      self.pending.clear();
      self.wire.clear();
      return;
    }
    if let Err(e) = self.io.seal(&mut self.pending, &mut self.wire) {
      self.broken(e);
    }
  }

  fn turn(&mut self) -> Interest {
    crate::diag::bump(crate::diag::C::Turn);
    #[cfg(feature = "diagnostics")]
    if let Some(t) = self.released_at.take() {
      crate::diag::idle(t.elapsed().as_nanos() as u64);
    }
    self.flush_now();
    let mut taken = 0;
    let mut got = false;
    while taken < TURN_READ && !self.closed {
      match self.io.read(&mut self.read_buf) {
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
          self.input.extend_from_slice(&self.read_buf[..n]);
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
    if !got {
      crate::diag::bump(crate::diag::C::EmptyTurn);
    }
    // After `consumed` the engine runs even with nothing read, so output it deferred, such as an HTTP/2 window update, goes out.
    if got || self.consumed {
      self.consumed = false;
      self.engine.on_bytes(&mut self.input, &mut self.out);
      self.apply();
    }
    self.flush_now();
    Interest { read: true, write: !self.wire.is_empty() && !self.writing }
  }

  /// The earlier of the request's deadline and the engine's. Returns whether the engine's is the one waited for.
  fn wait_until(&self, request: Option<Instant>) -> (Option<Instant>, bool) {
    match (request, self.engine.next_deadline()) {
      (Some(r), Some(e)) if e < r => (Some(e), true),
      (None, Some(e)) => (Some(e), true),
      (r, _) => (r, false),
    }
  }

  /// Runs the engine's timers that are due.
  fn tick(&mut self) {
    self.engine.on_tick(Instant::now(), &mut self.out);
    self.apply();
    self.flush_now();
  }

  fn closed_error(&self) -> Error {
    self.error.clone().unwrap_or(Error::Closed)
  }

  /// The next event for `id`. A finished or broken exchange gives a `Reset`.
  fn take(&mut self, id: ExchangeId) -> Option<Event> {
    let Some(slot) = self.slots.get_mut(&id) else { return Some(Event::Reset(self.closed_error())) };
    if let Some(event) = slot.events.pop_front() {
      crate::diag::bump(crate::diag::C::Taken);
      match &event {
        Event::Data(b) => {
          let n = b.len();
          self.engine.consumed(id, n, &mut self.out);
          self.consumed = true;
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

  /// Every event queued for `id`, at least one, taken under one lock. Stops after `End` or `Reset`.
  fn take_all(&mut self, id: ExchangeId) -> Option<VecDeque<Event>> {
    let first = self.take(id)?;
    let mut all = VecDeque::from([first]);
    while !matches!(all.back(), Some(Event::End | Event::Reset(_))) && self.slots.get(&id).is_some_and(|s| !s.events.is_empty()) {
      match self.take(id) {
        Some(event) => all.push_back(event),
        None => break,
      }
    }
    Some(all)
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
    (self.wire.is_empty() || self.writing || self.closed).then_some(())
  }

  fn wake_any(&mut self) {
    if let Some(slot) = self.slots.values_mut().find(|s| s.waiter.is_some()) {
      slot.waiter.wake();
      crate::diag::bump(crate::diag::C::Handoff);
      #[cfg(feature = "diagnostics")]
      {
        self.released_at = Some(Instant::now());
      }
    }
  }

}

pub(crate) struct Conn {
  reg: Registration,
  st: HybridMutex<State>,
  writer: crate::io::Writer,
  waitable: Waitable,
  pub(crate) proto: Proto,
  closed: AtomicBool,
  /// Exchanges open and the most allowed, as of the last operation. `has_room` reads these without the lock.
  open: AtomicUsize,
  max: AtomicUsize,
  idle_since: crate::sync::Mutex<Instant>,
}

type Check<R> = fn(&mut State, ExchangeId) -> Option<R>;
type Guard<'a> = fibre::sync::MutexGuard<'a, State>;

impl Conn {
  pub(crate) fn new(engine: Box<dyn Engine + Send>, io: Io, proto: Proto, reg: Registration) -> Arc<Conn> {
    let waitable = io.waitable();
    let writer = io.writer().expect("a connected socket can be cloned for writing");
    Arc::new(Conn {
      reg,
      writer,
      st: HybridMutex::new(State {
        engine,
        io,
        input: BytesMut::new(),
        read_buf: vec![0u8; READ_CHUNK].into_boxed_slice(),
        pending: BytesMut::new(),
        wire: BytesMut::new(),
        writing: false,
        consumed: false,
        out: Output::default(),
        slots: HashMap::new(),
        leader: false,
        #[cfg(feature = "diagnostics")]
        released_at: None,
        closed: false,
        going_away: false,
        error: None,
      }),
      waitable,
      proto,
      closed: AtomicBool::new(false),
      open: AtomicUsize::new(0),
      max: AtomicUsize::new(1),
      idle_since: crate::sync::Mutex::new(Instant::now()),
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

  /// Drops the lock, first writing out sealed bytes if nobody else is: write combining. Bytes sealed by others while this writes go out in the next round. A full socket leaves the rest in `wire` for the leader, which then waits for writable.
  fn release<'a>(&'a self, mut st: Guard<'a>) -> bool {
    loop {
      if st.writing || st.wire.is_empty() || st.closed {
        return !st.wire.is_empty() && !st.writing && !st.closed;
      }
      st.writing = true;
      let buf = std::mem::take(&mut st.wire);
      drop(st);
      let written = self.write_some(&buf);
      st = self.st.lock();
      if !self.settle(&mut st, buf, written) {
        return !st.wire.is_empty() && !st.closed;
      }
    }
  }

  async fn release_async<'a>(&'a self, mut st: Guard<'a>) -> bool {
    loop {
      if st.writing || st.wire.is_empty() || st.closed {
        return !st.wire.is_empty() && !st.writing && !st.closed;
      }
      st.writing = true;
      let buf = std::mem::take(&mut st.wire);
      drop(st);
      let written = self.write_some(&buf);
      st = self.st.lock_async().await;
      if !self.settle(&mut st, buf, written) {
        return !st.wire.is_empty() && !st.closed;
      }
    }
  }

  /// Writes until the socket would block. Returns how much went out.
  fn write_some(&self, buf: &[u8]) -> std::io::Result<usize> {
    let mut off = 0;
    while off < buf.len() {
      match self.writer.write(&buf[off..]) {
        Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
        Ok(n) => off += n,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
        Err(e) => return Err(e),
      }
    }
    Ok(off)
  }

  /// Puts back what the socket did not take, ahead of anything sealed meanwhile. Returns whether to write another round.
  fn settle(&self, st: &mut State, mut buf: BytesMut, written: std::io::Result<usize>) -> bool {
    st.writing = false;
    match written {
      Err(e) => {
        st.broken(e);
        self.note(st);
        false
      }
      Ok(n) if n < buf.len() => {
        let _ = buf.split_to(n);
        buf.extend_from_slice(&st.wire);
        st.wire = buf;
        if !st.leader {
          st.wake_any();
        }
        false
      }
      Ok(_) => true,
    }
  }

  fn open_locked(&self, st: &mut State, head: RequestHead) -> Result<ExchangeId, OpenError> {
    if st.closed {
      return Err(OpenError::Closed);
    }
    let result = st.engine.open(head, Instant::now(), &mut st.out);
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
    let r = self.open_locked(&mut st, head);
    self.release(st);
    r
  }

  pub(crate) async fn open_async(&self, head: RequestHead) -> Result<ExchangeId, OpenError> {
    let mut st = self.st.lock_async().await;
    let r = self.open_locked(&mut st, head);
    self.release_async(st).await;
    r
  }

  fn offer(&self, st: &mut State, id: ExchangeId, data: &Bytes, end: bool, off: &mut usize) -> Result<(), Error> {
    if st.closed {
      return Err(st.closed_error());
    }
    let n = st.engine.send_body(id, data.slice(*off..), end, &mut st.out);
    *off += n;
    st.apply();
    st.flush_now();
    self.note(st);
    Ok(())
  }

  /// Sends `data` as the next piece of the request body; `end` marks the last piece.
  pub(crate) fn send_body_sync(&self, id: ExchangeId, data: Bytes, end: bool, deadline: Option<Instant>) -> Result<(), Error> {
    let mut off = 0;
    let mut first = true;
    loop {
      let unflushed = {
        let mut st = self.st.lock();
        if first || off < data.len() {
          self.offer(&mut st, id, &data, end, &mut off)?;
          first = false;
        }
        self.release(st);
        let st = self.st.lock();
        !(st.wire.is_empty() || st.writing || st.closed)
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

  pub(crate) async fn send_body_async(&self, id: ExchangeId, data: Bytes, end: bool, deadline: Option<Instant>) -> Result<(), Error> {
    let mut off = 0;
    let mut first = true;
    loop {
      let unflushed = {
        let mut st = self.st.lock_async().await;
        if first || off < data.len() {
          self.offer(&mut st, id, &data, end, &mut off)?;
          first = false;
        }
        self.release_async(st).await;
        let st = self.st.lock_async().await;
        !(st.wire.is_empty() || st.writing || st.closed)
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

  /// Every event already queued for `id`, waiting for at least one.
  pub(crate) fn next_events_sync(&self, id: ExchangeId, deadline: Option<Instant>) -> Result<VecDeque<Event>, Error> {
    self.drive_sync(id, deadline, State::take_all)
  }

  pub(crate) async fn next_events_async(&self, id: ExchangeId, deadline: Option<Instant>) -> Result<VecDeque<Event>, Error> {
    self.drive_async(id, deadline, State::take_all).await
  }

  /// The next event for `id` if one is already queued. Never waits.
  pub(crate) fn try_event(&self, id: ExchangeId) -> Option<Event> {
    let mut st = self.st.lock();
    if !st.slots.get(&id).is_some_and(|s| !s.events.is_empty()) {
      return None;
    }
    let event = st.take(id);
    self.note(&st);
    self.release(st);
    event
  }

  pub(crate) fn reset(&self, id: ExchangeId) {
    let mut guard = self.st.lock();
    let st = &mut *guard;
    if st.slots.remove(&id).is_some() {
      st.engine.reset(id, &mut st.out);
      st.apply();
      st.flush_now();
    }
    self.note(&guard);
    self.release(guard);
  }

  fn drive_sync<R>(&self, id: ExchangeId, deadline: Option<Instant>, check: Check<R>) -> Result<R, Error> {
    loop {
      let mut st = self.st.lock();
      if let Some(r) = check(&mut st, id) {
        self.release(st);
        return Ok(r);
      }
      if !st.leader {
        st.leader = true;
        crate::diag::bump(crate::diag::C::Lead);
        loop {
          let interest = st.turn();
          self.note(&st);
          if let Some(r) = check(&mut st, id) {
            st.leader = false;
            st.wake_any();
            self.release(st);
            return Ok(r);
          }
          let (until, engine_timer) = st.wait_until(deadline);
          let left = self.release(st);
          let interest = Interest { read: true, write: interest.write || left };
          let ready = wait::block_on(&self.waitable, interest, until);
          st = self.st.lock();
          match ready {
            Ok(true) => {}
            Ok(false) if engine_timer => st.tick(),
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
      self.release(st);
      match deadline {
        Some(d) => {
          let now = Instant::now();
          if now >= d {
            return Err(Error::Timeout);
          }
          crate::diag::bump(crate::diag::C::Park);
          thread::park_timeout(d - now);
        }
        None => {
          crate::diag::bump(crate::diag::C::Park);
          #[cfg(feature = "diagnostics")]
          let parked = Instant::now();
          thread::park();
          #[cfg(feature = "diagnostics")]
          {
            crate::diag::parked(parked.elapsed().as_nanos() as u64);
            let woke = Instant::now();
            drop(self.st.lock());
            crate::diag::relock(woke.elapsed().as_nanos() as u64);
          }
        }
      }
    }
  }

  async fn drive_async<R>(&self, id: ExchangeId, deadline: Option<Instant>, check: Check<R>) -> Result<R, Error> {
    loop {
      let mut st = self.st.lock_async().await;
      if let Some(r) = check(&mut st, id) {
        self.release_async(st).await;
        return Ok(r);
      }
      if !st.leader {
        st.leader = true;
        crate::diag::bump(crate::diag::C::Lead);
        let mut lead = Lead { conn: self, active: true };
        loop {
          let interest = st.turn();
          self.note(&st);
          if let Some(r) = check(&mut st, id) {
            st.leader = false;
            st.wake_any();
            lead.active = false;
            self.release_async(st).await;
            return Ok(r);
          }
          let (until, engine_timer) = st.wait_until(deadline);
          let left = self.release_async(st).await;
          let interest = Interest { read: true, write: interest.write || left };
          let ready = self.reg.wait_on(&self.waitable, interest, until).await;
          st = self.st.lock_async().await;
          match ready {
            Ok(true) => {}
            Ok(false) if engine_timer => st.tick(),
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
      let signal = Arc::new(Signal::default());
      if let Some(slot) = st.slots.get_mut(&id) {
        slot.waiter = Waiter::Signal(signal.clone());
      }
      self.release_async(st).await;
      let parked = signal.wait();
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

/// Shuttle models of the leader handover: many callers on one shared connection, over an in-memory pipe, with a mock engine and a model server. A lost wake shows as a shuttle deadlock report with its schedule. Run with `http/verify/shuttle.sh`.
#[cfg(all(test, shuttle))]
mod model {
  use std::collections::HashSet;
  use std::future::Future;
  use std::sync::Arc;
  use std::task::{Context, Waker};

  use bytes::{Buf, Bytes, BytesMut};
  use http::{HeaderMap, Method, StatusCode, Version};

  use super::*;
  use crate::engine::{Capacity, ConnEvent, Event, NetAction, OpenError, Output, RequestHead, ResponseHead};
  use crate::io::model::Pipe;

  const ITERATIONS: usize = 2000;

  /// Requests go out as `Q<id>;`. Each `A<id>;` from the server is that exchange's whole response.
  struct Mock {
    open: HashSet<u64>,
    next: u64,
  }

  impl Engine for Mock {
    fn capacity(&self) -> Capacity {
      Capacity::Shared { open: self.open.len(), max: 16 }
    }

    fn open(&mut self, _head: RequestHead, _now: Instant, out: &mut Output) -> Result<ExchangeId, OpenError> {
      self.next += 1;
      self.open.insert(self.next);
      out.net.push(NetAction::Send(Bytes::from(format!("Q{};", self.next))));
      Ok(ExchangeId(self.next))
    }

    fn send_body(&mut self, _id: ExchangeId, data: Bytes, _end: bool, _out: &mut Output) -> usize {
      data.len()
    }

    fn consumed(&mut self, _id: ExchangeId, _n: usize, _out: &mut Output) {}

    fn reset(&mut self, id: ExchangeId, _out: &mut Output) {
      self.open.remove(&id.0);
    }

    fn on_bytes(&mut self, input: &mut BytesMut, out: &mut Output) {
      while let Some(end) = input.iter().position(|b| *b == b';') {
        let token = String::from_utf8_lossy(&input[..end]).into_owned();
        input.advance(end + 1);
        let id: u64 = token.trim_start_matches('A').parse().unwrap();
        if self.open.remove(&id) {
          let head = ResponseHead { status: StatusCode::OK, version: Version::HTTP_2, headers: HeaderMap::new() };
          out.events.push((ExchangeId(id), Event::Head(head)));
          out.events.push((ExchangeId(id), Event::Data(Bytes::from_static(b"ok"))));
          out.events.push((ExchangeId(id), Event::End));
        }
      }
    }

    fn on_eof(&mut self, out: &mut Output) {
      for id in self.open.drain() {
        out.events.push((ExchangeId(id), Event::Reset(Error::Closed)));
      }
      out.conn.push(ConnEvent::Closed(None));
    }

    fn on_tick(&mut self, _now: Instant, _out: &mut Output) {}

    fn next_deadline(&self) -> Option<Instant> {
      None
    }

    fn close(&mut self, out: &mut Output) {
      self.on_eof(out);
    }
  }

  fn connection() -> (Arc<Conn>, Arc<Pipe>) {
    let pipe = Arc::new(Pipe::default());
    let conn = Conn::new(Box::new(Mock { open: HashSet::new(), next: 0 }), Io::Model(pipe.clone()), Proto::H2, Registration::default());
    (conn, pipe)
  }

  /// Answers `answers` requests one at a time, in the order they arrive.
  fn server(pipe: Arc<Pipe>, answers: usize) -> thread::JoinHandle<()> {
    thread::spawn(move || {
      let mut pending = Vec::new();
      let mut answered = 0;
      while answered < answers {
        let bytes = pipe.take_requests();
        if bytes.is_empty() {
          return;
        }
        pending.extend_from_slice(&bytes);
        while let Some(end) = pending.iter().position(|b| *b == b';') {
          let token: Vec<u8> = pending.drain(..=end).collect();
          let id = String::from_utf8_lossy(&token[1..token.len() - 1]).into_owned();
          pipe.respond(format!("A{id};").as_bytes());
          answered += 1;
        }
      }
    })
  }

  fn head() -> RequestHead {
    RequestHead { method: Method::GET, uri: "http://model.test/".parse().unwrap(), version: Version::HTTP_2, headers: HeaderMap::new() }
  }

  fn kind(event: &Event) -> &'static str {
    match event {
      Event::Head(_) => "head",
      Event::Data(_) => "data",
      Event::End => "end",
      Event::Reset(_) => "reset",
      _ => "other",
    }
  }

  fn exchange_sync(conn: &Conn) -> Vec<&'static str> {
    let id = conn.open_sync(head()).unwrap();
    let mut seen = Vec::new();
    loop {
      for event in conn.next_events_sync(id, None).unwrap() {
        seen.push(kind(&event));
        if matches!(event, Event::End | Event::Reset(_)) {
          return seen;
        }
      }
    }
  }

  async fn exchange_async(conn: &Conn) -> Vec<&'static str> {
    let id = conn.open_async(head()).await.unwrap();
    let mut seen = Vec::new();
    loop {
      for event in conn.next_events_async(id, None).await.unwrap() {
        seen.push(kind(&event));
        if matches!(event, Event::End | Event::Reset(_)) {
          return seen;
        }
      }
    }
  }

  fn blocking_callers() {
    let (conn, pipe) = connection();
    let srv = server(pipe, 3);
    let callers: Vec<_> = (0..3)
      .map(|_| {
        let conn = conn.clone();
        thread::spawn(move || exchange_sync(&conn))
      })
      .collect();
    for c in callers {
      assert_eq!(c.join().unwrap(), ["head", "data", "end"]);
    }
    srv.join().unwrap();
  }

  #[test]
  fn blocking_callers_each_get_their_own_response_random() {
    shuttle::check_random(blocking_callers, ITERATIONS);
  }

  #[test]
  fn blocking_callers_each_get_their_own_response_pct() {
    shuttle::check_pct(blocking_callers, ITERATIONS, 3);
  }

  fn mixed_callers() {
    let (conn, pipe) = connection();
    let srv = server(pipe, 3);
    let blocking: Vec<_> = (0..2)
      .map(|_| {
        let conn = conn.clone();
        thread::spawn(move || exchange_sync(&conn))
      })
      .collect();
    let task = {
      let conn = conn.clone();
      thread::spawn(move || shuttle::future::block_on(async move { exchange_async(&conn).await }))
    };
    for c in blocking {
      assert_eq!(c.join().unwrap(), ["head", "data", "end"]);
    }
    assert_eq!(task.join().unwrap(), ["head", "data", "end"]);
    srv.join().unwrap();
  }

  #[test]
  fn blocking_and_async_callers_share_the_lead_random() {
    shuttle::check_random(mixed_callers, ITERATIONS);
  }

  #[test]
  fn blocking_and_async_callers_share_the_lead_pct() {
    shuttle::check_pct(mixed_callers, ITERATIONS, 3);
  }

  /// An async caller takes the lead, parks on the pipe, and its future is dropped. The others must still finish, and so must the dropped caller when it asks again.
  fn dropped_async_leader() {
    let (conn, pipe) = connection();
    let srv = server(pipe, 3);
    let dropper = {
      let conn = conn.clone();
      thread::spawn(move || {
        let id = conn.open_sync(head()).unwrap();
        {
          let mut wait = std::pin::pin!(conn.next_events_async(id, None));
          let mut cx = Context::from_waker(Waker::noop());
          if let std::task::Poll::Ready(events) = wait.as_mut().poll(&mut cx) {
            assert_eq!(kind(&events.unwrap()[0]), "head");
            return;
          }
        }
        let mut seen = Vec::new();
        'read: loop {
          for event in conn.next_events_sync(id, None).unwrap() {
            seen.push(kind(&event));
            if matches!(event, Event::End) {
              break 'read;
            }
          }
        }
        assert_eq!(seen.last(), Some(&"end"));
      })
    };
    let others: Vec<_> = (0..2)
      .map(|_| {
        let conn = conn.clone();
        thread::spawn(move || exchange_sync(&conn))
      })
      .collect();
    dropper.join().unwrap();
    for c in others {
      assert_eq!(c.join().unwrap(), ["head", "data", "end"]);
    }
    srv.join().unwrap();
  }

  #[test]
  fn a_dropped_async_leader_hands_the_lead_on_random() {
    shuttle::check_random(dropped_async_leader, ITERATIONS);
  }

  #[test]
  fn a_dropped_async_leader_hands_the_lead_on_pct() {
    shuttle::check_pct(dropped_async_leader, ITERATIONS, 3);
  }
}
