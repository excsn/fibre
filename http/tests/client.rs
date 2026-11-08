use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use fibre_http::{AsyncClient, Error, Pool, SyncClient};
use http::{Response, Version};

#[derive(Clone, Copy, PartialEq)]
enum H1Mode {
  KeepAlive,
  /// Closes every connection after one response without saying so.
  SilentClose,
}

struct H1Server {
  addr: SocketAddr,
  accepted: Arc<AtomicUsize>,
}

fn h1_server(mode: H1Mode) -> H1Server {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  let accepted = Arc::new(AtomicUsize::new(0));
  let count = accepted.clone();
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      count.fetch_add(1, Ordering::SeqCst);
      std::thread::spawn(move || serve_h1(stream, mode));
    }
  });
  H1Server { addr, accepted }
}

fn serve_h1(stream: TcpStream, mode: H1Mode) {
  let mut reader = BufReader::new(stream.try_clone().unwrap());
  let mut writer = stream;
  loop {
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
      return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut length = 0usize;
    loop {
      let mut h = String::new();
      if reader.read_line(&mut h).unwrap_or(0) == 0 {
        return;
      }
      if h == "\r\n" {
        break;
      }
      let lower = h.to_ascii_lowercase();
      if let Some(v) = lower.strip_prefix("content-length:") {
        length = v.trim().parse().unwrap();
      }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let close = path == "/close";
    let response: Vec<u8> = match path.as_str() {
      "/echo" => [format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len()).into_bytes(), body].concat(),
      "/chunked" => b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n".to_vec(),
      "/close" => b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 3\r\n\r\nbye".to_vec(),
      "/stall" => {
        std::thread::sleep(Duration::from_secs(5));
        return;
      }
      _ => b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello".to_vec(),
    };
    if writer.write_all(&response).is_err() {
      return;
    }
    if close || mode == H1Mode::SilentClose {
      return;
    }
  }
}

fn url(addr: SocketAddr, path: &str) -> String {
  format!("http://{addr}{path}")
}

#[test]
fn sync_get_h1() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = SyncClient::new();
  let r = client.get(url(s.addr, "/")).send().unwrap();
  assert_eq!(r.status(), 200);
  assert_eq!(r.version(), Version::HTTP_11);
  assert_eq!(r.text().unwrap(), "hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_get_h1() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = AsyncClient::new();
  let r = client.get(url(s.addr, "/chunked")).send().await.unwrap();
  assert_eq!(r.text().await.unwrap(), "hello world");
}

#[test]
fn keep_alive_reuses_one_connection() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = SyncClient::new();
  for _ in 0..20 {
    assert_eq!(client.get(url(s.addr, "/")).send().unwrap().text().unwrap(), "hello");
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn post_body_is_echoed() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = SyncClient::new();
  let body = vec![b'x'; 100_000];
  let r = client.post(url(s.addr, "/echo")).body(body.clone()).send().unwrap();
  assert_eq!(r.bytes().unwrap(), Bytes::from(body));
}

#[test]
fn connection_close_response_opens_a_new_connection() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = SyncClient::new();
  assert_eq!(client.get(url(s.addr, "/close")).send().unwrap().text().unwrap(), "bye");
  assert_eq!(client.get(url(s.addr, "/")).send().unwrap().text().unwrap(), "hello");
  assert_eq!(s.accepted.load(Ordering::SeqCst), 2);
}

#[test]
fn silently_closed_idle_connection_is_found_by_peek() {
  let s = h1_server(H1Mode::SilentClose);
  let pool = Pool::builder().peek_after(Duration::ZERO).build();
  let client = SyncClient::builder().pool(&pool).build_sync().unwrap();
  for _ in 0..3 {
    assert_eq!(client.get(url(s.addr, "/")).send().unwrap().text().unwrap(), "hello");
    std::thread::sleep(Duration::from_millis(20));
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 3);
}

#[test]
fn silently_closed_idle_connection_is_retried_without_peek() {
  let s = h1_server(H1Mode::SilentClose);
  let pool = Pool::builder().peek_after(Duration::from_secs(3600)).build();
  let client = SyncClient::builder().pool(&pool).build_sync().unwrap();
  for _ in 0..3 {
    assert_eq!(client.get(url(s.addr, "/")).send().unwrap().text().unwrap(), "hello");
    std::thread::sleep(Duration::from_millis(20));
  }
}

#[test]
fn timeout_ends_a_stalled_request() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = SyncClient::new();
  let started = Instant::now();
  let r = client.get(url(s.addr, "/stall")).timeout(Duration::from_millis(300)).send();
  assert!(matches!(r, Err(Error::Timeout)), "{:?}", r.err());
  assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_and_async_clients_share_one_pool() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = AsyncClient::new();
  assert_eq!(client.get(url(s.addr, "/")).send().await.unwrap().text().await.unwrap(), "hello");
  let blocking = client.clone().to_sync();
  let target = url(s.addr, "/");
  let text = tokio::task::spawn_blocking(move || blocking.get(target).send().unwrap().text().unwrap()).await.unwrap();
  assert_eq!(text, "hello");
  assert_eq!(client.get(url(s.addr, "/")).send().await.unwrap().text().await.unwrap(), "hello");
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_body_reads_on_a_blocking_thread() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = AsyncClient::new();
  let body = vec![b'y'; 50_000];
  let r = client.post(url(s.addr, "/echo")).body(body.clone()).send().await.unwrap();
  let r = r.to_sync();
  let got = tokio::task::spawn_blocking(move || {
    let mut out = Vec::new();
    r.into_body().read_to_end(&mut out).unwrap();
    out
  })
  .await
  .unwrap();
  assert_eq!(got, body);
}

mod h2c {
  use super::*;

  pub struct H2Server {
    pub addr: SocketAddr,
    pub accepted: Arc<AtomicUsize>,
  }

  pub fn server() -> H2Server {
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
          tokio::spawn(serve(sock));
        }
      });
    });
    H2Server { addr, accepted }
  }

  pub async fn serve<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static>(io: T) {
    let Ok(mut conn) = h2::server::handshake(io).await else { return };
    while let Some(Ok((req, mut respond))) = conn.accept().await {
      tokio::spawn(async move {
        let path = req.uri().path().to_string();
        let mut body = req.into_body();
        let mut got = Vec::new();
        while let Some(Ok(chunk)) = body.data().await {
          let _ = body.flow_control().release_capacity(chunk.len());
          got.extend_from_slice(&chunk);
        }
        if path == "/slow" {
          tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let reply: Bytes = match path.as_str() {
          "/echo" => Bytes::from(got),
          "/big" => Bytes::from(vec![7u8; 200_000]),
          _ => Bytes::from_static(b"hello"),
        };
        let mut send = respond.send_response(Response::new(()), false).unwrap();
        let _ = send.send_data(reply, true);
      });
    }
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2c_concurrent_async_requests_share_one_connection() {
  let s = h2c::server();
  let client = AsyncClient::builder().http2_prior_knowledge().build_async().unwrap();
  let mut tasks = Vec::new();
  for _ in 0..64 {
    let c = client.clone();
    let u = url(s.addr, "/");
    tasks.push(tokio::spawn(async move {
      let r = c.get(u).send().await.unwrap();
      assert_eq!(r.version(), Version::HTTP_2);
      r.text().await.unwrap()
    }));
  }
  for t in tasks {
    assert_eq!(t.await.unwrap(), "hello");
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn h2c_threads_and_tasks_share_one_connection() {
  let s = h2c::server();
  let pool = Pool::default();
  let sync = SyncClient::builder().pool(&pool).http2_prior_knowledge().build_sync().unwrap();
  let warm = sync.get(url(s.addr, "/")).send().unwrap().text().unwrap();
  assert_eq!(warm, "hello");
  let mut threads = Vec::new();
  for _ in 0..4 {
    let c = sync.clone();
    let u = url(s.addr, "/echo");
    threads.push(std::thread::spawn(move || {
      for i in 0..25 {
        let body = format!("thread {i}");
        assert_eq!(c.post(&u).body(body.clone()).send().unwrap().text().unwrap(), body);
      }
    }));
  }
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  let asynch = sync.clone().to_async();
  rt.block_on(async {
    let mut tasks = Vec::new();
    for i in 0..50 {
      let c = asynch.clone();
      let u = url(s.addr, "/echo");
      tasks.push(tokio::spawn(async move {
        let body = format!("task {i}");
        assert_eq!(c.post(u).body(body.clone()).send().await.unwrap().text().await.unwrap(), body);
      }));
    }
    for t in tasks {
      t.await.unwrap();
    }
  });
  for t in threads {
    t.join().unwrap();
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn h2c_large_body_flows() {
  let s = h2c::server();
  let client = SyncClient::builder().http2_prior_knowledge().build_sync().unwrap();
  let got = client.get(url(s.addr, "/big")).send().unwrap().bytes().unwrap();
  assert_eq!(got.len(), 200_000);
  let up = vec![3u8; 300_000];
  let echoed = client.post(url(s.addr, "/echo")).body(up.clone()).send().unwrap().bytes().unwrap();
  assert_eq!(echoed.len(), up.len());
}

mod tls {
  use super::*;
  use tokio_rustls::rustls;

  pub struct TlsServer {
    pub addr: SocketAddr,
    pub root: Vec<u8>,
  }

  pub fn server(alpn: &[&[u8]]) -> TlsServer {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let root = cert.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.signing_key.serialize_der()).unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
      .with_safe_default_protocol_versions()
      .unwrap()
      .with_no_client_auth()
      .with_single_cert(vec![cert.cert.der().clone()], key)
      .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
      let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
      rt.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        loop {
          let (sock, _) = listener.accept().await.unwrap();
          let acceptor = acceptor.clone();
          tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(sock).await else { return };
            if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
              h2c::serve(tls).await;
            } else {
              serve_h1_async(tls).await;
            }
          });
        }
      });
    });
    TlsServer { addr, root }
  }

  async fn serve_h1_async<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(io: T) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut io = tokio::io::BufReader::new(io);
    loop {
      let mut line = String::new();
      if io.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
      }
      loop {
        let mut h = String::new();
        if io.read_line(&mut h).await.unwrap_or(0) == 0 || h == "\r\n" {
          break;
        }
      }
      if io.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello").await.is_err() {
        return;
      }
      let _ = io.flush().await;
    }
  }
}

#[test]
fn tls_negotiates_http2() {
  let s = tls::server(&[b"h2", b"http/1.1"]);
  let client = SyncClient::builder().add_root_certificate(s.root.clone()).build_sync().unwrap();
  let r = client.get(format!("https://localhost:{}/", s.addr.port())).send().unwrap();
  assert_eq!(r.version(), Version::HTTP_2);
  assert_eq!(r.text().unwrap(), "hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_falls_back_to_http1() {
  let s = tls::server(&[b"http/1.1"]);
  let client = AsyncClient::builder().add_root_certificate(s.root.clone()).build_async().unwrap();
  for _ in 0..3 {
    let r = client.get(format!("https://localhost:{}/", s.addr.port())).send().await.unwrap();
    assert_eq!(r.version(), Version::HTTP_11);
    assert_eq!(r.text().await.unwrap(), "hello");
  }
}

#[test]
fn tls_rejects_an_untrusted_certificate() {
  let s = tls::server(&[b"h2"]);
  let client = SyncClient::new();
  let r = client.get(format!("https://localhost:{}/", s.addr.port())).send();
  assert!(matches!(r, Err(Error::Tls(_))), "{:?}", r.err());
}

#[test]
fn query_pairs_are_form_encoded() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  let seen = std::thread::spawn(move || {
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    loop {
      let mut h = String::new();
      reader.read_line(&mut h).unwrap();
      if h == "\r\n" {
        break;
      }
    }
    let mut w = stream;
    w.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").unwrap();
    line
  });
  let client = SyncClient::new();
  client.get(url(addr, "/p?a=1")).query(&[("q", "a b&c"), ("é", "x/y")]).send().unwrap();
  assert_eq!(seen.join().unwrap(), "GET /p?a=1&q=a+b%26c&%C3%A9=x%2Fy HTTP/1.1\r\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_async_leader_hands_the_connection_to_a_parked_request() {
  let s = h2c::server();
  let client = AsyncClient::builder().http2_prior_knowledge().build_async().unwrap();
  assert_eq!(client.get(url(s.addr, "/")).send().await.unwrap().text().await.unwrap(), "hello");
  for _ in 0..20 {
    let slow = client.clone();
    let slow_url = url(s.addr, "/slow");
    let leader = tokio::spawn(async move { slow.get(slow_url).send().await.map(|r| r.status()) });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let fast = client.clone();
    let fast_url = url(s.addr, "/echo");
    let follower = tokio::spawn(async move { fast.post(fast_url).body("still here").send().await?.text().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    leader.abort();
    let text = tokio::time::timeout(Duration::from_secs(5), follower).await.expect("the follower finished").unwrap().unwrap();
    assert_eq!(text, "still here");
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn a_timed_out_async_leader_leaves_a_blocking_follower_working() {
  let s = h2c::server();
  let pool = Pool::default();
  let sync = SyncClient::builder().pool(&pool).http2_prior_knowledge().build_sync().unwrap();
  assert_eq!(sync.get(url(s.addr, "/")).send().unwrap().text().unwrap(), "hello");
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  let asynch = sync.clone().to_async();
  for _ in 0..10 {
    let slow_url = url(s.addr, "/slow");
    let a = asynch.clone();
    let leader = rt.spawn(async move { tokio::time::timeout(Duration::from_millis(60), a.get(slow_url).send()).await.is_err() });
    std::thread::sleep(Duration::from_millis(20));
    let text = sync.post(url(s.addr, "/echo")).body("blocking").timeout(Duration::from_secs(5)).send().unwrap().text().unwrap();
    assert_eq!(text, "blocking");
    assert!(rt.block_on(leader).unwrap(), "the async request timed out");
  }
  assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_http1_request_does_not_poison_the_pool() {
  let s = h1_server(H1Mode::KeepAlive);
  let client = AsyncClient::new();
  for _ in 0..5 {
    let c = client.clone();
    let u = url(s.addr, "/stall");
    let stalled = tokio::spawn(async move { c.get(u).send().await.map(|r| r.status()) });
    tokio::time::sleep(Duration::from_millis(30)).await;
    stalled.abort();
    let r = tokio::time::timeout(Duration::from_secs(5), client.get(url(s.addr, "/")).send()).await.expect("not stuck behind the dropped request").unwrap();
    assert_eq!(r.text().await.unwrap(), "hello");
  }
}
