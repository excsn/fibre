#![cfg(feature = "http3")]

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};
use fibre_http::{AsyncClient, Pool, SyncClient};
use http::Version;

const BIG: usize = 4 * 1024 * 1024;

struct Cert {
  der: Vec<u8>,
  key: Vec<u8>,
}

fn cert() -> Cert {
  let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
  Cert { der: ck.cert.der().to_vec(), key: ck.signing_key.serialize_der() }
}

fn server_tls(cert: &Cert, alpn: &[u8]) -> rustls::ServerConfig {
  let key = rustls::pki_types::PrivateKeyDer::try_from(cert.key.clone()).unwrap();
  let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![rustls::pki_types::CertificateDer::from(cert.der.clone())], key)
    .unwrap();
  tls.alpn_protocols = vec![alpn.to_vec()];
  tls
}

/// An HTTP/3 server on quinn and h3. `GET /` answers `hello`, `POST /echo` echoes the body and `GET /big` sends `BIG` bytes. `/early` answers 405 without reading the request and keeps the stream open for 30 s.
fn h3_server(cert: &Cert) -> SocketAddr {
  let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(server_tls(cert, b"h3")).unwrap();
  let config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
  let (tx, rx) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(async move {
      let ep = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
      tx.send(ep.local_addr().unwrap()).unwrap();
      while let Some(incoming) = ep.accept().await {
        tokio::spawn(async move {
          let Ok(conn) = incoming.await else { return };
          let Ok(mut h3c) = h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await else { return };
          while let Ok(Some(resolver)) = h3c.accept().await {
            tokio::spawn(async move {
              let Ok((req, mut stream)) = resolver.resolve_request().await else { return };
              if req.uri().path() == "/early" {
                let resp = http::Response::builder().status(405).header("content-length", 0).body(()).unwrap();
                if stream.send_response(resp).await.is_ok() {
                  let _ = stream.finish().await;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(stream);
                return;
              }
              let mut body = Vec::new();
              while let Ok(Some(mut chunk)) = stream.recv_data().await {
                while chunk.has_remaining() {
                  let c = chunk.chunk();
                  body.extend_from_slice(c);
                  let n = c.len();
                  chunk.advance(n);
                }
              }
              let reply = match req.uri().path() {
                "/echo" => Bytes::from(body),
                "/big" => Bytes::from(vec![b'x'; BIG]),
                _ => Bytes::from_static(b"hello"),
              };
              let resp = http::Response::builder().status(200).header("content-length", reply.len()).body(()).unwrap();
              if stream.send_response(resp).await.is_err() {
                return;
              }
              if stream.send_data(reply).await.is_err() {
                return;
              }
              let _ = stream.finish().await;
            });
          }
        });
      }
    });
  });
  rx.recv().unwrap()
}

/// HTTP/1.1 over TLS answering `hello` with `alt-svc: <alt_svc>`.
fn tcp_server(cert: &Cert, alt_svc: String) -> SocketAddr {
  let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_tls(cert, b"http/1.1")));
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
        let alt_svc = alt_svc.clone();
        tokio::spawn(async move {
          use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
          let Ok(tls) = acceptor.accept(sock).await else { return };
          let mut io = tokio::io::BufReader::new(tls);
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
            let reply = format!("HTTP/1.1 200 OK\r\nalt-svc: {alt_svc}\r\ncontent-length: 5\r\n\r\nhello");
            if io.write_all(reply.as_bytes()).await.is_err() {
              return;
            }
            let _ = io.flush().await;
          }
        });
      }
    });
  });
  addr
}

fn url(addr: SocketAddr, path: &str) -> String {
  format!("https://localhost:{}{path}", addr.port())
}

/// A UDP port with nothing listening on it.
fn dead_udp_port() -> u16 {
  UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn prior_knowledge_speaks_http3() {
  let c = cert();
  let addr = h3_server(&c);
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_sync().unwrap();
  for _ in 0..3 {
    let r = client.get(url(addr, "/")).send().unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.version(), Version::HTTP_3);
    assert_eq!(r.text().unwrap(), "hello");
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prior_knowledge_speaks_http3_async() {
  let c = cert();
  let addr = h3_server(&c);
  let client = AsyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_async().unwrap();
  for _ in 0..3 {
    let r = client.get(url(addr, "/")).send().await.unwrap();
    assert_eq!(r.version(), Version::HTTP_3);
    assert_eq!(r.text().await.unwrap(), "hello");
  }
}

#[test]
fn a_request_body_larger_than_the_stream_window_is_echoed() {
  let c = cert();
  let addr = h3_server(&c);
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(20)).build_sync().unwrap();
  let body: Vec<u8> = (0..BIG).map(|i| (i % 251) as u8).collect();
  let r = client.post(url(addr, "/echo")).body(body.clone()).send().unwrap();
  assert_eq!(r.version(), Version::HTTP_3);
  assert_eq!(r.bytes().unwrap().as_ref(), body.as_slice());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamed_request_body_is_echoed_async() {
  let c = cert();
  let addr = h3_server(&c);
  let client = AsyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(20)).build_async().unwrap();
  let chunks: Vec<Result<Bytes, std::io::Error>> = (0..64).map(|i| Ok(Bytes::from(vec![i as u8; 16 * 1024]))).collect();
  let expected: Vec<u8> = (0..64).flat_map(|i| vec![i as u8; 16 * 1024]).collect();
  let r = client.post(url(addr, "/echo")).body_stream(futures_util::stream::iter(chunks), None).send().await.unwrap();
  assert_eq!(r.bytes().await.unwrap().as_ref(), expected.as_slice());
}

/// A server answers before reading the request body and never asks for the rest. The request ends with that answer and the connection keeps serving.
#[test]
fn a_response_before_the_whole_body_ends_the_upload() {
  let c = cert();
  let addr = h3_server(&c);
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_sync().unwrap();
  let (tx, rx) = std::sync::mpsc::channel();
  let sender = client.clone();
  std::thread::spawn(move || {
    let r = sender.post(url(addr, "/early")).body(vec![1u8; BIG]).send().map(|r| r.status().as_u16());
    let _ = tx.send(r);
  });
  let r = rx.recv_timeout(Duration::from_secs(15)).expect("the request returned");
  assert_eq!(r, Ok(405));
  assert_eq!(client.get(url(addr, "/")).send().unwrap().text().unwrap(), "hello");
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn a_response_before_the_whole_body_ends_the_upload_async() {
  let c = cert();
  let addr = h3_server(&c);
  let client = AsyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_async().unwrap();
  let (tx, rx) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let r = rt.block_on(async { client.post(url(addr, "/early")).body(vec![1u8; BIG]).send().await.map(|r| r.status().as_u16()) });
    let _ = tx.send(r);
  });
  let r = rx.recv_timeout(Duration::from_secs(15)).expect("the request returned");
  assert_eq!(r, Ok(405));
}

#[test]
fn a_response_larger_than_the_receive_window_flows() {
  let c = cert();
  let addr = h3_server(&c);
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(20)).build_sync().unwrap();
  let r = client.get(url(addr, "/big")).send().unwrap();
  let body = r.bytes().unwrap();
  assert_eq!(body.len(), BIG);
  assert!(body.iter().all(|b| *b == b'x'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_share_one_http3_connection() {
  let c = cert();
  let addr = h3_server(&c);
  let pool = Pool::default();
  let client = AsyncClient::builder().pool(&pool).add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_async().unwrap();
  client.get(url(addr, "/")).send().await.unwrap().text().await.unwrap();
  let tasks: Vec<_> = (0..32)
    .map(|_| {
      let client = client.clone();
      tokio::spawn(async move { client.get(url(addr, "/")).send().await.unwrap().text().await.unwrap() })
    })
    .collect();
  for t in tasks {
    assert_eq!(t.await.unwrap(), "hello");
  }
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn blocking_threads_and_async_tasks_share_one_http3_connection() {
  let c = cert();
  let addr = h3_server(&c);
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(10)).build_sync().unwrap();
  client.get(url(addr, "/")).send().unwrap().text().unwrap();
  let threads: Vec<_> = (0..4)
    .map(|_| {
      let client = client.clone();
      std::thread::spawn(move || (0..8).map(|_| client.get(url(addr, "/")).send().unwrap().text().unwrap()).collect::<Vec<_>>())
    })
    .collect();
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  let async_client = client.clone().to_async();
  let from_tasks = rt.block_on(async move {
    let tasks: Vec<_> = (0..8)
      .map(|_| {
        let c = async_client.clone();
        tokio::spawn(async move { c.get(url(addr, "/")).send().await.unwrap().text().await.unwrap() })
      })
      .collect();
    let mut out = Vec::new();
    for t in tasks {
      out.push(t.await.unwrap());
    }
    out
  });
  for t in threads {
    assert!(t.join().unwrap().iter().all(|s| s == "hello"));
  }
  assert!(from_tasks.iter().all(|s| s == "hello"));
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn alt_svc_moves_the_next_connection_to_http3() {
  let c = cert();
  let udp = h3_server(&c);
  let tcp = tcp_server(&c, format!("h3=\":{}\"; ma=60", udp.port()));
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).timeout(Duration::from_secs(10)).build_sync().unwrap();
  let first = client.get(url(tcp, "/")).send().unwrap();
  assert_eq!(first.version(), Version::HTTP_11);
  assert_eq!(first.text().unwrap(), "hello");
  for _ in 0..3 {
    let r = client.get(url(tcp, "/")).send().unwrap();
    assert_eq!(r.version(), Version::HTTP_3);
    assert_eq!(r.text().unwrap(), "hello");
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alt_svc_moves_the_next_connection_to_http3_async() {
  let c = cert();
  let udp = h3_server(&c);
  let tcp = tcp_server(&c, format!("h3=\":{}\"; ma=60", udp.port()));
  let client = AsyncClient::builder().add_root_certificate(c.der.clone()).timeout(Duration::from_secs(10)).build_async().unwrap();
  let first = client.get(url(tcp, "/")).send().await.unwrap();
  assert_eq!(first.version(), Version::HTTP_11);
  first.text().await.unwrap();
  let r = client.get(url(tcp, "/")).send().await.unwrap();
  assert_eq!(r.version(), Version::HTTP_3);
  assert_eq!(r.text().await.unwrap(), "hello");
}

#[test]
fn an_unreachable_alt_svc_falls_back_to_tcp_and_is_not_tried_again() {
  let c = cert();
  let tcp = tcp_server(&c, format!("h3=\":{}\"; ma=60", dead_udp_port()));
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).add_root_certificate(c.der.clone()).timeout(Duration::from_secs(10)).build_sync().unwrap();
  for _ in 0..4 {
    let r = client.get(url(tcp, "/")).send().unwrap();
    assert_eq!(r.version(), Version::HTTP_11);
    assert_eq!(r.text().unwrap(), "hello");
  }
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn alt_svc_clear_keeps_the_origin_on_tcp() {
  let c = cert();
  let tcp = tcp_server(&c, "clear".to_string());
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).timeout(Duration::from_secs(10)).build_sync().unwrap();
  for _ in 0..2 {
    assert_eq!(client.get(url(tcp, "/")).send().unwrap().version(), Version::HTTP_11);
  }
}

#[test]
fn alt_svc_off_ignores_the_advertisement() {
  let c = cert();
  let udp = h3_server(&c);
  let tcp = tcp_server(&c, format!("h3=\":{}\"", udp.port()));
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).http3_alt_svc(false).timeout(Duration::from_secs(10)).build_sync().unwrap();
  for _ in 0..2 {
    assert_eq!(client.get(url(tcp, "/")).send().unwrap().version(), Version::HTTP_11);
  }
}

#[test]
fn prior_knowledge_to_a_dead_port_fails_without_falling_back() {
  let c = cert();
  let client = SyncClient::builder().add_root_certificate(c.der.clone()).http3_prior_knowledge().timeout(Duration::from_secs(5)).build_sync().unwrap();
  let started = Instant::now();
  let r = client.get(format!("https://localhost:{}/", dead_udp_port())).send();
  assert!(r.is_err(), "{:?}", r.map(|r| r.version()));
  assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn http3_rejects_an_untrusted_certificate() {
  let c = cert();
  let addr = h3_server(&c);
  let client = SyncClient::builder().http3_prior_knowledge().timeout(Duration::from_secs(5)).build_sync().unwrap();
  assert!(client.get(url(addr, "/")).send().is_err());
}
