//! Client contracts from the reqwest and hyper suites, against servers that record what they receive.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use fibre_http::{AsyncClient, Error, Pool, SyncClient};
use http::{HeaderMap, HeaderValue};

#[derive(Clone, Debug)]
struct Seen {
  line: String,
  headers: Vec<(String, String)>,
  body: Vec<u8>,
}

impl Seen {
  fn header(&self, name: &str) -> Option<&str> {
    self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
  }
}

type Reply = Arc<dyn Fn(&Seen, SocketAddr) -> Vec<u8> + Send + Sync>;

struct Server {
  addr: SocketAddr,
  seen: Arc<Mutex<Vec<Seen>>>,
  accepted: Arc<AtomicUsize>,
}

impl Server {
  fn url(&self, path: &str) -> String {
    format!("http://{}{path}", self.addr)
  }

  fn last(&self) -> Seen {
    self.seen.lock().unwrap().last().cloned().expect("a request")
  }
}

fn ok(body: &str) -> Vec<u8> {
  format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn server_on(listener: TcpListener, reply: Reply) -> Server {
  let addr = listener.local_addr().unwrap();
  let seen = Arc::new(Mutex::new(Vec::new()));
  let accepted = Arc::new(AtomicUsize::new(0));
  let (record, count) = (seen.clone(), accepted.clone());
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      count.fetch_add(1, Ordering::SeqCst);
      let (record, reply) = (record.clone(), reply.clone());
      std::thread::spawn(move || serve(stream, record, reply, addr));
    }
  });
  Server { addr, seen, accepted }
}

fn server(reply: impl Fn(&Seen, SocketAddr) -> Vec<u8> + Send + Sync + 'static) -> Server {
  server_on(TcpListener::bind("127.0.0.1:0").unwrap(), Arc::new(reply))
}

fn serve(stream: TcpStream, record: Arc<Mutex<Vec<Seen>>>, reply: Reply, addr: SocketAddr) {
  let mut reader = BufReader::new(stream.try_clone().unwrap());
  let mut w = stream;
  loop {
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
      return;
    }
    let mut headers = Vec::new();
    loop {
      let mut h = String::new();
      if reader.read_line(&mut h).unwrap_or(0) == 0 {
        return;
      }
      if h == "\r\n" {
        break;
      }
      if let Some((k, v)) = h.trim_end().split_once(':') {
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
      }
    }
    let length: usize = headers.iter().find(|(k, _)| k == "content-length").map(|(_, v)| v.parse().unwrap()).unwrap_or(0);
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
      return;
    }
    let seen = Seen { line: line.trim_end().to_string(), headers, body };
    record.lock().unwrap().push(seen.clone());
    let out = reply(&seen, addr);
    if out.is_empty() {
      std::thread::sleep(Duration::from_secs(5));
      return;
    }
    if w.write_all(&out).is_err() {
      return;
    }
  }
}

#[test]
fn a_get_without_a_body_sends_no_framing_headers() {
  let s = server(|_, _| ok("x"));
  SyncClient::new().get(s.url("/")).send().unwrap().text().unwrap();
  let seen = s.last();
  assert!(seen.header("content-length").is_none(), "{seen:?}");
  assert!(seen.header("transfer-encoding").is_none(), "{seen:?}");
  assert!(seen.header("content-type").is_none(), "{seen:?}");
}

#[test]
fn an_empty_post_sends_content_length_zero() {
  let s = server(|_, _| ok("x"));
  SyncClient::new().post(s.url("/")).send().unwrap().text().unwrap();
  assert_eq!(s.last().header("content-length"), Some("0"));
}

#[test]
fn the_fragment_is_not_sent_and_an_empty_path_is_slash() {
  let s = server(|_, _| ok("x"));
  let client = SyncClient::new();
  client.get(format!("http://{}/a?b=1#frag", s.addr)).send().unwrap().text().unwrap();
  assert_eq!(s.last().line, "GET /a?b=1 HTTP/1.1");
  client.get(format!("http://{}", s.addr)).send().unwrap().text().unwrap();
  assert_eq!(s.last().line, "GET / HTTP/1.1");
}

#[test]
fn a_form_body_is_urlencoded_with_its_content_type() {
  let s = server(|_, _| ok("x"));
  SyncClient::new().post(s.url("/")).form(&[("a", "1 2"), ("b", "x&y")]).send().unwrap().text().unwrap();
  let seen = s.last();
  assert_eq!(seen.header("content-type"), Some("application/x-www-form-urlencoded"));
  assert_eq!(seen.body, b"a=1+2&b=x%26y");
}

#[cfg(feature = "json")]
#[test]
fn json_sets_its_content_type_only_when_none_is_set() {
  let s = server(|_, _| ok("x"));
  let client = SyncClient::new();
  client.post(s.url("/")).json(&[1, 2]).send().unwrap().text().unwrap();
  assert_eq!(s.last().header("content-type"), Some("application/json"));
  client.post(s.url("/")).header("content-type", "application/vnd.x+json").json(&[1]).send().unwrap().text().unwrap();
  assert_eq!(s.last().header("content-type"), Some("application/vnd.x+json"));
}

#[test]
fn default_headers_are_sent_and_a_request_header_replaces_them() {
  let s = server(|_, _| ok("x"));
  let mut defaults = HeaderMap::new();
  defaults.insert("x-team", HeaderValue::from_static("core"));
  defaults.insert("x-trace", HeaderValue::from_static("default"));
  let client = SyncClient::builder().default_headers(defaults).build_sync().unwrap();
  client.get(s.url("/")).header("x-trace", "mine").send().unwrap().text().unwrap();
  let seen = s.last();
  assert_eq!(seen.header("x-team"), Some("core"));
  assert_eq!(seen.headers.iter().filter(|(k, _)| k == "x-trace").map(|(_, v)| v.as_str()).collect::<Vec<_>>(), ["mine"]);
}

#[test]
fn error_for_status_turns_4xx_and_5xx_into_errors() {
  let s = server(|seen, _| match seen.line.split_whitespace().nth(1) {
    Some("/missing") => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec(),
    Some("/broken") => b"HTTP/1.1 503 Busy\r\ncontent-length: 0\r\n\r\n".to_vec(),
    _ => ok("fine"),
  });
  let client = SyncClient::new();
  assert!(matches!(client.get(s.url("/missing")).send().unwrap().error_for_status(), Err(Error::Status(404))));
  assert!(matches!(client.get(s.url("/broken")).send().unwrap().error_for_status(), Err(Error::Status(503))));
  assert_eq!(client.get(s.url("/")).send().unwrap().error_for_status().unwrap().text().unwrap(), "fine");
}

#[test]
fn a_redirect_to_get_drops_the_body_headers() {
  let s = server(|seen, _| match seen.line.split_whitespace().nth(1) {
    Some("/form") => b"HTTP/1.1 303 See Other\r\nlocation: /done\r\ncontent-length: 0\r\n\r\n".to_vec(),
    _ => ok("done"),
  });
  let r = SyncClient::new().post(s.url("/form")).header("content-type", "text/plain").header("content-encoding", "identity").body("data").send().unwrap();
  assert_eq!(r.text().unwrap(), "done");
  let seen = s.last();
  assert_eq!(seen.line, "GET /done HTTP/1.1");
  for name in ["content-type", "content-length", "content-encoding"] {
    assert!(seen.header(name).is_none(), "{name} in {seen:?}");
  }
}

#[test]
fn a_cross_origin_redirect_drops_credentials() {
  let target = server(|_, _| ok("there"));
  let target_url = target.url("/landing");
  let origin = server(move |seen, _| match seen.line.split_whitespace().nth(1) {
    Some("/same") => b"HTTP/1.1 302 Found\r\nlocation: /landing\r\ncontent-length: 0\r\n\r\n".to_vec(),
    Some("/away") => format!("HTTP/1.1 302 Found\r\nlocation: {target_url}\r\ncontent-length: 0\r\n\r\n").into_bytes(),
    _ => ok("here"),
  });
  let client = SyncClient::new();
  client.get(origin.url("/same")).header("authorization", "Bearer t").header("cookie", "a=1").send().unwrap().text().unwrap();
  let same = origin.last();
  assert_eq!(same.header("authorization"), Some("Bearer t"));
  assert_eq!(same.header("cookie"), Some("a=1"));
  assert_eq!(client.get(origin.url("/away")).header("authorization", "Bearer t").header("cookie", "a=1").send().unwrap().text().unwrap(), "there");
  let away = target.last();
  assert!(away.header("authorization").is_none(), "{away:?}");
  assert!(away.header("cookie").is_none(), "{away:?}");
}

#[test]
fn an_unparseable_location_returns_the_redirect() {
  let s = server(|_, _| b"HTTP/1.1 302 Found\r\nlocation: http://[::1\r\ncontent-length: 2\r\n\r\nno".to_vec());
  let r = SyncClient::new().get(s.url("/")).send().unwrap();
  assert_eq!(r.status(), 302);
  assert_eq!(r.text().unwrap(), "no");
}

#[test]
fn an_idle_connection_past_its_timeout_is_not_reused() {
  let s = server(|_, _| ok("x"));
  let pool = Pool::builder().idle_timeout(Duration::from_millis(50)).build();
  let client = SyncClient::builder().pool(&pool).build_sync().unwrap();
  client.get(s.url("/")).send().unwrap().text().unwrap();
  client.get(s.url("/")).send().unwrap().text().unwrap();
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
  std::thread::sleep(Duration::from_millis(120));
  client.get(s.url("/")).send().unwrap().text().unwrap();
  assert_eq!(s.accepted.load(Ordering::SeqCst), 2);
}

#[test]
fn every_resolved_address_is_tried() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let port = listener.local_addr().unwrap().port();
  if TcpListener::bind(("::1", port)).is_err() {
    return;
  }
  let s = server_on(listener, Arc::new(|_, _| ok("found")));
  let addrs: Vec<_> = std::net::ToSocketAddrs::to_socket_addrs(&("localhost", port)).unwrap().collect();
  let r = SyncClient::new().get(format!("http://localhost:{port}/")).send().unwrap();
  assert_eq!(r.text().unwrap(), "found", "{addrs:?}");
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn the_deadline_covers_a_stalled_body() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  std::thread::spawn(move || {
    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf);
    stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc").unwrap();
    std::thread::sleep(Duration::from_secs(5));
  });
  let started = Instant::now();
  let r = SyncClient::new().get(format!("http://{addr}/")).timeout(Duration::from_millis(300)).send().unwrap();
  assert!(matches!(r.bytes(), Err(Error::Timeout)));
  assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn the_deadline_covers_queueing_for_a_connection() {
  let s = server(|seen, _| match seen.line.split_whitespace().nth(1) {
    Some("/hold") => Vec::new(),
    _ => ok("x"),
  });
  let pool = Pool::builder().max_per_host(1).build();
  let client = SyncClient::builder().pool(&pool).build_sync().unwrap();
  let holder = client.clone();
  let hold = s.url("/hold");
  std::thread::spawn(move || {
    let _ = holder.get(hold).timeout(Duration::from_secs(3)).send();
  });
  std::thread::sleep(Duration::from_millis(100));
  let started = Instant::now();
  let r = client.get(s.url("/")).timeout(Duration::from_millis(300)).send();
  assert!(matches!(r, Err(Error::Timeout)), "{:?}", r.err());
  assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn the_deadline_covers_a_request_body_the_server_does_not_read() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  std::thread::spawn(move || {
    let held: Vec<_> = listener.incoming().take(1).collect();
    std::thread::sleep(Duration::from_secs(5));
    drop(held);
  });
  let started = Instant::now();
  let r = SyncClient::new().post(format!("http://{addr}/")).body(vec![0u8; 64 * 1024 * 1024]).timeout(Duration::from_millis(500)).send();
  assert!(matches!(r, Err(Error::Timeout)), "{:?}", r.err());
  assert!(started.elapsed() < Duration::from_secs(3));
}

mod h2c {
  use super::*;

  /// An h2c server that answers `/slow` after 100 ms and sends GOAWAY after `goaway_after` requests on a connection.
  pub fn server(max_streams: Option<u32>, goaway_after: Option<usize>) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    std::thread::spawn(move || {
      let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
      rt.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        loop {
          let (sock, _) = listener.accept().await.unwrap();
          count.fetch_add(1, Ordering::SeqCst);
          tokio::spawn(async move {
            let mut builder = h2::server::Builder::new();
            if let Some(n) = max_streams {
              builder.max_concurrent_streams(n);
            }
            let Ok(mut conn) = builder.handshake::<_, Bytes>(sock).await else { return };
            let mut served = 0;
            while let Some(Ok((req, mut respond))) = conn.accept().await {
              served += 1;
              tokio::spawn(async move {
                if req.uri().path() == "/slow" {
                  tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let mut send = respond.send_response(http::Response::new(()), false).unwrap();
                let _ = send.send_data(Bytes::from_static(b"ok"), true);
              });
              if goaway_after == Some(served) {
                conn.graceful_shutdown();
              }
            }
          });
        }
      });
    });
    (addr, accepted)
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_a_goaway_requests_go_to_a_new_connection() {
  let (addr, accepted) = h2c::server(None, Some(2));
  let client = AsyncClient::builder().http2_prior_knowledge().build_async().unwrap();
  for _ in 0..6 {
    assert_eq!(client.get(format!("http://{addr}/")).send().await.unwrap().text().await.unwrap(), "ok");
  }
  assert_eq!(accepted.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_requests_through_a_slow_one_stream_connection() {
  let (addr, accepted) = h2c::server(Some(1), None);
  let pool = Pool::builder().max_per_host(1).build();
  let client = AsyncClient::builder().pool(&pool).http2_prior_knowledge().build_async().unwrap();
  let mut tasks = Vec::new();
  for i in 0..100 {
    let c = client.clone();
    let path = if i % 10 == 0 { "/slow" } else { "/" };
    let u = format!("http://{addr}{path}");
    tasks.push(tokio::spawn(async move { c.get(u).send().await.unwrap().text().await.unwrap() }));
  }
  for t in tasks {
    assert_eq!(t.await.unwrap(), "ok");
  }
  assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn a_reader_body_of_unknown_length_is_sent_chunked() {
  let s = server(|seen, _| ok(&format!("{}", seen.body.len())));
  let data = vec![b'z'; 300_000];
  let reader = std::io::Cursor::new(data.clone());
  let listener_body = SyncClient::new().post(s.url("/up")).body_reader(reader, None).send().unwrap().text().unwrap();
  let _ = listener_body;
  let seen = s.last();
  assert_eq!(seen.header("transfer-encoding"), Some("chunked"));
  assert!(seen.header("content-length").is_none());
}

#[test]
fn a_reader_body_of_known_length_is_sent_sized() {
  let s = server(|seen, _| ok(&format!("{}", seen.body.len())));
  let data = vec![b'q'; 200_000];
  let r = SyncClient::new().post(s.url("/up")).body_reader(std::io::Cursor::new(data.clone()), Some(data.len() as u64)).send().unwrap();
  assert_eq!(r.text().unwrap(), "200000");
  assert_eq!(s.last().header("content-length"), Some("200000"));
  assert_eq!(s.last().body, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_body_is_sent() {
  let s = server(|seen, _| ok(&format!("{}", seen.body.len())));
  let chunks: Vec<Result<Bytes, std::io::Error>> = (0..50).map(|i| Ok(Bytes::from(vec![b'0' + (i % 10) as u8; 1000]))).collect();
  let stream = futures_util::stream::iter(chunks);
  let r = AsyncClient::new().post(s.url("/up")).body_stream(stream, Some(50_000)).send().await.unwrap();
  assert_eq!(r.text().await.unwrap(), "50000");
}

#[test]
fn a_failing_reader_aborts_the_request() {
  struct Broken(usize);
  impl Read for Broken {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
      if self.0 == 0 {
        return Err(std::io::Error::other("disk gone"));
      }
      self.0 -= 1;
      buf[..10].fill(b'x');
      Ok(10)
    }
  }
  let s = server(|_, _| ok("x"));
  let r = SyncClient::new().post(s.url("/up")).body_reader(Broken(3), Some(1000)).send();
  assert!(matches!(r, Err(Error::Io(_))), "{:?}", r.err());
}

#[test]
fn a_307_with_a_streamed_body_returns_the_redirect() {
  let s = server(|seen, _| match seen.line.split_whitespace().nth(1) {
    Some("/move") => b"HTTP/1.1 307 Temporary Redirect\r\nlocation: /up\r\ncontent-length: 0\r\n\r\n".to_vec(),
    _ => ok("landed"),
  });
  let r = SyncClient::new().post(s.url("/move")).body_reader(std::io::Cursor::new(b"once".to_vec()), Some(4)).send().unwrap();
  assert_eq!(r.status(), 307);
}

#[test]
fn a_body_over_the_size_limit_is_an_error() {
  let s = server(|_, _| ok(&"y".repeat(10_000)));
  let client = SyncClient::builder().max_response_size(4096).build_sync().unwrap();
  assert!(matches!(client.get(s.url("/")).send().unwrap().bytes(), Err(Error::TooLarge)));
  let client = SyncClient::builder().max_response_size(20_000).build_sync().unwrap();
  assert_eq!(client.get(s.url("/")).send().unwrap().bytes().unwrap().len(), 10_000);
}
