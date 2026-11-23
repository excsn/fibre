//! HTTP/3 engine over quinn-proto, which is QUIC with no I/O and no clock.
//!
//! Datagrams go in through `on_input` and come out as `NetAction::Datagram`. Every call ends by taking whatever quinn-proto has to send, so no output waits for a later call.

mod qpack;
mod tables;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::{Buf, Bytes, BytesMut};
use http::header::{CONNECTION, CONTENT_LENGTH, HOST, TE, TRANSFER_ENCODING, UPGRADE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use quinn_proto::{ConnectionError, ConnectionHandle, Dir, Endpoint, EndpointConfig, Event as QuicEvent, ReadError, StreamEvent, StreamId, VarInt, WriteError};

use crate::Error;
use crate::engine::{self, Capacity, ConnEvent, Event, ExchangeId, Input, NetAction, OpenError, Output, RequestHead, ResponseHead};

const FRAME_DATA: u64 = 0x00;
const FRAME_HEADERS: u64 = 0x01;
const FRAME_SETTINGS: u64 = 0x04;
const FRAME_GOAWAY: u64 = 0x07;
const STREAM_CONTROL: u64 = 0x00;
/// RFC 9114 section 8.1.
const H3_NO_ERROR: u32 = 0x100;
const H3_REQUEST_REJECTED: u32 = 0x10b;
const H3_REQUEST_CANCELLED: u32 = 0x10c;
/// Frame types RFC 9114 section 7.2.8 reserves from HTTP/2, plus the control-stream frames. None may appear on a request stream.
const NOT_ON_A_REQUEST: [u64; 8] = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09];
/// Response bytes read from a stream and not yet consumed by the caller. Past this the stream is not read, so QUIC flow control holds the server back.
const RECV_WINDOW: usize = 1024 * 1024;
/// The most of one `send_body` call taken at once.
const SEND_CHUNK: usize = 256 * 1024;

struct Exchange {
  stream: StreamId,
  /// Request frames quinn-proto has not taken yet.
  out: BytesMut,
  /// Finish the send side once `out` is empty.
  finish: bool,
  /// The request body may still grow.
  sending: bool,
  /// The server stopped reading the request. Body offered from now on is dropped.
  stopped: bool,
  /// The caller was told to wait for `Event::Writable`.
  wants_writable: bool,
  input: BytesMut,
  /// Bytes left in the DATA frame being read.
  data_left: u64,
  head: bool,
  /// `Event::Data` bytes not yet consumed.
  unconsumed: usize,
  /// The stream was left unread because `unconsumed` reached `RECV_WINDOW`.
  paused: bool,
}

enum Uni {
  Unknown(BytesMut),
  Control(BytesMut),
  Discard,
}

pub struct Engine {
  endpoint: Endpoint,
  handle: ConnectionHandle,
  conn: quinn_proto::Connection,
  remote: SocketAddr,
  tx: Vec<u8>,
  reply: Vec<u8>,
  exchanges: HashMap<ExchangeId, Exchange>,
  by_stream: HashMap<StreamId, ExchangeId>,
  uni: HashMap<StreamId, Uni>,
  control: Option<(StreamId, BytesMut)>,
  next_id: u64,
  connected: bool,
  /// `streams().open` found the peer's limit.
  blocked: bool,
  going_away: bool,
  closed: bool,
  deadline: Option<Instant>,
}

impl Engine {
  /// A connection to `remote` that verifies the server as `server_name`. `config` carries the TLS configuration, with `h3` as its only ALPN protocol.
  pub fn new(config: quinn_proto::ClientConfig, server_name: &str, remote: SocketAddr, now: Instant) -> Result<Self, Error> {
    let mut endpoint = Endpoint::new(Arc::new(EndpointConfig::default()), None, true, None);
    let (handle, conn) = endpoint.connect(now, config, remote, server_name).map_err(|e| Error::Protocol(e.to_string()))?;
    let mut engine = Engine {
      endpoint,
      handle,
      conn,
      remote,
      tx: Vec::with_capacity(1500),
      reply: Vec::new(),
      exchanges: HashMap::new(),
      by_stream: HashMap::new(),
      uni: HashMap::new(),
      control: None,
      next_id: 1,
      connected: false,
      blocked: false,
      going_away: false,
      closed: false,
      deadline: None,
    };
    engine.deadline = engine.conn.poll_timeout();
    Ok(engine)
  }

  fn process(&mut self, now: Instant, out: &mut Output) {
    loop {
      self.endpoint_events();
      let Some(event) = self.conn.poll() else { break };
      match event {
        QuicEvent::Connected => {
          self.connected = true;
          self.open_control();
        }
        QuicEvent::ConnectionLost { reason } => self.end(Some(map_lost(&reason)), out),
        QuicEvent::Stream(StreamEvent::Opened { dir: Dir::Uni }) => {
          while let Some(stream) = self.conn.streams().accept(Dir::Uni) {
            self.uni.insert(stream, Uni::Unknown(BytesMut::new()));
            self.read_uni(stream, out);
          }
        }
        QuicEvent::Stream(StreamEvent::Opened { dir: Dir::Bi }) => {
          while let Some(stream) = self.conn.streams().accept(Dir::Bi) {
            let _ = self.conn.recv_stream(stream).stop(code(0x103));
            let _ = self.conn.send_stream(stream).reset(code(0x103));
          }
        }
        QuicEvent::Stream(StreamEvent::Readable { id: stream }) => {
          if let Some(&id) = self.by_stream.get(&stream) {
            self.read_exchange(id, out);
          } else if self.uni.contains_key(&stream) {
            self.read_uni(stream, out);
          }
        }
        QuicEvent::Stream(StreamEvent::Writable { id: stream }) => {
          if let Some(&id) = self.by_stream.get(&stream) {
            self.write_exchange(id, out);
          } else {
            self.write_control();
          }
        }
        QuicEvent::Stream(StreamEvent::Stopped { id: stream, .. }) => {
          if let Some(ex) = self.by_stream.get(&stream).and_then(|id| self.exchanges.get_mut(id)) {
            ex.stopped = true;
            ex.out.clear();
            ex.finish = false;
            ex.sending = false;
            if ex.wants_writable {
              ex.wants_writable = false;
              let id = self.by_stream[&stream];
              out.events.push((id, Event::Writable));
            }
          }
        }
        QuicEvent::Stream(StreamEvent::Available { dir: Dir::Bi }) => self.blocked = false,
        _ => {}
      }
    }
    self.transmit(now, out);
  }

  fn endpoint_events(&mut self) {
    while let Some(event) = self.conn.poll_endpoint_events() {
      if let Some(reply) = self.endpoint.handle_event(self.handle, event) {
        self.conn.handle_event(reply);
      }
    }
  }

  fn transmit(&mut self, now: Instant, out: &mut Output) {
    loop {
      self.endpoint_events();
      self.tx.clear();
      let Some(t) = self.conn.poll_transmit(now, 1, &mut self.tx) else { break };
      out.net.push(NetAction::Datagram(Bytes::copy_from_slice(&self.tx[..t.size])));
    }
    self.deadline = self.conn.poll_timeout();
  }

  fn open_control(&mut self) {
    let Some(stream) = self.conn.streams().open(Dir::Uni) else { return };
    let mut frames = Vec::with_capacity(3);
    put_varint(&mut frames, STREAM_CONTROL);
    put_varint(&mut frames, FRAME_SETTINGS);
    put_varint(&mut frames, 0);
    self.control = Some((stream, BytesMut::from(&frames[..])));
    self.write_control();
  }

  fn write_control(&mut self) {
    let Some((stream, frames)) = self.control.as_mut() else { return };
    if !frames.is_empty() {
      if let Ok(n) = self.conn.send_stream(*stream).write(frames) {
        frames.advance(n);
      }
    }
  }

  /// Hands `out` to quinn-proto as far as stream credit allows, then finishes the send side when asked.
  fn write_exchange(&mut self, id: ExchangeId, out: &mut Output) {
    let Some(ex) = self.exchanges.get_mut(&id) else { return };
    let mut send = self.conn.send_stream(ex.stream);
    while !ex.out.is_empty() {
      match send.write(&ex.out) {
        Ok(n) => ex.out.advance(n),
        Err(WriteError::Blocked) => break,
        Err(_) => {
          ex.stopped = true;
          ex.out.clear();
          ex.finish = false;
        }
      }
    }
    if ex.out.is_empty() {
      if ex.finish {
        let _ = send.finish();
        ex.finish = false;
      }
      if ex.wants_writable {
        ex.wants_writable = false;
        out.events.push((id, Event::Writable));
      }
    }
  }

  fn read_exchange(&mut self, id: ExchangeId, out: &mut Output) {
    let Some(ex) = self.exchanges.get_mut(&id) else { return };
    let mut fin = false;
    let mut reset = None;
    {
      let mut recv = self.conn.recv_stream(ex.stream);
      if let Ok(mut chunks) = recv.read(true) {
        loop {
          let room = RECV_WINDOW.saturating_sub(ex.unconsumed + ex.input.len());
          if room == 0 {
            ex.paused = true;
            break;
          }
          match chunks.next(room) {
            Ok(Some(chunk)) => ex.input.extend_from_slice(&chunk.bytes),
            Ok(None) => {
              fin = true;
              break;
            }
            Err(ReadError::Blocked) => break,
            Err(ReadError::Reset(code)) => {
              reset = Some(code.into_inner());
              break;
            }
          }
        }
        let _ = chunks.finalize();
      }
    }
    if let Some(code) = reset {
      let error = if code == H3_REQUEST_REJECTED as u64 { Error::Refused } else { Error::Reset(code as u32) };
      return self.fail(id, error, out);
    }
    if let Err(e) = self.frames(id, out) {
      return self.fail(id, e, out);
    }
    if fin {
      let ex = &self.exchanges[&id];
      if !ex.head || !ex.input.is_empty() || ex.data_left > 0 {
        return self.fail(id, Error::Protocol("the response ended inside a frame or before its head".into()), out);
      }
      if let Some(ex) = self.remove(id) {
        if ex.sending || ex.finish || !ex.out.is_empty() {
          // RFC 9114 section 4.1.2: the response is complete, so the rest of the request is not needed.
          let _ = self.conn.send_stream(ex.stream).reset(code(H3_NO_ERROR));
        }
      }
      out.events.push((id, Event::End));
    }
  }

  /// Turns the complete frames in an exchange's input into events.
  fn frames(&mut self, id: ExchangeId, out: &mut Output) -> Result<(), Error> {
    let ex = self.exchanges.get_mut(&id).expect("exchange is open");
    loop {
      if ex.data_left > 0 {
        let take = (ex.input.len() as u64).min(ex.data_left) as usize;
        if take == 0 {
          return Ok(());
        }
        ex.data_left -= take as u64;
        ex.unconsumed += take;
        out.events.push((id, Event::Data(ex.input.split_to(take).freeze())));
        continue;
      }
      let mut b = &ex.input[..];
      let (Some(kind), Some(len)) = (get_varint(&mut b), get_varint(&mut b)) else { return Ok(()) };
      let header = ex.input.len() - b.len();
      if kind == FRAME_DATA {
        if !ex.head {
          return Err(Error::Protocol("DATA before the response head".into()));
        }
        ex.input.advance(header);
        ex.data_left = len;
        continue;
      }
      if NOT_ON_A_REQUEST.contains(&kind) {
        return Err(Error::Protocol(format!("frame type {kind:#x} on a request stream")));
      }
      if (b.len() as u64) < len {
        return Ok(());
      }
      let payload = ex.input.split_to(header + len as usize).split_off(header);
      if kind != FRAME_HEADERS {
        continue;
      }
      let fields = qpack::decode(&payload).map_err(|e| Error::Protocol(e.0.into()))?;
      if ex.head {
        let mut trailers = HeaderMap::new();
        for (n, v) in fields {
          if n.first() == Some(&b':') {
            return Err(Error::Protocol("pseudo-header in trailers".into()));
          }
          trailers.append(header_name(&n)?, header_value(&v)?);
        }
        out.events.push((id, Event::Trailers(trailers)));
        continue;
      }
      let head = response_head(fields)?;
      if head.status.is_informational() {
        out.events.push((id, Event::Informational(head)));
      } else {
        ex.head = true;
        out.events.push((id, Event::Head(head)));
      }
    }
  }

  fn read_uni(&mut self, stream: StreamId, out: &mut Output) {
    let mut data = BytesMut::new();
    {
      let mut recv = self.conn.recv_stream(stream);
      if let Ok(mut chunks) = recv.read(true) {
        while let Ok(Some(chunk)) = chunks.next(usize::MAX) {
          data.extend_from_slice(&chunk.bytes);
        }
        let _ = chunks.finalize();
      }
    }
    let Some(state) = self.uni.get_mut(&stream) else { return };
    match state {
      Uni::Unknown(buf) => {
        buf.extend_from_slice(&data);
        let mut b = &buf[..];
        let Some(kind) = get_varint(&mut b) else { return };
        let rest = BytesMut::from(b);
        *state = if kind == STREAM_CONTROL { Uni::Control(rest) } else { Uni::Discard };
      }
      Uni::Control(buf) => buf.extend_from_slice(&data),
      Uni::Discard => return,
    }
    let Some(Uni::Control(buf)) = self.uni.get_mut(&stream) else { return };
    let mut goaway = None;
    loop {
      let mut b = &buf[..];
      let (Some(kind), Some(len)) = (get_varint(&mut b), get_varint(&mut b)) else { break };
      if (b.len() as u64) < len {
        break;
      }
      if kind == FRAME_GOAWAY {
        let mut p = &b[..len as usize];
        goaway = get_varint(&mut p);
      }
      let used = buf.len() - b.len() + len as usize;
      buf.advance(used);
    }
    if let Some(last) = goaway {
      self.go_away(last, out);
    }
  }

  /// RFC 9114 section 5.2: requests on streams from `last` up were not processed and may go again on another connection.
  fn go_away(&mut self, last: u64, out: &mut Output) {
    if !self.going_away {
      self.going_away = true;
      out.conn.push(ConnEvent::GoingAway);
    }
    let unprocessed: Vec<ExchangeId> = self.exchanges.iter().filter(|(_, ex)| VarInt::from(ex.stream).into_inner() >= last).map(|(id, _)| *id).collect();
    for id in unprocessed {
      self.fail(id, Error::Refused, out);
    }
  }

  fn remove(&mut self, id: ExchangeId) -> Option<Exchange> {
    let ex = self.exchanges.remove(&id)?;
    self.by_stream.remove(&ex.stream);
    Some(ex)
  }

  /// Ends one exchange with `error` and cancels its stream.
  fn fail(&mut self, id: ExchangeId, error: Error, out: &mut Output) {
    if let Some(ex) = self.remove(id) {
      let _ = self.conn.send_stream(ex.stream).reset(code(H3_REQUEST_CANCELLED));
      let _ = self.conn.recv_stream(ex.stream).stop(code(H3_REQUEST_CANCELLED));
      out.events.push((id, Event::Reset(error)));
    }
  }

  fn end(&mut self, error: Option<Error>, out: &mut Output) {
    for (id, _) in std::mem::take(&mut self.exchanges) {
      out.events.push((id, Event::Reset(error.clone().unwrap_or(Error::Closed))));
    }
    self.by_stream.clear();
    if !self.closed {
      self.closed = true;
      out.net.push(NetAction::Close);
      out.conn.push(ConnEvent::Closed(error));
    }
  }

  fn run_timers(&mut self, now: Instant) {
    if self.deadline.is_some_and(|d| d <= now) {
      self.conn.handle_timeout(now);
    }
  }
}

fn code(c: u32) -> VarInt {
  VarInt::from_u32(c)
}

fn map_lost(reason: &ConnectionError) -> Error {
  let crypto = |c: u64| (0x100..0x200).contains(&c);
  match reason {
    ConnectionError::TimedOut => Error::Timeout,
    ConnectionError::TransportError(e) if crypto(u64::from(e.code)) => Error::Tls(e.to_string()),
    ConnectionError::ConnectionClosed(c) if crypto(u64::from(c.error_code)) => Error::Tls(c.to_string()),
    ConnectionError::ApplicationClosed(c) if c.error_code.into_inner() == H3_NO_ERROR as u64 => Error::Closed,
    ConnectionError::Reset | ConnectionError::LocallyClosed => Error::Closed,
    other => Error::Protocol(other.to_string()),
  }
}

fn header_name(n: &[u8]) -> Result<HeaderName, Error> {
  HeaderName::from_bytes(n).map_err(|_| Error::Protocol("invalid header name".into()))
}

fn header_value(v: &[u8]) -> Result<HeaderValue, Error> {
  HeaderValue::from_bytes(v).map_err(|_| Error::Protocol("invalid header value".into()))
}

fn response_head(fields: Vec<(Vec<u8>, Vec<u8>)>) -> Result<ResponseHead, Error> {
  let mut status = None;
  let mut headers = HeaderMap::with_capacity(fields.len());
  for (n, v) in fields {
    if n == b":status" {
      status = Some(StatusCode::from_bytes(&v).map_err(|_| Error::Protocol("invalid :status".into()))?);
    } else if n.first() == Some(&b':') {
      return Err(Error::Protocol("unknown pseudo-header in a response".into()));
    } else {
      headers.append(header_name(&n)?, header_value(&v)?);
    }
  }
  let status = status.ok_or_else(|| Error::Protocol("response without :status".into()))?;
  Ok(ResponseHead { status, version: Version::HTTP_3, headers })
}

fn has_body(head: &RequestHead) -> bool {
  match head.headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()) {
    Some(n) => n > 0,
    None => matches!(head.method, Method::POST | Method::PUT | Method::PATCH),
  }
}

/// The HEADERS frame for `head`.
fn request_frame(head: &RequestHead) -> Vec<u8> {
  let authority = head.uri.authority().map(|a| a.as_str().to_owned()).or_else(|| head.headers.get(HOST).and_then(|h| h.to_str().ok()).map(str::to_owned)).unwrap_or_default();
  let path = head.uri.path_and_query().map(|p| p.as_str()).filter(|p| !p.is_empty()).unwrap_or("/");
  let mut block = Vec::with_capacity(64 + authority.len() + path.len());
  qpack::begin(&mut block);
  qpack::field(&mut block, ":method", head.method.as_str().as_bytes());
  qpack::field(&mut block, ":scheme", head.uri.scheme_str().unwrap_or("https").as_bytes());
  qpack::field(&mut block, ":authority", authority.as_bytes());
  qpack::field(&mut block, ":path", path.as_bytes());
  for (name, value) in &head.headers {
    if [CONNECTION, TRANSFER_ENCODING, UPGRADE, HOST].contains(name) || name == "keep-alive" || name == "proxy-connection" {
      continue;
    }
    if name == TE && value != "trailers" {
      continue;
    }
    qpack::field(&mut block, name.as_str(), value.as_bytes());
  }
  let mut frame = Vec::with_capacity(block.len() + 9);
  put_varint(&mut frame, FRAME_HEADERS);
  put_varint(&mut frame, block.len() as u64);
  frame.extend_from_slice(&block);
  frame
}

fn get_varint(b: &mut &[u8]) -> Option<u64> {
  let first = *b.first()?;
  let len = 1usize << (first >> 6);
  if b.len() < len {
    return None;
  }
  let mut v = (first & 0x3f) as u64;
  for &byte in &b[1..len] {
    v = (v << 8) | byte as u64;
  }
  *b = &b[len..];
  Some(v)
}

fn put_varint(out: &mut Vec<u8>, v: u64) {
  if v < 1 << 6 {
    out.push(v as u8);
  } else if v < 1 << 14 {
    out.extend_from_slice(&((v as u16) | 0x4000).to_be_bytes());
  } else if v < 1 << 30 {
    out.extend_from_slice(&((v as u32) | 0x8000_0000).to_be_bytes());
  } else {
    out.extend_from_slice(&(v | 0xc000_0000_0000_0000).to_be_bytes());
  }
}

impl engine::Engine for Engine {
  fn capacity(&self) -> Capacity {
    let open = self.exchanges.len();
    let max = if self.closed || self.going_away || !self.connected {
      0
    } else if self.blocked {
      open
    } else {
      open + 1
    };
    Capacity::Shared { open, max }
  }

  fn open(&mut self, head: RequestHead, now: Instant, out: &mut Output) -> Result<ExchangeId, OpenError> {
    self.run_timers(now);
    self.process(now, out);
    if self.closed {
      return Err(OpenError::Closed);
    }
    if self.going_away {
      return Err(OpenError::GoingAway);
    }
    if !self.connected {
      return Err(OpenError::Full);
    }
    let Some(stream) = self.conn.streams().open(Dir::Bi) else {
      self.blocked = true;
      return Err(OpenError::Full);
    };
    let body = has_body(&head);
    let id = ExchangeId(self.next_id);
    self.next_id += 1;
    let ex = Exchange {
      stream,
      out: BytesMut::from(&request_frame(&head)[..]),
      finish: !body,
      sending: body,
      stopped: false,
      wants_writable: false,
      input: BytesMut::new(),
      data_left: 0,
      head: false,
      unconsumed: 0,
      paused: false,
    };
    self.exchanges.insert(id, ex);
    self.by_stream.insert(stream, id);
    self.write_exchange(id, out);
    self.process(now, out);
    Ok(id)
  }

  fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize {
    let Some(ex) = self.exchanges.get_mut(&id) else { return 0 };
    if ex.stopped {
      return data.len();
    }
    if !ex.sending {
      return 0;
    }
    if !ex.out.is_empty() {
      ex.wants_writable = true;
      return 0;
    }
    let n = data.len().min(SEND_CHUNK);
    if n > 0 {
      let mut header = Vec::with_capacity(9);
      put_varint(&mut header, FRAME_DATA);
      put_varint(&mut header, n as u64);
      ex.out.extend_from_slice(&header);
      ex.out.extend_from_slice(&data[..n]);
    }
    if end && n == data.len() {
      ex.finish = true;
      ex.sending = false;
    } else if n < data.len() {
      ex.wants_writable = true;
    }
    self.write_exchange(id, out);
    self.process(Instant::now(), out);
    n
  }

  fn consumed(&mut self, id: ExchangeId, n: usize, out: &mut Output) {
    let Some(ex) = self.exchanges.get_mut(&id) else { return };
    ex.unconsumed = ex.unconsumed.saturating_sub(n);
    if ex.paused && ex.unconsumed + ex.input.len() <= RECV_WINDOW / 2 {
      ex.paused = false;
      self.read_exchange(id, out);
      self.process(Instant::now(), out);
    }
  }

  fn reset(&mut self, id: ExchangeId, out: &mut Output) {
    if let Some(ex) = self.remove(id) {
      let _ = self.conn.send_stream(ex.stream).reset(code(H3_REQUEST_CANCELLED));
      let _ = self.conn.recv_stream(ex.stream).stop(code(H3_REQUEST_CANCELLED));
      self.process(Instant::now(), out);
    }
  }

  fn on_input(&mut self, input: Input<'_>, now: Instant, out: &mut Output) {
    let Input::Datagram(data) = input else { return };
    self.reply.clear();
    match self.endpoint.handle(now, self.remote, None, None, data, &mut self.reply) {
      Some(quinn_proto::DatagramEvent::ConnectionEvent(_, event)) => self.conn.handle_event(event),
      Some(quinn_proto::DatagramEvent::Response(t)) => out.net.push(NetAction::Datagram(Bytes::copy_from_slice(&self.reply[..t.size]))),
      _ => {}
    }
    self.process(now, out);
  }

  fn on_eof(&mut self, out: &mut Output) {
    self.end(Some(Error::Closed), out);
  }

  fn on_tick(&mut self, now: Instant, out: &mut Output) {
    self.run_timers(now);
    self.process(now, out);
  }

  fn next_deadline(&self) -> Option<Instant> {
    if self.closed { None } else { self.deadline }
  }

  fn close(&mut self, out: &mut Output) {
    let now = Instant::now();
    self.conn.close(now, code(H3_NO_ERROR), Bytes::new());
    self.transmit(now, out);
    self.end(None, out);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn varints_round_trip_at_every_length() {
    for v in [0u64, 63, 64, 16383, 16384, (1 << 30) - 1, 1 << 30, (1 << 62) - 1] {
      let mut out = Vec::new();
      put_varint(&mut out, v);
      assert_eq!(get_varint(&mut &out[..]), Some(v));
    }
  }

  #[test]
  fn a_request_frame_carries_the_pseudo_headers_and_drops_connection_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(HOST, HeaderValue::from_static("other.test"));
    headers.insert("x-a", HeaderValue::from_static("1"));
    let head = RequestHead { method: Method::POST, uri: "https://example.test:8443/p?q=1".parse().unwrap(), version: Version::HTTP_3, headers };
    let frame = request_frame(&head);
    let mut b = &frame[..];
    assert_eq!(get_varint(&mut b), Some(FRAME_HEADERS));
    let len = get_varint(&mut b).unwrap() as usize;
    assert_eq!(b.len(), len);
    let fields: Vec<(String, String)> = qpack::decode(b).unwrap().into_iter().map(|(n, v)| (String::from_utf8(n).unwrap(), String::from_utf8(v).unwrap())).collect();
    let expected = [(":method", "POST"), (":scheme", "https"), (":authority", "example.test:8443"), (":path", "/p?q=1"), ("x-a", "1")];
    assert_eq!(fields, expected.map(|(n, v)| (n.to_owned(), v.to_owned())));
  }
}
