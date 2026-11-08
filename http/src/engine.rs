//! Protocol engines: HTTP turned into bytes and back, with no I/O and no clock.
//!
//! A driver feeds an engine the bytes it read, the caller's requests and the time, and carries out the [`NetAction`]s the engine returns.

use std::time::Instant;

use bytes::{Bytes, BytesMut};
use http::{HeaderMap, Method, StatusCode, Uri, Version};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExchangeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Capacity {
  /// One exchange at a time, as HTTP/1.1.
  Unique { available: bool },
  /// Many exchanges at once, as HTTP/2 and HTTP/3. `max` is the peer's limit.
  Shared { open: usize, max: usize },
}

#[derive(Debug, Clone)]
pub struct RequestHead {
  pub method: Method,
  pub uri: Uri,
  pub version: Version,
  pub headers: HeaderMap,
}

#[derive(Debug, Clone)]
pub struct ResponseHead {
  pub status: StatusCode,
  pub version: Version,
  pub headers: HeaderMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenError {
  /// No capacity for another exchange now.
  Full,
  /// The peer is going away. Open the exchange on another connection.
  GoingAway,
  Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NetAction {
  Send(Bytes),
  Flush,
  Close,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
  Informational(ResponseHead),
  Head(ResponseHead),
  Data(Bytes),
  Trailers(HeaderMap),
  End,
  Reset(Error),
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ConnEvent {
  /// Exchanges above `last_id` were never processed and can be retried elsewhere.
  GoAway { last_id: ExchangeId },
  Closed(Option<Error>),
}

#[derive(Debug, Default)]
pub struct Output {
  pub net: Vec<NetAction>,
  pub events: Vec<(ExchangeId, Event)>,
  pub conn: Vec<ConnEvent>,
}

impl Output {
  pub fn is_empty(&self) -> bool {
    self.net.is_empty() && self.events.is_empty() && self.conn.is_empty()
  }
}

pub trait Engine {
  fn capacity(&self) -> Capacity;

  fn open(&mut self, head: RequestHead, out: &mut Output) -> Result<ExchangeId, OpenError>;

  /// Returns how much of `data` was accepted. HTTP/2 flow control can take less than offered.
  fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize;

  /// The caller has read `n` body bytes of `id`.
  fn consumed(&mut self, id: ExchangeId, n: usize, out: &mut Output);

  fn reset(&mut self, id: ExchangeId, out: &mut Output);

  /// Takes what it can from `input` and leaves a partial frame there for the next call.
  fn on_bytes(&mut self, input: &mut BytesMut, out: &mut Output);

  /// The peer closed its side. An HTTP/1.1 body without a length ends here.
  fn on_eof(&mut self, out: &mut Output);

  fn on_tick(&mut self, now: Instant, out: &mut Output);

  fn next_deadline(&self) -> Option<Instant>;

  fn close(&mut self, out: &mut Output);
}
