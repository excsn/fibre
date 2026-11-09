//! HTTP/2 engine over `h2`, polled by hand over in-memory buffers.
//!
//! `h2` reads and writes two byte buffers instead of a socket and is polled with one flag waker per exchange, so only exchanges `h2` woke are polled again.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use ::h2::client::{Builder, Connection, ResponseFuture, SendRequest};
use ::h2::{Ping, PingPong, Reason, RecvStream, SendStream};
use bytes::{Bytes, BytesMut};
use http::header::{CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING, UPGRADE};
use http::{Method, Version};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::Error;
use crate::engine::{self, Capacity, ConnEvent, Event, ExchangeId, NetAction, OpenError, Output, RequestHead, ResponseHead};

const MAX_ROUNDS: usize = 64;

#[derive(Default)]
struct Bufs {
  inbound: VecDeque<u8>,
  outbound: Vec<u8>,
  eof: bool,
  read_waker: Option<Waker>,
}

#[derive(Clone, Default)]
struct MemIo(Arc<Mutex<Bufs>>);

impl AsyncRead for MemIo {
  fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
    let mut b = self.0.lock();
    if b.inbound.is_empty() {
      if b.eof {
        return Poll::Ready(Ok(()));
      }
      b.read_waker = Some(cx.waker().clone());
      return Poll::Pending;
    }
    let n = buf.remaining().min(b.inbound.len());
    let (front, back) = b.inbound.as_slices();
    let from_front = n.min(front.len());
    buf.put_slice(&front[..from_front]);
    buf.put_slice(&back[..n - from_front]);
    b.inbound.drain(..n);
    Poll::Ready(Ok(()))
  }
}

impl AsyncWrite for MemIo {
  fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
    self.0.lock().outbound.extend_from_slice(data);
    Poll::Ready(Ok(data.len()))
  }

  fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }

  fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

struct Flag(AtomicBool);

impl Flag {
  fn raised() -> Arc<Self> {
    Arc::new(Flag(AtomicBool::new(true)))
  }

  fn raise(&self) {
    self.0.store(true, Ordering::Release);
  }

  fn is_raised(&self) -> bool {
    self.0.load(Ordering::Acquire)
  }

  fn take(&self) -> bool {
    self.0.swap(false, Ordering::AcqRel)
  }
}

impl Wake for Flag {
  fn wake(self: Arc<Self>) {
    self.raise();
  }

  fn wake_by_ref(self: &Arc<Self>) {
    self.raise();
  }
}

struct Exchange {
  flag: Arc<Flag>,
  response: Option<ResponseFuture>,
  body: Option<RecvStream>,
  reading_trailers: bool,
  send: Option<SendStream<Bytes>>,
  wants_capacity: bool,
}

type Handshake = Pin<Box<dyn Future<Output = Result<(SendRequest<Bytes>, Connection<MemIo, Bytes>), ::h2::Error>> + Send>>;

pub struct Engine {
  io: MemIo,
  handshake: Option<Handshake>,
  send: Option<SendRequest<Bytes>>,
  conn: Option<Connection<MemIo, Bytes>>,
  conn_flag: Arc<Flag>,
  exchanges: BTreeMap<ExchangeId, Exchange>,
  next_id: u64,
  going_away: bool,
  closed: bool,
  /// PING interval and how long to wait for its PONG.
  keep_alive: Option<(Duration, Duration)>,
  ping_pong: Option<PingPong>,
  ping_sent: Option<Instant>,
  last_read: Instant,
}

impl Engine {
  pub fn new() -> Self {
    let io = MemIo::default();
    let handshake: Handshake = Box::pin(Builder::new().handshake::<MemIo, Bytes>(io.clone()));
    Self {
      io,
      handshake: Some(handshake),
      send: None,
      conn: None,
      conn_flag: Flag::raised(),
      exchanges: BTreeMap::new(),
      next_id: 1,
      going_away: false,
      closed: false,
      keep_alive: None,
      ping_pong: None,
      ping_sent: None,
      last_read: Instant::now(),
    }
  }

  /// Sends a PING after `interval` with nothing read while an exchange is open. With no PONG within `timeout` every open exchange ends in `Error::Timeout` and the connection closes.
  pub fn keep_alive(mut self, interval: Duration, timeout: Duration) -> Self {
    self.keep_alive = Some((interval, timeout));
    self
  }

  fn poll_pong(&mut self, cx: &mut Context<'_>) -> bool {
    if self.ping_sent.is_none() {
      return false;
    }
    let Some(pp) = self.ping_pong.as_mut() else { return false };
    match pp.poll_pong(cx) {
      Poll::Ready(Ok(_)) => {
        self.ping_sent = None;
        self.last_read = Instant::now();
        true
      }
      _ => false,
    }
  }

  fn drive(&mut self, out: &mut Output) {
    for _ in 0..MAX_ROUNDS {
      let mut progressed = false;
      if self.conn_flag.take() {
        progressed |= self.poll_conn(out);
      }
      let woken: Vec<ExchangeId> = self.exchanges.iter().filter(|(_, ex)| ex.flag.take()).map(|(id, _)| *id).collect();
      for id in woken {
        self.poll_exchange(id, out);
        progressed = true;
      }
      let bytes = std::mem::take(&mut self.io.0.lock().outbound);
      if !bytes.is_empty() {
        out.net.push(NetAction::Send(Bytes::from(bytes)));
        progressed = true;
      }
      let pending = self.conn_flag.is_raised() || self.exchanges.values().any(|ex| ex.flag.is_raised());
      if !progressed && !pending {
        break;
      }
    }
  }

  fn poll_conn(&mut self, out: &mut Output) -> bool {
    let waker = Waker::from(self.conn_flag.clone());
    let mut cx = Context::from_waker(&waker);
    if let Some(handshake) = self.handshake.as_mut() {
      match handshake.as_mut().poll(&mut cx) {
        Poll::Ready(Ok((send, mut conn))) => {
          self.send = Some(send);
          self.ping_pong = conn.ping_pong();
          self.conn = Some(conn);
          self.handshake = None;
        }
        Poll::Ready(Err(e)) => {
          self.handshake = None;
          self.end(Some(map_error(&e)), out);
          return true;
        }
        Poll::Pending => return false,
      }
    }
    let Some(conn) = self.conn.as_mut() else { return false };
    let polled = Pin::new(conn).poll(&mut cx);
    let ponged = self.poll_pong(&mut cx);
    match polled {
      Poll::Ready(result) => {
        self.conn = None;
        let ids: Vec<ExchangeId> = self.exchanges.keys().copied().collect();
        for id in ids {
          self.poll_exchange(id, out);
        }
        self.end(result.err().map(|e| map_error(&e)), out);
        true
      }
      Poll::Pending => ponged,
    }
  }

  fn end(&mut self, error: Option<Error>, out: &mut Output) {
    for (id, _) in std::mem::take(&mut self.exchanges) {
      out.events.push((id, Event::Reset(error.clone().unwrap_or(Error::Closed))));
    }
    if !self.closed {
      self.closed = true;
      self.send = None;
      self.conn = None;
      out.net.push(NetAction::Close);
      out.conn.push(ConnEvent::Closed(error));
    }
  }

  fn going_away(&mut self, out: &mut Output) {
    if !self.going_away {
      self.going_away = true;
      out.conn.push(ConnEvent::GoingAway);
    }
  }

  fn fail_exchange(&mut self, id: ExchangeId, e: ::h2::Error, out: &mut Output) {
    self.exchanges.remove(&id);
    if e.is_go_away() {
      self.going_away(out);
    }
    out.events.push((id, Event::Reset(map_error(&e))));
  }

  fn poll_exchange(&mut self, id: ExchangeId, out: &mut Output) {
    let Some(ex) = self.exchanges.get_mut(&id) else { return };
    let waker = Waker::from(ex.flag.clone());
    let mut cx = Context::from_waker(&waker);

    if let Some(response) = ex.response.as_mut() {
      match Pin::new(response).poll(&mut cx) {
        Poll::Ready(Ok(response)) => {
          let (parts, body) = response.into_parts();
          out.events.push((id, Event::Head(ResponseHead { status: parts.status, version: parts.version, headers: parts.headers })));
          ex.response = None;
          ex.body = Some(body);
        }
        Poll::Ready(Err(e)) => return self.fail_exchange(id, e, out),
        Poll::Pending => {}
      }
    }

    if let Some(body) = ex.body.as_mut() {
      while !ex.reading_trailers {
        match body.poll_data(&mut cx) {
          Poll::Ready(Some(Ok(data))) => out.events.push((id, Event::Data(data))),
          Poll::Ready(Some(Err(e))) => return self.fail_exchange(id, e, out),
          Poll::Ready(None) => ex.reading_trailers = true,
          Poll::Pending => break,
        }
      }
      if ex.reading_trailers {
        match body.poll_trailers(&mut cx) {
          Poll::Ready(Ok(trailers)) => {
            if let Some(trailers) = trailers {
              out.events.push((id, Event::Trailers(trailers)));
            }
            out.events.push((id, Event::End));
            self.exchanges.remove(&id);
            return;
          }
          Poll::Ready(Err(e)) => return self.fail_exchange(id, e, out),
          Poll::Pending => {}
        }
      }
    }

    if ex.wants_capacity {
      if let Some(send) = ex.send.as_mut() {
        if let Poll::Ready(Some(Ok(n))) = send.poll_capacity(&mut cx) {
          if n > 0 {
            ex.wants_capacity = false;
            out.events.push((id, Event::Writable));
          }
        }
      }
    }
  }

  fn max_streams(&self) -> usize {
    self.conn.as_ref().map(|c| c.max_concurrent_send_streams()).unwrap_or(0)
  }
}

impl Default for Engine {
  fn default() -> Self {
    Self::new()
  }
}

fn map_error(e: &::h2::Error) -> Error {
  if e.is_go_away() && e.is_remote() {
    Error::Refused
  } else if e.is_reset() && e.is_remote() && e.reason() == Some(Reason::REFUSED_STREAM) {
    // RFC 9113 section 8.7: REFUSED_STREAM means no application processing happened.
    Error::Refused
  } else if e.is_reset() {
    Error::Reset(e.reason().map(u32::from).unwrap_or(0))
  } else if e.is_io() {
    Error::Closed
  } else {
    Error::Protocol(e.to_string())
  }
}

fn has_body(head: &RequestHead) -> bool {
  match head.headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()) {
    Some(n) => n > 0,
    None => matches!(head.method, Method::POST | Method::PUT | Method::PATCH),
  }
}

impl engine::Engine for Engine {
  fn capacity(&self) -> Capacity {
    if self.closed || self.going_away {
      return Capacity::Shared { open: self.exchanges.len(), max: 0 };
    }
    Capacity::Shared { open: self.exchanges.len(), max: self.max_streams() }
  }

  fn open(&mut self, head: RequestHead, _now: Instant, out: &mut Output) -> Result<ExchangeId, OpenError> {
    if self.send.is_none() {
      self.drive(out);
    }
    if self.closed {
      return Err(OpenError::Closed);
    }
    if self.going_away {
      return Err(OpenError::GoingAway);
    }
    if self.conn.is_some() && self.exchanges.len() >= self.max_streams() {
      return Err(OpenError::Full);
    }
    let body = has_body(&head);
    let mut builder = http::Request::builder().method(head.method).uri(head.uri).version(Version::HTTP_2);
    if let Some(headers) = builder.headers_mut() {
      *headers = head.headers;
      for name in [CONNECTION, TRANSFER_ENCODING, UPGRADE] {
        headers.remove(name);
      }
      headers.remove("keep-alive");
      headers.remove("proxy-connection");
    }
    let request = builder.body(()).map_err(|_| OpenError::Closed)?;

    let send = self.send.as_mut().ok_or(OpenError::Full)?;
    let mut cx = Context::from_waker(Waker::noop());
    match send.poll_ready(&mut cx) {
      Poll::Ready(Ok(())) => {}
      Poll::Pending => return Err(OpenError::Full),
      Poll::Ready(Err(e)) => {
        if e.is_go_away() {
          self.going_away(out);
          return Err(OpenError::GoingAway);
        }
        return Err(OpenError::Closed);
      }
    }
    let (response, stream) = match send.send_request(request, !body) {
      Ok(pair) => pair,
      Err(e) => {
        if e.is_go_away() {
          self.going_away(out);
          return Err(OpenError::GoingAway);
        }
        return Err(OpenError::Closed);
      }
    };
    let id = ExchangeId(self.next_id);
    self.next_id += 1;
    self.exchanges.insert(
      id,
      Exchange { flag: Flag::raised(), response: Some(response), body: None, reading_trailers: false, send: body.then_some(stream), wants_capacity: false },
    );
    self.conn_flag.raise();
    self.drive(out);
    Ok(id)
  }

  fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize {
    let Some(send) = self.exchanges.get_mut(&id).and_then(|ex| ex.send.as_mut()) else { return 0 };
    if data.is_empty() {
      if end {
        let _ = send.send_data(Bytes::new(), true);
        if let Some(ex) = self.exchanges.get_mut(&id) {
          ex.send = None;
        }
      }
      self.conn_flag.raise();
      self.drive(out);
      return 0;
    }
    send.reserve_capacity(data.len());
    if send.capacity() == 0 {
      self.conn_flag.raise();
      self.drive(out);
    }
    let Some(ex) = self.exchanges.get_mut(&id) else { return 0 };
    let Some(send) = ex.send.as_mut() else { return 0 };
    let n = send.capacity().min(data.len());
    if n == 0 {
      ex.wants_capacity = true;
      ex.flag.raise();
      return 0;
    }
    let last = end && n == data.len();
    if send.send_data(data.slice(..n), last).is_err() {
      return 0;
    }
    if last {
      ex.send = None;
    } else if n < data.len() {
      ex.wants_capacity = true;
      ex.flag.raise();
    }
    self.conn_flag.raise();
    self.drive(out);
    n
  }

  fn consumed(&mut self, id: ExchangeId, n: usize, out: &mut Output) {
    if let Some(body) = self.exchanges.get_mut(&id).and_then(|ex| ex.body.as_mut()) {
      // The window update goes out on the next drive. The caller that consumed reads next, so that drive comes.
      let _ = body.flow_control().release_capacity(n);
      self.conn_flag.raise();
      let _ = out;
    }
  }

  fn reset(&mut self, id: ExchangeId, out: &mut Output) {
    if let Some(mut ex) = self.exchanges.remove(&id) {
      if let Some(send) = ex.send.as_mut() {
        send.send_reset(Reason::CANCEL);
      }
      drop(ex);
      self.conn_flag.raise();
      self.drive(out);
    }
  }

  fn on_bytes(&mut self, input: &mut BytesMut, out: &mut Output) {
    if !input.is_empty() {
      self.last_read = Instant::now();
      let waker = {
        let mut b = self.io.0.lock();
        b.inbound.extend(input.iter());
        b.read_waker.take()
      };
      input.clear();
      if let Some(w) = waker {
        w.wake();
      }
      self.conn_flag.raise();
    }
    self.drive(out);
  }

  fn on_eof(&mut self, out: &mut Output) {
    let waker = {
      let mut b = self.io.0.lock();
      b.eof = true;
      b.read_waker.take()
    };
    if let Some(w) = waker {
      w.wake();
    }
    self.conn_flag.raise();
    self.drive(out);
    if !self.closed {
      self.end(Some(Error::Closed), out);
    }
  }

  fn on_tick(&mut self, now: Instant, out: &mut Output) {
    let Some((interval, timeout)) = self.keep_alive else { return };
    if self.closed || self.exchanges.is_empty() {
      return;
    }
    match self.ping_sent {
      Some(sent) if now >= sent + timeout => self.end(Some(Error::Timeout), out),
      Some(_) => {}
      None if now >= self.last_read + interval => {
        let Some(pp) = self.ping_pong.as_mut() else { return };
        if pp.send_ping(Ping::opaque()).is_ok() {
          self.ping_sent = Some(now);
          self.conn_flag.raise();
          self.drive(out);
        }
      }
      None => {}
    }
  }

  fn next_deadline(&self) -> Option<Instant> {
    let (interval, timeout) = self.keep_alive?;
    if self.closed || self.exchanges.is_empty() || self.ping_pong.is_none() {
      return None;
    }
    Some(match self.ping_sent {
      Some(sent) => sent + timeout,
      None => self.last_read + interval,
    })
  }

  fn close(&mut self, out: &mut Output) {
    self.end(None, out);
  }
}
