#![cfg(feature = "http2")]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use bytes::{Bytes, BytesMut};
use fibre_http::Error;
use fibre_http::engine::{Capacity, ConnEvent, Engine as _, Event, ExchangeId, NetAction, OpenError, Output, RequestHead};
use fibre_http::h2::Engine;
use h2::RecvStream;
use h2::server::SendResponse;
use http::{HeaderMap, HeaderValue, Method, Request, Response, Version};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Default)]
struct PipeBufs {
  inbound: VecDeque<u8>,
  outbound: Vec<u8>,
}

#[derive(Clone, Default)]
struct Pipe(Rc<RefCell<PipeBufs>>);

impl AsyncRead for Pipe {
  fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
    let mut b = self.0.borrow_mut();
    if b.inbound.is_empty() {
      return Poll::Pending;
    }
    let n = buf.remaining().min(b.inbound.len());
    let chunk: Vec<u8> = b.inbound.drain(..n).collect();
    buf.put_slice(&chunk);
    Poll::Ready(Ok(()))
  }
}

impl AsyncWrite for Pipe {
  fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
    self.0.borrow_mut().outbound.extend_from_slice(data);
    Poll::Ready(Ok(data.len()))
  }
  fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }
  fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

type Task = Pin<Box<dyn Future<Output = ()>>>;

#[derive(Clone, Default)]
struct Spawner(Rc<RefCell<Vec<Task>>>);

impl Spawner {
  fn spawn(&self, f: impl Future<Output = ()> + 'static) {
    self.0.borrow_mut().push(Box::pin(f));
  }
}

type Handler = Rc<dyn Fn(Request<RecvStream>, SendResponse<Bytes>, &Spawner)>;

struct Server {
  pipe: Pipe,
  spawner: Spawner,
  main: Option<Task>,
}

impl Server {
  fn new(max_streams: Option<u32>, handler: Handler) -> Self {
    let pipe = Pipe::default();
    let spawner = Spawner::default();
    let (io, sp) = (pipe.clone(), spawner.clone());
    let main: Task = Box::pin(async move {
      let mut builder = h2::server::Builder::new();
      if let Some(n) = max_streams {
        builder.max_concurrent_streams(n);
      }
      let Ok(mut conn) = builder.handshake::<_, Bytes>(io).await else { return };
      while let Some(Ok((req, respond))) = conn.accept().await {
        handler(req, respond, &sp);
      }
    });
    Self { pipe, spawner, main: Some(main) }
  }

  fn poll(&mut self) {
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..4 {
      if let Some(main) = self.main.as_mut() {
        if main.as_mut().poll(&mut cx).is_ready() {
          self.main = None;
        }
      }
      let mut tasks = std::mem::take(&mut *self.spawner.0.borrow_mut());
      tasks.retain_mut(|t| t.as_mut().poll(&mut cx).is_pending());
      self.spawner.0.borrow_mut().extend(tasks);
    }
  }
}

fn respond_with(body: &'static [u8]) -> Handler {
  Rc::new(move |_req, mut respond, _sp| {
    let mut send = respond.send_response(Response::new(()), false).unwrap();
    send.send_data(Bytes::from_static(body), true).unwrap();
  })
}

#[derive(Debug, PartialEq)]
enum Seen {
  Head(u16),
  Data(usize),
  Trailers(Vec<(String, String)>),
  End,
  Reset(Error),
  Writable,
}

struct Harness {
  engine: Engine,
  out: Output,
  input: BytesMut,
  server: Server,
  seen: Vec<(ExchangeId, Seen)>,
  conn: Vec<ConnEvent>,
  closed: bool,
}

impl Harness {
  fn new(max_streams: Option<u32>, handler: Handler) -> Self {
    Self {
      engine: Engine::new(),
      out: Output::default(),
      input: BytesMut::new(),
      server: Server::new(max_streams, handler),
      seen: Vec::new(),
      conn: Vec::new(),
      closed: false,
    }
  }

  fn open(&mut self, method: Method, path: &str, headers: &[(&str, &str)]) -> Result<ExchangeId, OpenError> {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
      map.append(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
    }
    let head = RequestHead { method, uri: format!("https://example.test{path}").parse().unwrap(), version: Version::HTTP_2, headers: map };
    self.engine.open(head, &mut self.out)
  }

  fn collect(&mut self) -> Vec<u8> {
    let mut bytes = Vec::new();
    for action in self.out.net.drain(..) {
      match action {
        NetAction::Send(b) => bytes.extend_from_slice(&b),
        NetAction::Close => self.closed = true,
        _ => {}
      }
    }
    for (id, event) in self.out.events.drain(..) {
      let seen = match event {
        Event::Head(h) => Seen::Head(h.status.as_u16()),
        Event::Data(d) => Seen::Data(d.len()),
        Event::Trailers(t) => Seen::Trailers(t.iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string())).collect()),
        Event::End => Seen::End,
        Event::Reset(e) => Seen::Reset(e),
        Event::Writable => Seen::Writable,
        other => panic!("unexpected event {other:?}"),
      };
      self.seen.push((id, seen));
    }
    self.conn.extend(self.out.conn.drain(..));
    bytes
  }

  fn pump_with(&mut self, step: usize) {
    for _ in 0..64 {
      let to_server = self.collect();
      let sent = !to_server.is_empty();
      self.server.pipe.0.borrow_mut().inbound.extend(to_server);
      self.server.poll();
      let to_client = std::mem::take(&mut self.server.pipe.0.borrow_mut().outbound);
      let received = !to_client.is_empty();
      for chunk in to_client.chunks(step.max(1)) {
        self.input.extend_from_slice(chunk);
        self.engine.on_bytes(&mut self.input, &mut self.out);
      }
      if to_client.is_empty() {
        self.engine.on_bytes(&mut self.input, &mut self.out);
      }
      if !sent && !received && self.out.is_empty() {
        return;
      }
    }
  }

  fn pump(&mut self) {
    self.pump_with(usize::MAX);
  }

  fn of(&mut self, id: ExchangeId) -> Vec<Seen> {
    let mut mine = Vec::new();
    let mut rest = Vec::new();
    for (eid, s) in self.seen.drain(..) {
      if eid == id {
        match (mine.last_mut(), s) {
          (Some(Seen::Data(n)), Seen::Data(m)) => *n += m,
          (_, s) => mine.push(s),
        }
      } else {
        rest.push((eid, s));
      }
    }
    self.seen = rest;
    mine
  }

  fn data_len(&self, id: ExchangeId) -> usize {
    self.seen.iter().filter(|(e, _)| *e == id).map(|(_, s)| if let Seen::Data(n) = s { *n } else { 0 }).sum()
  }

  fn conn_closed(&self) -> bool {
    self.conn.iter().any(|c| matches!(c, ConnEvent::Closed(_)))
  }
}

fn silent() -> (Handler, Rc<RefCell<Vec<SendResponse<Bytes>>>>) {
  let held: Rc<RefCell<Vec<SendResponse<Bytes>>>> = Rc::default();
  let keep = held.clone();
  let handler: Handler = Rc::new(move |req, mut respond, _sp| {
    if req.uri().path() == "/slow" {
      keep.borrow_mut().push(respond);
    } else {
      let mut send = respond.send_response(Response::new(()), false).unwrap();
      send.send_data(Bytes::from_static(b"fast"), true).unwrap();
    }
  });
  (handler, held)
}

#[test]
fn get_round_trip() {
  let mut h = Harness::new(None, respond_with(b"hello"));
  let id = h.open(Method::GET, "/", &[]).unwrap();
  h.pump();
  assert_eq!(h.of(id), vec![Seen::Head(200), Seen::Data(5), Seen::End]);
}

#[test]
fn response_fed_one_byte_at_a_time() {
  let mut h = Harness::new(None, respond_with(b"hello"));
  let id = h.open(Method::GET, "/", &[]).unwrap();
  h.pump_with(1);
  assert_eq!(h.of(id), vec![Seen::Head(200), Seen::Data(5), Seen::End]);
}

#[test]
fn concurrent_exchanges_complete_independently() {
  let mut h = Harness::new(None, respond_with(b"hello"));
  let a = h.open(Method::GET, "/a", &[]).unwrap();
  let b = h.open(Method::GET, "/b", &[]).unwrap();
  assert_ne!(a, b);
  assert!(matches!(h.engine.capacity(), Capacity::Shared { open: 2, .. }));
  h.pump();
  assert_eq!(h.of(a), vec![Seen::Head(200), Seen::Data(5), Seen::End]);
  assert_eq!(h.of(b), vec![Seen::Head(200), Seen::Data(5), Seen::End]);
  assert!(matches!(h.engine.capacity(), Capacity::Shared { open: 0, .. }));
}

#[test]
fn capacity_follows_the_server_stream_limit() {
  let (handler, _held) = silent();
  let mut h = Harness::new(Some(1), handler);
  let a = h.open(Method::GET, "/slow", &[]).unwrap();
  h.pump();
  assert_eq!(h.engine.capacity(), Capacity::Shared { open: 1, max: 1 });
  assert_eq!(h.open(Method::GET, "/b", &[]), Err(OpenError::Full));
  assert_eq!(h.of(a), vec![]);
}

#[test]
fn post_body_reaches_the_server() {
  let got: Rc<RefCell<Vec<u8>>> = Rc::default();
  let sink = got.clone();
  let handler: Handler = Rc::new(move |req, mut respond, sp| {
    let sink = sink.clone();
    sp.spawn(async move {
      let mut body = req.into_body();
      while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        let _ = body.flow_control().release_capacity(chunk.len());
        sink.borrow_mut().extend_from_slice(&chunk);
      }
      let mut send = respond.send_response(Response::new(()), false).unwrap();
      send.send_data(Bytes::from_static(b"ok"), true).unwrap();
    });
  });
  let mut h = Harness::new(None, handler);
  let id = h.open(Method::POST, "/submit", &[("content-length", "5")]).unwrap();
  assert_eq!(h.engine.send_body(id, Bytes::from_static(b"hello"), true, &mut h.out), 5);
  h.pump();
  assert_eq!(&*got.borrow(), b"hello");
  assert_eq!(h.of(id), vec![Seen::Head(200), Seen::Data(2), Seen::End]);
}

#[test]
fn large_body_waits_for_consumed() {
  const SIZE: usize = 200_000;
  static BIG: [u8; SIZE] = [7; SIZE];
  let mut h = Harness::new(None, respond_with(&BIG));
  let id = h.open(Method::GET, "/", &[]).unwrap();
  h.pump();
  let first = h.data_len(id);
  assert_eq!(first, 65_535, "initial window");
  let mut total = first;
  let mut released = 0;
  for _ in 0..16 {
    let pending = total - released;
    h.engine.consumed(id, pending, &mut h.out);
    released += pending;
    h.pump();
    total = h.data_len(id);
    if total == SIZE {
      break;
    }
  }
  assert_eq!(total, SIZE);
  assert_eq!(h.of(id), vec![Seen::Head(200), Seen::Data(SIZE), Seen::End]);
}

#[test]
fn trailers_arrive_before_end() {
  let handler: Handler = Rc::new(|_req, mut respond, _sp| {
    let mut send = respond.send_response(Response::new(()), false).unwrap();
    send.send_data(Bytes::from_static(b"hi"), false).unwrap();
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("0"));
    send.send_trailers(trailers).unwrap();
  });
  let mut h = Harness::new(None, handler);
  let id = h.open(Method::GET, "/", &[]).unwrap();
  h.pump();
  assert_eq!(h.of(id), vec![Seen::Head(200), Seen::Data(2), Seen::Trailers(vec![("grpc-status".into(), "0".into())]), Seen::End]);
}

#[test]
fn reset_cancels_one_exchange_and_keeps_the_connection() {
  let (handler, _held) = silent();
  let mut h = Harness::new(None, handler);
  let slow = h.open(Method::GET, "/slow", &[]).unwrap();
  h.pump();
  h.engine.reset(slow, &mut h.out);
  let fast = h.open(Method::GET, "/fast", &[]).unwrap();
  h.pump();
  assert_eq!(h.of(fast), vec![Seen::Head(200), Seen::Data(4), Seen::End]);
  assert_eq!(h.of(slow), vec![]);
  assert!(!h.closed && !h.conn_closed());
  assert!(matches!(h.engine.capacity(), Capacity::Shared { open: 0, .. }));
}

#[test]
fn goaway_refuses_unprocessed_exchanges() {
  let mut h = Harness::new(None, respond_with(b"hello"));
  let first = h.open(Method::GET, "/", &[]).unwrap();
  h.pump();
  assert_eq!(h.of(first), vec![Seen::Head(200), Seen::Data(5), Seen::End]);
  let unseen = h.open(Method::GET, "/never", &[]).unwrap();
  h.collect();
  let goaway: [u8; 17] = [0, 0, 8, 7, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
  h.input.extend_from_slice(&goaway);
  h.engine.on_bytes(&mut h.input, &mut h.out);
  h.collect();
  assert_eq!(h.of(unseen), vec![Seen::Reset(Error::Refused)]);
  let going = h.conn.iter().position(|c| matches!(c, ConnEvent::GoingAway)).expect("GoingAway reported");
  if let Some(closed) = h.conn.iter().position(|c| matches!(c, ConnEvent::Closed(_))) {
    assert!(going < closed, "GoingAway before Closed");
  }
  assert!(matches!(h.open(Method::GET, "/", &[]), Err(OpenError::GoingAway | OpenError::Closed)));
}

#[test]
fn eof_resets_open_exchanges() {
  let (handler, _held) = silent();
  let mut h = Harness::new(None, handler);
  let slow = h.open(Method::GET, "/slow", &[]).unwrap();
  h.pump();
  h.engine.on_eof(&mut h.out);
  h.collect();
  assert_eq!(h.of(slow), vec![Seen::Reset(Error::Closed)]);
  assert!(h.conn_closed());
}

#[test]
fn engine_has_no_deadlines() {
  let mut h = Harness::new(None, respond_with(b"hello"));
  h.open(Method::GET, "/", &[]).unwrap();
  assert_eq!(h.engine.next_deadline(), None);
}
