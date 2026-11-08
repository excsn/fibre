#![cfg(feature = "http1")]

use bytes::{Bytes, BytesMut};
use fibre_http::engine::{Capacity, ConnEvent, Engine as _, Event, ExchangeId, NetAction, OpenError, Output, RequestHead};
use fibre_http::h1::Engine;
use http::{HeaderMap, HeaderValue, Method, Version};

struct Harness {
  engine: Engine,
  out: Output,
  input: BytesMut,
}

#[derive(Debug, PartialEq)]
enum Seen {
  Head(u16),
  Data(Vec<u8>),
  End,
  Reset,
}

impl Harness {
  fn new() -> Self {
    Self { engine: Engine::new(), out: Output::default(), input: BytesMut::new() }
  }

  fn open(&mut self, method: Method, uri: &str, headers: &[(&str, &str)]) -> Result<ExchangeId, OpenError> {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
      map.append(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
    }
    let head = RequestHead { method, uri: uri.parse().unwrap(), version: Version::HTTP_11, headers: map };
    self.engine.open(head, &mut self.out)
  }

  fn sent(&mut self) -> Vec<u8> {
    let mut bytes = Vec::new();
    self.out.net.retain(|a| match a {
      NetAction::Send(b) => {
        bytes.extend_from_slice(b);
        false
      }
      _ => true,
    });
    bytes
  }

  fn feed(&mut self, bytes: &[u8]) {
    self.input.extend_from_slice(bytes);
    self.engine.on_bytes(&mut self.input, &mut self.out);
  }

  fn eof(&mut self) {
    self.engine.on_eof(&mut self.out);
  }

  fn seen(&mut self, id: ExchangeId) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    for (eid, event) in self.out.events.drain(..) {
      assert_eq!(eid, id, "event for another exchange");
      match event {
        Event::Head(h) => seen.push(Seen::Head(h.status.as_u16())),
        Event::Data(d) => match seen.last_mut() {
          Some(Seen::Data(buf)) => buf.extend_from_slice(&d),
          _ => seen.push(Seen::Data(d.to_vec())),
        },
        Event::End => seen.push(Seen::End),
        Event::Reset(_) => seen.push(Seen::Reset),
        other => panic!("unexpected event {other:?}"),
      }
    }
    seen
  }

  fn closed(&self) -> bool {
    self.out.net.contains(&NetAction::Close) && self.out.conn.iter().any(|c| matches!(c, ConnEvent::Closed(_)))
  }
}

const HELLO: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello";

#[test]
fn get_writes_request_line_and_host() {
  let mut h = Harness::new();
  h.open(Method::GET, "http://example.test/path?q=1", &[("x-probe", "1")]).unwrap();
  assert_eq!(h.sent(), b"GET /path?q=1 HTTP/1.1\r\nhost: example.test\r\nx-probe: 1\r\n\r\n");
}

#[test]
fn response_split_at_every_byte_gives_the_same_events() {
  for split in 0..=HELLO.len() {
    let mut h = Harness::new();
    let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
    h.feed(&HELLO[..split]);
    h.feed(&HELLO[split..]);
    assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End], "split at {split}");
    assert!(h.input.is_empty(), "split at {split} left input");
  }
}

#[test]
fn response_fed_one_byte_at_a_time() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  for b in HELLO {
    h.feed(&[*b]);
  }
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
}

#[test]
fn chunked_body_is_decoded() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello world".to_vec()), Seen::End]);
}

#[test]
fn informational_response_is_skipped_before_the_final_one() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 103 Early Hints\r\nlink: </a.css>\r\n\r\n");
  h.feed(HELLO);
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
}

#[test]
fn response_without_body_ends_after_head() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 204 No Content\r\n\r\n");
  assert_eq!(h.seen(id), vec![Seen::Head(204), Seen::End]);
}

#[test]
fn redirect_is_delivered_like_any_response() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 301 Moved Permanently\r\nlocation: /new\r\ncontent-length: 2\r\n\r\nok");
  assert_eq!(h.seen(id), vec![Seen::Head(301), Seen::Data(b"ok".to_vec()), Seen::End]);
}

#[test]
fn capacity_is_unique_and_returns_after_the_body_ends() {
  let mut h = Harness::new();
  assert_eq!(h.engine.capacity(), Capacity::Unique { available: true });
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  assert_eq!(h.engine.capacity(), Capacity::Unique { available: false });
  assert_eq!(h.open(Method::GET, "http://example.test/", &[]), Err(OpenError::Full));
  h.feed(HELLO);
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
  assert_eq!(h.engine.capacity(), Capacity::Unique { available: true });
  h.sent();
  let second = h.open(Method::GET, "http://example.test/again", &[]).unwrap();
  assert_ne!(second, id);
  assert!(h.sent().starts_with(b"GET /again HTTP/1.1\r\n"));
  h.feed(HELLO);
  assert_eq!(h.seen(second), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
}

#[test]
fn connection_close_response_closes_the_connection() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 5\r\n\r\nhello");
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
  assert!(h.closed());
  assert_eq!(h.engine.capacity(), Capacity::Unique { available: false });
}

#[test]
fn close_delimited_body_ends_at_eof() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 200 OK\r\n\r\nall of ");
  h.feed(b"it");
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"all of it".to_vec())]);
  h.eof();
  assert_eq!(h.seen(id), vec![Seen::End]);
  assert!(h.closed());
}

#[test]
fn eof_mid_response_resets_the_exchange() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nhal");
  h.eof();
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hal".to_vec()), Seen::Reset]);
  assert!(h.closed());
}

#[test]
fn post_with_content_length_sends_the_body() {
  let mut h = Harness::new();
  let id = h.open(Method::POST, "http://example.test/submit", &[("content-length", "5")]).unwrap();
  let head = h.sent();
  assert!(head.starts_with(b"POST /submit HTTP/1.1\r\n"));
  assert_eq!(h.engine.send_body(id, Bytes::from_static(b"hello"), true, &mut h.out), 5);
  assert_eq!(h.sent(), b"hello");
  h.feed(HELLO);
  assert_eq!(h.seen(id), vec![Seen::Head(200), Seen::Data(b"hello".to_vec()), Seen::End]);
}

#[test]
fn post_without_length_sends_a_chunked_body() {
  let mut h = Harness::new();
  let id = h.open(Method::POST, "http://example.test/submit", &[]).unwrap();
  let head = h.sent();
  assert!(String::from_utf8_lossy(&head).contains("transfer-encoding: chunked\r\n"));
  assert_eq!(h.engine.send_body(id, Bytes::from_static(b"hello"), false, &mut h.out), 5);
  assert_eq!(h.engine.send_body(id, Bytes::new(), true, &mut h.out), 0);
  assert_eq!(h.sent(), b"5\r\nhello\r\n0\r\n\r\n");
}

#[test]
fn reset_mid_response_closes_the_connection() {
  let mut h = Harness::new();
  let id = h.open(Method::GET, "http://example.test/", &[]).unwrap();
  h.feed(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nhal");
  h.seen(id);
  h.engine.reset(id, &mut h.out);
  assert!(h.closed());
  assert_eq!(h.engine.capacity(), Capacity::Unique { available: false });
}

#[test]
fn engine_has_no_deadlines() {
  let mut h = Harness::new();
  h.open(Method::GET, "http://example.test/", &[]).unwrap();
  assert_eq!(h.engine.next_deadline(), None);
}
