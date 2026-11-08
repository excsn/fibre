use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use fibre_http::{AsyncClient, Error, Proxy, SyncClient};
use tokio_rustls::rustls;

fn read_head(reader: &mut BufReader<TcpStream>) -> Option<(String, Vec<(String, String)>)> {
  let mut line = String::new();
  if reader.read_line(&mut line).ok()? == 0 {
    return None;
  }
  let mut headers = Vec::new();
  loop {
    let mut h = String::new();
    if reader.read_line(&mut h).ok()? == 0 {
      return None;
    }
    if h == "\r\n" {
      break;
    }
    if let Some((k, v)) = h.trim_end().split_once(':') {
      headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
  }
  Some((line.trim_end().to_string(), headers))
}

type Heads = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

/// A forward proxy that answers every request itself and records what it received.
fn forward_proxy() -> (SocketAddr, Heads) {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  let heads: Heads = Arc::default();
  let record = heads.clone();
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      let record = record.clone();
      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut w = stream;
        while let Some(head) = read_head(&mut reader) {
          record.lock().unwrap().push(head);
          if w.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 7\r\n\r\nproxied").is_err() {
            return;
          }
        }
      });
    }
  });
  (addr, heads)
}

/// A `CONNECT` proxy that tunnels to whatever it is asked for, or answers `refuse` when set.
fn tunnel_proxy(refuse: Option<&'static str>) -> (SocketAddr, Heads) {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  let heads: Heads = Arc::default();
  let record = heads.clone();
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      let record = record.clone();
      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let Some((line, headers)) = read_head(&mut reader) else { return };
        record.lock().unwrap().push((line.clone(), headers));
        let mut client = stream;
        if let Some(reply) = refuse {
          let _ = client.write_all(reply.as_bytes());
          return;
        }
        let target = line.split_whitespace().nth(1).unwrap().to_string();
        let Ok(upstream) = TcpStream::connect(&target) else { return };
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").unwrap();
        let (mut up_r, mut up_w) = (upstream.try_clone().unwrap(), upstream);
        let mut down_w = client.try_clone().unwrap();
        let buffered = reader.buffer().to_vec();
        if !buffered.is_empty() {
          up_w.write_all(&buffered).unwrap();
        }
        let mut down_r = reader.into_inner();
        std::thread::spawn(move || {
          let _ = std::io::copy(&mut down_r, &mut up_w);
        });
        let _ = std::io::copy(&mut up_r, &mut down_w);
      });
    }
  });
  (addr, heads)
}

/// A TLS server for `localhost` that answers `hello` over HTTP/2 or HTTP/1.1, by ALPN.
fn tls_server() -> (SocketAddr, Vec<u8>) {
  let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
  let root = cert.cert.der().to_vec();
  let key = rustls::pki_types::PrivateKeyDer::try_from(cert.signing_key.serialize_der()).unwrap();
  let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.cert.der().clone()], key)
    .unwrap();
  config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
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
            let Ok(mut conn) = h2::server::handshake(tls).await else { return };
            while let Some(Ok((_req, mut respond))) = conn.accept().await {
              let mut send = respond.send_response(http::Response::new(()), false).unwrap();
              let _ = send.send_data(bytes::Bytes::from_static(b"hello"), true);
            }
          } else {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
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
              if io.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello").await.is_err() {
                return;
              }
              let _ = io.flush().await;
            }
          }
        });
      }
    });
  });
  (addr, root)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
  headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[test]
fn a_forward_proxy_gets_absolute_form_with_host_and_credentials() {
  let (addr, heads) = forward_proxy();
  let proxy = Proxy::parse(&format!("http://us%3Aer:pa%40ss@{addr}")).unwrap();
  let client = SyncClient::builder().proxy(proxy).build_sync().unwrap();
  for _ in 0..3 {
    let r = client.get("http://origin.test:8080/path?q=1").send().unwrap();
    assert_eq!(r.text().unwrap(), "proxied");
  }
  let heads = heads.lock().unwrap();
  assert_eq!(heads.len(), 3);
  let (line, headers) = &heads[0];
  assert_eq!(line, "GET http://origin.test:8080/path?q=1 HTTP/1.1");
  assert_eq!(header(headers, "host"), Some("origin.test:8080"));
  assert_eq!(header(headers, "proxy-authorization"), Some("Basic dXM6ZXI6cGFAc3M="));
}

#[test]
fn https_goes_through_a_connect_tunnel() {
  let (tls, root) = tls_server();
  let (addr, heads) = tunnel_proxy(None);
  let proxy = Proxy::parse(&format!("http://{addr}")).unwrap();
  let client = SyncClient::builder().proxy(proxy).add_root_certificate(root).build_sync().unwrap();
  let r = client.get(format!("https://localhost:{}/", tls.port())).send().unwrap();
  assert_eq!(r.version(), http::Version::HTTP_2);
  assert_eq!(r.text().unwrap(), "hello");
  let heads = heads.lock().unwrap();
  let (line, headers) = &heads[0];
  assert_eq!(line, &format!("CONNECT localhost:{} HTTP/1.1", tls.port()));
  assert_eq!(header(headers, "host"), Some(format!("localhost:{}", tls.port()).as_str()));
  assert!(header(headers, "user-agent").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_async_client_tunnels_too() {
  let (tls, root) = tls_server();
  let (addr, _) = tunnel_proxy(None);
  let proxy = Proxy::parse(&format!("http://{addr}")).unwrap();
  let client = AsyncClient::builder().proxy(proxy).add_root_certificate(root).build_async().unwrap();
  for _ in 0..3 {
    let r = client.get(format!("https://localhost:{}/", tls.port())).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "hello");
  }
}

#[test]
fn a_407_from_the_proxy_is_proxy_auth() {
  let (addr, _) = tunnel_proxy(Some("HTTP/1.1 407 Proxy Authentication Required\r\ncontent-length: 0\r\n\r\n"));
  let proxy = Proxy::parse(&format!("http://{addr}")).unwrap();
  let client = SyncClient::builder().proxy(proxy).build_sync().unwrap();
  let r = client.get("https://localhost:1/").send();
  assert!(matches!(r, Err(Error::ProxyAuth)), "{:?}", r.err());
}

#[test]
fn a_refused_connect_is_an_error() {
  let (addr, _) = tunnel_proxy(Some("HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n"));
  let proxy = Proxy::parse(&format!("http://{addr}")).unwrap();
  let client = SyncClient::builder().proxy(proxy).build_sync().unwrap();
  let r = client.get("https://localhost:1/").send();
  assert!(matches!(r, Err(Error::Protocol(_))), "{:?}", r.err());
}

#[test]
fn no_proxy_hosts_go_direct() {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  std::thread::spawn(move || {
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    read_head(&mut reader).unwrap();
    let mut w = stream;
    w.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\ndirect").unwrap();
    let mut rest = Vec::new();
    let _ = reader.read_to_end(&mut rest);
  });
  let proxy = Proxy::parse("http://127.0.0.1:1").unwrap();
  let client = SyncClient::builder().proxy(proxy).no_proxy_for("127.0.0.1").build_sync().unwrap();
  assert_eq!(client.get(format!("http://{addr}/")).send().unwrap().text().unwrap(), "direct");
}
