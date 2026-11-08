#![cfg(all(feature = "gzip", feature = "zstd"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use fibre_http::{AsyncClient, Error, SyncClient};

fn gzip(data: &[u8]) -> Vec<u8> {
  let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
  e.write_all(data).unwrap();
  e.finish().unwrap()
}

fn zstd(data: &[u8]) -> Vec<u8> {
  ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

fn text(n: usize) -> Vec<u8> {
  (0..n).map(|i| b"abcdefghij klmnop qrstuv wxyz\n"[i % 30]).collect()
}

struct Server {
  addr: SocketAddr,
  accept_encoding: Arc<Mutex<Vec<String>>>,
  accepted: Arc<std::sync::atomic::AtomicUsize>,
}

/// Answers `/gzip`, `/zstd`, `/zstd-chunked` (7-byte chunks), `/gzip-truncated` and `/plain`.
fn server() -> Server {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  let seen = Arc::new(Mutex::new(Vec::new()));
  let record = seen.clone();
  let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
  let count = accepted.clone();
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
      let record = record.clone();
      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut w = stream;
        loop {
          let mut line = String::new();
          if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
          }
          let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
          loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
              return;
            }
            if h == "\r\n" {
              break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("accept-encoding:") {
              record.lock().unwrap().push(v.trim().to_string());
            }
          }
          let body = text(100_000);
          let reply: Vec<u8> = match path.as_str() {
            "/gzip" => {
              let z = gzip(&body);
              [format!("HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\n\r\n", z.len()).into_bytes(), z].concat()
            }
            "/gzip-truncated" => {
              let z = gzip(&body);
              let z = &z[..z.len() / 2];
              [format!("HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\n\r\n", z.len()).into_bytes(), z.to_vec()].concat()
            }
            "/zstd" => {
              let z = zstd(&body);
              [format!("HTTP/1.1 200 OK\r\ncontent-encoding: zstd\r\ncontent-length: {}\r\n\r\n", z.len()).into_bytes(), z].concat()
            }
            "/zstd-chunked" => {
              let z = zstd(&text(1_000_000));
              let mut out = b"HTTP/1.1 200 OK\r\ncontent-encoding: zstd\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
              for c in z.chunks(7) {
                out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
                out.extend_from_slice(c);
                out.extend_from_slice(b"\r\n");
              }
              out.extend_from_slice(b"0\r\n\r\n");
              out
            }
            "/gzip-empty" => b"HTTP/1.1 204 No Content\r\ncontent-encoding: gzip\r\n\r\n".to_vec(),
            "/gzip-trailing" => {
              let z = [gzip(b"short"), b"junk".to_vec()].concat();
              [format!("HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\n\r\n", z.len()).into_bytes(), z].concat()
            }
            "/zstd-frames" => {
              let z = [zstd(b"first "), zstd(b"second")].concat();
              [format!("HTTP/1.1 200 OK\r\ncontent-encoding: zstd\r\ncontent-length: {}\r\n\r\n", z.len()).into_bytes(), z].concat()
            }
            _ => b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nplain".to_vec(),
          };
          if w.write_all(&reply).is_err() {
            return;
          }
        }
      });
    }
  });
  Server { addr, accept_encoding: seen, accepted }
}

#[test]
fn gzip_body_is_decoded_and_its_encoding_headers_removed() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/gzip", s.addr)).send().unwrap();
  assert!(r.headers().get("content-encoding").is_none());
  assert!(r.headers().get("content-length").is_none());
  assert_eq!(r.bytes().unwrap().to_vec(), text(100_000));
  assert_eq!(s.accept_encoding.lock().unwrap().as_slice(), ["gzip, zstd"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zstd_body_is_decoded() {
  let s = server();
  let r = AsyncClient::new().get(format!("http://{}/zstd", s.addr)).send().await.unwrap();
  assert_eq!(r.bytes().await.unwrap().to_vec(), text(100_000));
}

#[test]
fn zstd_body_in_seven_byte_chunks_is_decoded() {
  let s = server();
  let mut body = SyncClient::new().get(format!("http://{}/zstd-chunked", s.addr)).send().unwrap().into_body();
  let mut out = Vec::new();
  body.read_to_end(&mut out).unwrap();
  assert_eq!(out, text(1_000_000));
}

#[test]
fn a_callers_accept_encoding_leaves_the_body_encoded() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/gzip", s.addr)).header("accept-encoding", "gzip").send().unwrap();
  assert_eq!(r.headers()["content-encoding"], "gzip");
  assert_eq!(r.bytes().unwrap().to_vec(), gzip(&text(100_000)));
}

#[test]
fn decompress_off_sends_no_accept_encoding_and_keeps_the_body() {
  let s = server();
  let client = SyncClient::builder().decompress(false).build_sync().unwrap();
  let r = client.get(format!("http://{}/gzip", s.addr)).send().unwrap();
  assert_eq!(r.bytes().unwrap().to_vec(), gzip(&text(100_000)));
  assert!(s.accept_encoding.lock().unwrap().is_empty());
}

#[test]
fn a_truncated_gzip_body_is_an_error() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/gzip-truncated", s.addr)).send().unwrap();
  assert!(matches!(r.bytes(), Err(Error::Decode(_))));
}

#[test]
fn a_plain_body_is_untouched() {
  let s = server();
  assert_eq!(SyncClient::new().get(format!("http://{}/plain", s.addr)).send().unwrap().text().unwrap(), "plain");
}

#[test]
fn an_empty_body_with_an_encoding_decodes_to_nothing() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/gzip-empty", s.addr)).send().unwrap();
  assert_eq!(r.status(), 204);
  assert!(r.bytes().unwrap().is_empty());
}

#[test]
fn bytes_after_the_compressed_stream_are_an_error() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/gzip-trailing", s.addr)).send().unwrap();
  assert!(matches!(r.bytes(), Err(Error::Decode(_))));
}

#[test]
fn several_zstd_frames_decode_as_one_body() {
  let s = server();
  let r = SyncClient::new().get(format!("http://{}/zstd-frames", s.addr)).send().unwrap();
  assert_eq!(r.text().unwrap(), "first second");
}

#[test]
fn a_decoded_body_leaves_its_connection_reusable() {
  let s = server();
  let client = SyncClient::new();
  for _ in 0..3 {
    assert_eq!(client.get(format!("http://{}/zstd", s.addr)).send().unwrap().bytes().unwrap().to_vec(), text(100_000));
    assert_eq!(client.get(format!("http://{}/gzip", s.addr)).send().unwrap().bytes().unwrap().to_vec(), text(100_000));
  }
  assert_eq!(s.accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
}
