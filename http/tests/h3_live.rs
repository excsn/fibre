//! fibre_http against public HTTP/3 servers. Each test first checks with quinn that the server completes a QUIC handshake from this machine, and skips with a `SKIPPED` line on stderr when it does not or when the environment sends HTTPS through a proxy.

#![cfg(all(feature = "http3", feature = "webpki-roots"))]

use std::collections::HashMap;
use std::io::Write;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fibre_http::{AsyncClient, Error, Pool, SyncClient};
use http::{StatusCode, Version};

const TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MISSING: &str = "fibre-http-live-test-missing";

fn skip(test: &str, why: &str) {
  let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
}

fn proxied() -> Option<String> {
  ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// Whether quinn completes a QUIC handshake with ALPN `h3` to `host:port`, remembered for the run.
fn quic_answers(host: &str, port: u16) -> Result<(), String> {
  static SEEN: OnceLock<Mutex<HashMap<(String, u16), Result<(), String>>>> = OnceLock::new();
  let seen = SEEN.get_or_init(Default::default);
  if let Some(r) = seen.lock().unwrap().get(&(host.to_owned(), port)) {
    return r.clone();
  }
  let r = quinn_handshake(host, port);
  seen.lock().unwrap().insert((host.to_owned(), port), r.clone());
  r
}

fn quinn_handshake(host: &str, port: u16) -> Result<(), String> {
  let addr = (host, port).to_socket_addrs().map_err(|e| format!("lookup {host}: {e}"))?.find(|a| a.is_ipv4()).ok_or(format!("no IPv4 address for {host}"))?;
  let mut roots = rustls::RootCertStore::empty();
  roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
  let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
  tls.alpn_protocols = vec![b"h3".to_vec()];
  let config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap()));
  let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  rt.block_on(async move {
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).map_err(|e| e.to_string())?;
    ep.set_default_client_config(config);
    let connecting = ep.connect(addr, host).map_err(|e| e.to_string())?;
    match tokio::time::timeout(PROBE_TIMEOUT, connecting).await {
      Ok(Ok(conn)) => {
        conn.close(0u32.into(), b"");
        Ok(())
      }
      Ok(Err(e)) => Err(format!("QUIC handshake with {host}:{port} failed: {e}")),
      Err(_) => Err(format!("no QUIC handshake with {host}:{port} within {PROBE_TIMEOUT:?}")),
    }
  })
}

/// Why a test against `url` cannot run here, if it cannot.
fn skip_reason(url: &str) -> Option<String> {
  if let Some(p) = proxied() {
    return Some(format!("HTTPS goes through the proxy {p}"));
  }
  let uri: http::Uri = url.parse().unwrap();
  quic_answers(uri.host().unwrap(), uri.port_u16().unwrap_or(443)).err()
}

fn h3_sync() -> SyncClient {
  SyncClient::builder().timeout(TIMEOUT).http3_prior_knowledge().build_sync().unwrap()
}

fn h3_async() -> AsyncClient {
  AsyncClient::builder().timeout(TIMEOUT).http3_prior_knowledge().build_async().unwrap()
}

/// A server's expected answers, as observed on 2026-11-22.
struct Server {
  url: &'static str,
  head: StatusCode,
  post: &'static [StatusCode],
  missing: StatusCode,
}

const CLOUDFLARE_QUIC: Server = Server { url: "https://cloudflare-quic.com/", head: StatusCode::OK, post: &[StatusCode::OK], missing: StatusCode::NOT_FOUND };
const CLOUDFLARE: Server = Server { url: "https://www.cloudflare.com/", head: StatusCode::OK, post: &[StatusCode::OK, StatusCode::INTERNAL_SERVER_ERROR], missing: StatusCode::MOVED_PERMANENTLY };
const GOOGLE: Server = Server { url: "https://www.google.com/", head: StatusCode::OK, post: &[StatusCode::METHOD_NOT_ALLOWED], missing: StatusCode::NOT_FOUND };
const NGINX: Server = Server { url: "https://quic.nginx.org/", head: StatusCode::OK, post: &[StatusCode::METHOD_NOT_ALLOWED], missing: StatusCode::NOT_FOUND };
const QUICHE: Server = Server { url: "https://quic.tech:8443/", head: StatusCode::METHOD_NOT_ALLOWED, post: &[StatusCode::METHOD_NOT_ALLOWED], missing: StatusCode::NOT_FOUND };

fn alt_svc_moves_to_http3(test: &str, s: &Server) {
  if let Some(why) = skip_reason(s.url) {
    return skip(test, &why);
  }
  let client = SyncClient::builder().timeout(TIMEOUT).build_sync().unwrap();
  let first = client.get(s.url).send().unwrap();
  assert_ne!(first.version(), Version::HTTP_3, "the first request goes over TCP");
  assert!(first.headers().get("alt-svc").is_some_and(|v| v.to_str().unwrap().contains("h3=")), "{:?}", first.headers().get("alt-svc"));
  first.bytes().unwrap();
  for _ in 0..2 {
    let r = client.get(s.url).send().unwrap();
    assert_eq!(r.version(), Version::HTTP_3);
    assert_eq!(r.status(), StatusCode::OK);
    assert!(!r.bytes().unwrap().is_empty());
  }
}

fn get_reads_the_whole_body(test: &str, s: &Server) {
  if let Some(why) = skip_reason(s.url) {
    return skip(test, &why);
  }
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  rt.block_on(async {
    let r = h3_async().get(s.url).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.version(), Version::HTTP_3);
    let length = r.headers().get("content-length").map(|v| v.to_str().unwrap().parse::<usize>().unwrap());
    let body = r.bytes().await.unwrap();
    assert!(!body.is_empty());
    if let Some(n) = length {
      assert_eq!(body.len(), n);
    }
  });
}

fn head_has_no_body(test: &str, s: &Server) {
  if let Some(why) = skip_reason(s.url) {
    return skip(test, &why);
  }
  let r = h3_sync().head(s.url).send().unwrap();
  assert_eq!(r.status(), s.head);
  assert_eq!(r.version(), Version::HTTP_3);
  assert!(r.bytes().unwrap().is_empty());
}

fn post_gets_a_final_status(test: &str, s: &Server) {
  if let Some(why) = skip_reason(s.url) {
    return skip(test, &why);
  }
  let client = h3_sync();
  let body: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
  let r = client.post(s.url).header("content-type", "application/octet-stream").body(body).send().unwrap();
  assert!(s.post.contains(&r.status()), "{}", r.status());
  assert_eq!(r.version(), Version::HTTP_3);
  r.bytes().unwrap();
  let again = client.get(s.url).send().unwrap();
  assert_eq!(again.status(), StatusCode::OK, "the connection still serves after the POST");
}

fn missing_path_is_reported(test: &str, s: &Server) {
  if let Some(why) = skip_reason(s.url) {
    return skip(test, &why);
  }
  let client = SyncClient::builder().timeout(TIMEOUT).redirects(0).http3_prior_knowledge().build_sync().unwrap();
  let r = client.get(format!("{}{MISSING}", s.url)).send().unwrap();
  assert_eq!(r.status(), s.missing);
  assert_eq!(r.version(), Version::HTTP_3);
  match r.error_for_status() {
    Ok(r) => assert!(r.status().is_redirection()),
    Err(e) => assert_eq!(e, Error::Status(s.missing.as_u16())),
  }
}

macro_rules! server_tests {
  ($module:ident, $server:expr) => {
    mod $module {
      use super::*;

      #[test]
      fn alt_svc_moves_the_next_request_to_http3() {
        alt_svc_moves_to_http3(concat!(stringify!($module), "::alt_svc_moves_the_next_request_to_http3"), &$server);
      }

      #[test]
      fn get_reads_the_whole_body_async() {
        get_reads_the_whole_body(concat!(stringify!($module), "::get_reads_the_whole_body_async"), &$server);
      }

      #[test]
      fn head_answers_with_no_body() {
        head_has_no_body(concat!(stringify!($module), "::head_answers_with_no_body"), &$server);
      }

      #[test]
      fn a_half_megabyte_post_gets_a_final_status() {
        post_gets_a_final_status(concat!(stringify!($module), "::a_half_megabyte_post_gets_a_final_status"), &$server);
      }

      #[test]
      fn a_missing_path_is_reported() {
        missing_path_is_reported(concat!(stringify!($module), "::a_missing_path_is_reported"), &$server);
      }
    }
  };
}

server_tests!(cloudflare_quic, CLOUDFLARE_QUIC);
server_tests!(cloudflare, CLOUDFLARE);
server_tests!(google, GOOGLE);
server_tests!(nginx, NGINX);
server_tests!(quiche, QUICHE);

#[test]
fn a_redirect_to_another_origin_is_followed_over_http3() {
  let test = "a_redirect_to_another_origin_is_followed_over_http3";
  if let Some(why) = skip_reason("https://google.com/").or_else(|| skip_reason(GOOGLE.url)) {
    return skip(test, &why);
  }
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).timeout(TIMEOUT).http3_prior_knowledge().build_sync().unwrap();
  let r = client.get("https://google.com/").send().unwrap();
  assert_eq!(r.status(), StatusCode::OK);
  assert_eq!(r.version(), Version::HTTP_3);
  r.bytes().unwrap();
  assert_eq!(pool.connections_opened(), 2, "one connection to google.com, one to www.google.com");
}

#[test]
fn concurrent_requests_share_one_connection() {
  let test = "concurrent_requests_share_one_connection";
  if let Some(why) = skip_reason(GOOGLE.url) {
    return skip(test, &why);
  }
  let pool = Pool::default();
  let client = AsyncClient::builder().pool(&pool).timeout(TIMEOUT).http3_prior_knowledge().build_async().unwrap();
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
  rt.block_on(async {
    client.get(GOOGLE.url).send().await.unwrap().bytes().await.unwrap();
    let tasks: Vec<_> = (0..16)
      .map(|_| {
        let c = client.clone();
        tokio::spawn(async move {
          let r = c.get(GOOGLE.url).send().await.unwrap();
          (r.status(), r.version(), r.bytes().await.unwrap().len())
        })
      })
      .collect();
    for t in tasks {
      let (status, version, len) = t.await.unwrap();
      assert_eq!((status, version), (StatusCode::OK, Version::HTTP_3));
      assert!(len > 0);
    }
  });
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn threads_and_tasks_share_one_connection() {
  let test = "threads_and_tasks_share_one_connection";
  if let Some(why) = skip_reason(CLOUDFLARE_QUIC.url) {
    return skip(test, &why);
  }
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).timeout(TIMEOUT).http3_prior_knowledge().build_sync().unwrap();
  client.get(CLOUDFLARE_QUIC.url).send().unwrap().bytes().unwrap();
  let threads: Vec<_> = (0..4)
    .map(|_| {
      let c = client.clone();
      std::thread::spawn(move || (0..3).map(|_| c.get(CLOUDFLARE_QUIC.url).send().unwrap().status()).collect::<Vec<_>>())
    })
    .collect();
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  let async_client = client.clone().to_async();
  let from_tasks = rt.block_on(async move {
    let tasks: Vec<_> = (0..4)
      .map(|_| {
        let c = async_client.clone();
        tokio::spawn(async move { c.get(CLOUDFLARE_QUIC.url).send().await.unwrap().status() })
      })
      .collect();
    let mut out = Vec::new();
    for t in tasks {
      out.push(t.await.unwrap());
    }
    out
  });
  for t in threads {
    assert!(t.join().unwrap().iter().all(|s| *s == StatusCode::OK));
  }
  assert!(from_tasks.iter().all(|s| *s == StatusCode::OK));
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn a_body_started_async_finishes_on_a_blocking_thread() {
  let test = "a_body_started_async_finishes_on_a_blocking_thread";
  if let Some(why) = skip_reason(CLOUDFLARE.url) {
    return skip(test, &why);
  }
  let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
  rt.block_on(async {
    let r = h3_async().get(CLOUDFLARE.url).send().await.unwrap();
    assert_eq!(r.version(), Version::HTTP_3);
    let mut body = r.into_body();
    let first = body.chunk().await.unwrap().expect("a first chunk");
    let rest = tokio::task::spawn_blocking(move || {
      let mut sync = body.to_sync();
      let mut n = 0;
      while let Some(chunk) = sync.chunk().unwrap() {
        n += chunk.len();
      }
      n
    })
    .await
    .unwrap();
    assert!(first.len() + rest > 1024 * 1024, "the page is larger than one receive window: {}", first.len() + rest);
  });
}

#[test]
fn a_dropped_body_leaves_the_connection_working() {
  let test = "a_dropped_body_leaves_the_connection_working";
  if let Some(why) = skip_reason(CLOUDFLARE.url) {
    return skip(test, &why);
  }
  let pool = Pool::default();
  let client = SyncClient::builder().pool(&pool).timeout(TIMEOUT).http3_prior_knowledge().build_sync().unwrap();
  let mut body = client.get(CLOUDFLARE.url).send().unwrap().into_body();
  body.chunk().unwrap().expect("a first chunk");
  drop(body);
  let r = client.get(CLOUDFLARE.url).send().unwrap();
  assert_eq!(r.status(), StatusCode::OK);
  assert!(!r.bytes().unwrap().is_empty());
  assert_eq!(pool.connections_opened(), 1);
}

#[test]
fn a_deadline_shorter_than_the_round_trip_times_out() {
  let test = "a_deadline_shorter_than_the_round_trip_times_out";
  if let Some(why) = skip_reason(QUICHE.url) {
    return skip(test, &why);
  }
  let client = SyncClient::builder().timeout(Duration::from_millis(5)).http3_prior_knowledge().build_sync().unwrap();
  let started = Instant::now();
  assert_eq!(client.get(QUICHE.url).send().err(), Some(Error::Timeout));
  assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn prior_knowledge_to_a_server_without_quic_times_out() {
  let test = "prior_knowledge_to_a_server_without_quic_times_out";
  if let Some(p) = proxied() {
    return skip(test, &format!("HTTPS goes through the proxy {p}"));
  }
  if quic_answers("github.com", 443).is_ok() {
    return skip(test, "github.com answers QUIC now, so it no longer stands for a server without it");
  }
  if let Err(e) = SyncClient::builder().timeout(TIMEOUT).build_sync().unwrap().get("https://github.com/").send() {
    return skip(test, &format!("github.com does not answer over TCP either: {e}"));
  }
  let client = SyncClient::builder().timeout(Duration::from_secs(3)).http3_prior_knowledge().build_sync().unwrap();
  let started = Instant::now();
  assert_eq!(client.get("https://github.com/").send().err(), Some(Error::Timeout));
  assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_certificate_that_does_not_name_the_address_fails_tls() {
  let test = "a_certificate_that_does_not_name_the_address_fails_tls";
  if let Some(why) = skip_reason(CLOUDFLARE_QUIC.url) {
    return skip(test, &why);
  }
  let ip = ("cloudflare-quic.com", 443).to_socket_addrs().unwrap().find(|a| a.is_ipv4()).unwrap().ip();
  let r = h3_sync().get(format!("https://{ip}/")).send();
  assert!(matches!(r, Err(Error::Tls(_))), "{:?}", r.map(|r| r.status()));
}
