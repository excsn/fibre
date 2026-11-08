#![cfg(feature = "cookies")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};

use fibre_http::{AsyncClient, SyncClient};

/// Answers with the request's `Cookie` header as the body, after setting what the path asks for.
fn server() -> SocketAddr {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else { return };
      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut w = stream;
        loop {
          let mut line = String::new();
          if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
          }
          let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
          let mut cookie = String::new();
          let mut length = 0;
          loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
              return;
            }
            if h == "\r\n" {
              break;
            }
            let lower = h.to_ascii_lowercase();
            if lower.starts_with("cookie:") {
              cookie = h["cookie:".len()..].trim().to_string();
            }
            if let Some(v) = lower.strip_prefix("content-length:") {
              length = v.trim().parse().unwrap();
            }
          }
          let mut body = vec![0; length];
          reader.read_exact(&mut body).unwrap();
          let (status, set) = match path.as_str() {
            "/set" => ("200 OK", vec!["a=1; Path=/"]),
            "/overwrite" => ("200 OK", vec!["a=2; Path=/"]),
            "/expire" => ("200 OK", vec!["a=gone; Path=/; Max-Age=0"]),
            "/expired" => ("200 OK", vec!["b=old; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT"]),
            "/scoped/set" => ("200 OK", vec!["s=1; Path=/scoped"]),
            "/redirect-set" => ("302 Found", vec!["r=1; Path=/"]),
            _ => ("200 OK", vec![]),
          };
          let mut head = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\n", cookie.len());
          if path == "/redirect-set" {
            head.push_str("location: /show\r\n");
          }
          for c in set {
            head.push_str(&format!("set-cookie: {c}\r\n"));
          }
          head.push_str("\r\n");
          if w.write_all(head.as_bytes()).is_err() || w.write_all(cookie.as_bytes()).is_err() {
            return;
          }
        }
      });
    }
  });
  addr
}

fn get(client: &SyncClient, addr: SocketAddr, path: &str) -> String {
  client.get(format!("http://{addr}{path}")).send().unwrap().text().unwrap()
}

fn store() -> SyncClient {
  SyncClient::builder().cookie_store(true).build_sync().unwrap()
}

#[test]
fn a_stored_cookie_is_sent_on_later_requests() {
  let addr = server();
  let client = store();
  assert_eq!(get(&client, addr, "/show"), "");
  get(&client, addr, "/set");
  assert_eq!(get(&client, addr, "/show"), "a=1");
}

#[test]
fn a_cookie_with_the_same_name_replaces_the_stored_one() {
  let addr = server();
  let client = store();
  get(&client, addr, "/set");
  get(&client, addr, "/overwrite");
  assert_eq!(get(&client, addr, "/show"), "a=2");
}

#[test]
fn max_age_zero_deletes_and_a_past_expiry_is_not_stored() {
  let addr = server();
  let client = store();
  get(&client, addr, "/set");
  get(&client, addr, "/expire");
  get(&client, addr, "/expired");
  assert_eq!(get(&client, addr, "/show"), "");
}

#[test]
fn a_cookie_is_sent_only_under_its_path() {
  let addr = server();
  let client = store();
  get(&client, addr, "/scoped/set");
  assert_eq!(get(&client, addr, "/show"), "");
  assert_eq!(get(&client, addr, "/scoped/show"), "s=1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cookie_set_on_a_redirect_reaches_the_redirected_request() {
  let addr = server();
  let client = AsyncClient::builder().cookie_store(true).build_async().unwrap();
  let r = client.get(format!("http://{addr}/redirect-set")).send().await.unwrap();
  assert_eq!(r.text().await.unwrap(), "r=1");
}

#[test]
fn without_a_store_no_cookie_is_kept() {
  let addr = server();
  let client = SyncClient::new();
  get(&client, addr, "/set");
  assert_eq!(get(&client, addr, "/show"), "");
}

#[test]
fn a_callers_cookie_header_wins_over_the_store() {
  let addr = server();
  let client = store();
  get(&client, addr, "/set");
  let r = client.get(format!("http://{addr}/show")).header("cookie", "mine=1").send().unwrap();
  assert_eq!(r.text().unwrap(), "mine=1");
}
