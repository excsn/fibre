//! HTTP client throughput: fibre_http against hyper and reqwest on HTTP/1.1 and h2c. On HTTP/3 it runs against the h3 crate on quinn.
//!
//!   cargo bench -p fibre_http --bench client -- [seconds] [clients,...] [protocols,...] [levels,...]
//!   cargo bench -p fibre_http --features http3 --bench client      adds the h3 protocol
//!
//! Clients: fibre-async, fibre-sync, hyper, reqwest-async, reqwest-blocking, quinn. Protocols: h1, h2c, h3. hyper and reqwest run on h1 and h2c, quinn on h3 only.
//!
//! Local servers on their own 4-worker runtime answer every GET with 64 bytes: hyper for HTTP/1.1 and h2c, quinn and h3 for HTTP/3. Each cell runs for a fixed time with N requesters (tokio tasks on an N-worker runtime or N threads for the blocking clients) that send the next request as soon as the last body is read. Prints requests per second and ns per request per requester.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::{Request, Response};

const BODY: &[u8; 64] = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const THREADS: [usize; 3] = [1, 4, 8];
const CLIENTS: [&str; 6] = ["fibre-async", "fibre-sync", "hyper", "reqwest-async", "reqwest-blocking", "quinn"];
#[cfg(feature = "http3")]
const PROTOCOLS: [&str; 3] = ["h1", "h2c", "h3"];
#[cfg(not(feature = "http3"))]
const PROTOCOLS: [&str; 2] = ["h1", "h2c"];

fn server() -> (SocketAddr, SocketAddr) {
  let h1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let h2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
  let addrs = (h1.local_addr().unwrap(), h2.local_addr().unwrap());
  h1.set_nonblocking(true).unwrap();
  h2.set_nonblocking(true).unwrap();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    rt.block_on(async move {
      let svc = hyper::service::service_fn(|_req: Request<hyper::body::Incoming>| async {
        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(BODY))))
      });
      let h1 = tokio::net::TcpListener::from_std(h1).unwrap();
      let h2 = tokio::net::TcpListener::from_std(h2).unwrap();
      tokio::spawn(async move {
        loop {
          let (sock, _) = h2.accept().await.unwrap();
          let _ = sock.set_nodelay(true);
          tokio::spawn(async move {
            let _ = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
              .max_concurrent_streams(1024)
              .serve_connection(hyper_util::rt::TokioIo::new(sock), svc)
              .await;
          });
        }
      });
      loop {
        let (sock, _) = h1.accept().await.unwrap();
        let _ = sock.set_nodelay(true);
        tokio::spawn(async move {
          let _ = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(sock), svc).await;
        });
      }
    });
  });
  addrs
}

#[cfg(feature = "http3")]
mod h3s {
  use std::net::SocketAddr;
  use std::sync::Arc;
  use std::time::Duration;

  use bytes::Bytes;

  pub struct Cert {
    pub der: Vec<u8>,
    key: Vec<u8>,
  }

  pub fn cert() -> Cert {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    Cert { der: ck.cert.der().to_vec(), key: ck.signing_key.serialize_der() }
  }

  fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
  }

  pub fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.max_concurrent_bidi_streams(1024u32.into());
    t.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
    t
  }

  pub fn server(cert: &Cert) -> SocketAddr {
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.key.clone()).unwrap();
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
      .with_protocol_versions(&[&rustls::version::TLS13])
      .unwrap()
      .with_no_client_auth()
      .with_single_cert(vec![rustls::pki_types::CertificateDer::from(cert.der.clone())], key)
      .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
    config.transport_config(Arc::new(transport()));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
      let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
      rt.block_on(async move {
        let ep = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        tx.send(ep.local_addr().unwrap()).unwrap();
        while let Some(incoming) = ep.accept().await {
          tokio::spawn(async move {
            let Ok(conn) = incoming.await else { return };
            let Ok(mut h3c) = h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await else { return };
            while let Ok(Some(resolver)) = h3c.accept().await {
              tokio::spawn(async move {
                let Ok((_req, mut stream)) = resolver.resolve_request().await else { return };
                let resp = http::Response::builder().status(200).header("content-length", super::BODY.len()).body(()).unwrap();
                if stream.send_response(resp).await.is_err() {
                  return;
                }
                if stream.send_data(Bytes::from_static(super::BODY)).await.is_err() {
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

  pub fn client(cert: &Cert) -> quinn::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(cert.der.clone())).unwrap();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
      .with_protocol_versions(&[&rustls::version::TLS13])
      .unwrap()
      .with_root_certificates(roots)
      .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap()));
    cfg.transport_config(Arc::new(transport()));
    cfg
  }
}

struct Cell {
  stop: Arc<AtomicBool>,
  done: Arc<AtomicU64>,
  failed: Arc<AtomicU64>,
}

impl Cell {
  fn new() -> Self {
    Self { stop: Arc::new(AtomicBool::new(false)), done: Arc::new(AtomicU64::new(0)), failed: Arc::new(AtomicU64::new(0)) }
  }
}

fn run_async<F, Fut>(threads: usize, limit: Duration, make: F) -> (u64, u64)
where
  F: Fn() -> Box<dyn Fn() -> Fut + Send + Sync> + Send + Sync,
  Fut: std::future::Future<Output = bool> + Send + 'static,
{
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
  let cell = Cell::new();
  rt.block_on(async {
    let mut tasks = Vec::new();
    for _ in 0..threads {
      let one = make();
      let (stop, done, failed) = (cell.stop.clone(), cell.done.clone(), cell.failed.clone());
      tasks.push(tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
          if one().await {
            done.fetch_add(1, Ordering::Relaxed);
          } else {
            failed.fetch_add(1, Ordering::Relaxed);
          }
        }
      }));
    }
    tokio::time::sleep(limit).await;
    cell.stop.store(true, Ordering::Relaxed);
    for t in tasks {
      let _ = t.await;
    }
  });
  (cell.done.load(Ordering::Relaxed), cell.failed.load(Ordering::Relaxed))
}

fn run_sync(threads: usize, limit: Duration, one: Arc<dyn Fn() -> bool + Send + Sync>) -> (u64, u64) {
  let cell = Cell::new();
  let handles: Vec<_> = (0..threads)
    .map(|_| {
      let (stop, done, failed, one) = (cell.stop.clone(), cell.done.clone(), cell.failed.clone(), one.clone());
      std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
          if one() {
            done.fetch_add(1, Ordering::Relaxed);
          } else {
            failed.fetch_add(1, Ordering::Relaxed);
          }
        }
      })
    })
    .collect();
  std::thread::sleep(limit);
  cell.stop.store(true, Ordering::Relaxed);
  for h in handles {
    let _ = h.join();
  }
  (cell.done.load(Ordering::Relaxed), cell.failed.load(Ordering::Relaxed))
}

#[cfg(feature = "http3")]
fn quinn_cell(addr: SocketAddr, cert: &h3s::Cert, threads: usize, limit: Duration) -> (u64, u64) {
  use bytes::Buf;
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
  let cell = Cell::new();
  let config = h3s::client(cert);
  rt.block_on(async {
    let mut ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    ep.set_default_client_config(config);
    let conn = ep.connect(addr, "localhost").unwrap().await.unwrap();
    let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn)).await.unwrap();
    tokio::spawn(async move {
      let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });
    let uri: http::Uri = format!("https://localhost:{}/", addr.port()).parse().unwrap();
    let mut tasks = Vec::new();
    for _ in 0..threads {
      let (mut send, uri) = (send.clone(), uri.clone());
      let (stop, done, failed) = (cell.stop.clone(), cell.done.clone(), cell.failed.clone());
      tasks.push(tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
          let ok = async {
            let mut s = send.send_request(http::Request::get(uri.clone()).body(()).ok()?).await.ok()?;
            s.finish().await.ok()?;
            let resp = s.recv_response().await.ok()?;
            let mut n = 0;
            while let Some(mut d) = s.recv_data().await.ok()? {
              n += d.remaining();
              d.advance(d.remaining());
            }
            Some(resp.status() == 200 && n == BODY.len())
          }
          .await
            == Some(true);
          if ok {
            done.fetch_add(1, Ordering::Relaxed);
          } else {
            failed.fetch_add(1, Ordering::Relaxed);
          }
        }
      }));
    }
    tokio::time::sleep(limit).await;
    cell.stop.store(true, Ordering::Relaxed);
    for t in tasks {
      let _ = t.await;
    }
  });
  (cell.done.load(Ordering::Relaxed), cell.failed.load(Ordering::Relaxed))
}

struct Target<'a> {
  proto: &'a str,
  addr: SocketAddr,
  #[cfg(feature = "http3")]
  cert: &'a h3s::Cert,
}

impl Target<'_> {
  fn url(&self) -> String {
    match self.proto {
      "h3" => format!("https://localhost:{}/", self.addr.port()),
      _ => format!("http://{}/", self.addr),
    }
  }

  fn fibre(&self, b: fibre_http::ClientBuilder) -> fibre_http::ClientBuilder {
    let b = b.no_proxy().redirects(0);
    match self.proto {
      "h2c" => b.http2_prior_knowledge(),
      #[cfg(feature = "http3")]
      "h3" => b.add_root_certificate(self.cert.der.clone()).http3_prior_knowledge(),
      _ => b,
    }
  }
}

fn supports(client: &str, proto: &str) -> bool {
  match client {
    "fibre-async" | "fibre-sync" => true,
    "quinn" => proto == "h3",
    _ => proto != "h3",
  }
}

fn cell(client: &str, target: &Target, threads: usize, limit: Duration) -> (u64, u64) {
  let url = target.url();
  let h2 = target.proto == "h2c";
  match client {
    "fibre-async" => {
      let pool = fibre_http::Pool::builder().max_per_host(threads.max(1)).build();
      let c = target.fibre(fibre_http::AsyncClient::builder().pool(&pool)).build_async().unwrap();
      run_async(threads, limit, move || {
        let (c, url) = (c.clone(), url.clone());
        Box::new(move || {
          let (c, url) = (c.clone(), url.clone());
          async move {
            match c.get(&url).send().await {
              Ok(r) => r.bytes().await.is_ok_and(|b| b.len() == 64),
              Err(_) => false,
            }
          }
        })
      })
    }
    "fibre-sync" => {
      let pool = fibre_http::Pool::builder().max_per_host(threads.max(1)).build();
      let c = target.fibre(fibre_http::SyncClient::builder().pool(&pool)).build_sync().unwrap();
      run_sync(threads, limit, Arc::new(move || match c.get(&url).send() {
        Ok(r) => r.bytes().is_ok_and(|b| b.len() == 64),
        Err(_) => false,
      }))
    }
    "hyper" => {
      let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
      let _guard = rt.enter();
      let mut b = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new());
      if h2 {
        b.http2_only(true);
      }
      let c: hyper_util::client::legacy::Client<_, Empty<Bytes>> = b.build_http();
      let uri: hyper::Uri = url.parse().unwrap();
      drop(_guard);
      drop(rt);
      run_async(threads, limit, move || {
        let (c, uri) = (c.clone(), uri.clone());
        Box::new(move || {
          let (c, uri) = (c.clone(), uri.clone());
          async move {
            match c.get(uri).await {
              Ok(r) => r.into_body().collect().await.is_ok_and(|b| b.to_bytes().len() == 64),
              Err(_) => false,
            }
          }
        })
      })
    }
    "reqwest-async" => {
      let mut b = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none());
      if h2 {
        b = b.http2_prior_knowledge();
      }
      let c = b.build().unwrap();
      run_async(threads, limit, move || {
        let (c, url) = (c.clone(), url.clone());
        Box::new(move || {
          let (c, url) = (c.clone(), url.clone());
          async move {
            match c.get(&url).send().await {
              Ok(r) => r.bytes().await.is_ok_and(|b| b.len() == 64),
              Err(_) => false,
            }
          }
        })
      })
    }
    "reqwest-blocking" => {
      let mut b = reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none());
      if h2 {
        b = b.http2_prior_knowledge();
      }
      let c = b.build().unwrap();
      run_sync(threads, limit, Arc::new(move || match c.get(&url).send() {
        Ok(r) => r.bytes().is_ok_and(|b| b.len() == 64),
        Err(_) => false,
      }))
    }
    #[cfg(feature = "http3")]
    "quinn" => quinn_cell(target.addr, target.cert, threads, limit),
    other => panic!("unknown client {other}"),
  }
}

fn list<T>(arg: Option<&String>, default: impl FnOnce() -> Vec<T>, parse: impl Fn(&str) -> T) -> Vec<T> {
  arg.map(|s| s.split(',').map(&parse).collect()).unwrap_or_else(default)
}

fn main() {
  let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();
  let secs: u64 = args.first().map(|s| s.parse().unwrap()).unwrap_or(10);
  let clients = list(args.get(1), || CLIENTS.iter().map(|c| c.to_string()).collect(), str::to_owned);
  let protos = list(args.get(2), || PROTOCOLS.iter().map(|p| p.to_string()).collect(), str::to_owned);
  let levels = list(args.get(3), || THREADS.to_vec(), |n| n.parse().unwrap());
  let limit = Duration::from_secs(secs);
  let (h1, h2) = server();
  #[cfg(feature = "http3")]
  let cert = h3s::cert();
  #[cfg(feature = "http3")]
  let h3 = h3s::server(&cert);
  std::thread::sleep(Duration::from_millis(100));
  let cells: usize = protos.iter().map(|p| clients.iter().filter(|c| supports(c, p)).count() * levels.len()).sum();
  println!("{cells} cells x {secs} s");
  for proto in protos.iter().map(String::as_str) {
    let addr = match proto {
      "h1" => h1,
      "h2c" => h2,
      #[cfg(feature = "http3")]
      "h3" => h3,
      other => panic!("unknown protocol {other}"),
    };
    let target = Target {
      proto,
      addr,
      #[cfg(feature = "http3")]
      cert: &cert,
    };
    for &threads in &levels {
      for client in clients.iter().filter(|c| supports(c, proto)) {
        #[cfg(feature = "diagnostics")]
        fibre_http::diagnostics::reset();
        let started = Instant::now();
        let (done, failed) = cell(client, &target, threads, limit);
        let wall = started.elapsed().as_secs_f64();
        let rps = done as f64 / wall;
        let ns = 1e9 / (rps / threads as f64);
        println!("{proto:4} {client:17} T{threads}: {rps:>10.0} req/s  {ns:>9.0} ns/req/requester  failed {failed}");
        #[cfg(feature = "diagnostics")]
        if client.starts_with("fibre") {
          let d = fibre_http::diagnostics::snapshot();
          let per = |n: u64| n as f64 / done.max(1) as f64;
          println!(
            "      per request: turns {:.2}  empty turns {:.2}  leads {:.2}  handoffs {:.2}  parks {:.2}  events {:.2}  unread socket {:.0} ns  parked {:.0} ns  relock {:.0} ns",
            per(d.turns), per(d.empty_turns), per(d.leads), per(d.handoffs), per(d.parks), per(d.taken), per(d.idle_ns), per(d.parked_ns), per(d.relock_ns)
          );
        }
      }
    }
  }
}
