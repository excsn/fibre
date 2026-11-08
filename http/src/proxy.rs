//! HTTP proxies: which requests go through one, and the `CONNECT` tunnel for HTTPS.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use crate::Error;
use crate::wait::{self, Interest};

/// A proxy at `http://[user:pass@]host[:port]`. Only HTTP proxies are supported.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
  pub(crate) host: String,
  pub(crate) port: u16,
  /// `Basic <base64>` for `Proxy-Authorization`, from the URL's userinfo.
  pub(crate) auth: Option<String>,
}

impl Proxy {
  pub fn parse(url: &str) -> Result<Proxy, Error> {
    let bad = |why: &str| Error::Url(format!("proxy {url}: {why}"));
    let rest = match url.split_once("://") {
      Some(("http", rest)) => rest,
      Some((scheme, _)) => return Err(bad(&format!("scheme {scheme} is not supported"))),
      None => url,
    };
    let rest = rest.split('/').next().unwrap_or("");
    let (userinfo, hostport) = match rest.rsplit_once('@') {
      Some((u, h)) => (Some(u), h),
      None => (None, rest),
    };
    let (host, port) = match hostport.strip_prefix('[') {
      Some(v6) => {
        let (h, after) = v6.split_once(']').ok_or_else(|| bad("unclosed ["))?;
        (h.to_owned(), after.strip_prefix(':').map(str::to_owned))
      }
      None => match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), Some(p.to_owned())),
        None => (hostport.to_owned(), None),
      },
    };
    if host.is_empty() {
      return Err(bad("no host"));
    }
    let port = match port {
      Some(p) => p.parse().map_err(|_| bad("bad port"))?,
      None => 80,
    };
    let auth = userinfo.map(|u| {
      let (user, pass) = u.split_once(':').unwrap_or((u, ""));
      format!("Basic {}", base64(format!("{}:{}", percent_decode(user), percent_decode(pass)).as_bytes()))
    });
    Ok(Proxy { host, port, auth })
  }
}

/// Hosts that bypass the proxy, from `NO_PROXY`.
#[derive(Debug, Clone, Default)]
pub(crate) struct NoProxy {
  all: bool,
  entries: Vec<String>,
}

impl NoProxy {
  pub(crate) fn parse(list: &str) -> NoProxy {
    let mut no = NoProxy::default();
    for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
      if entry == "*" {
        no.all = true;
      }
      no.entries.push(entry.trim_start_matches('.').trim_start_matches("*.").to_ascii_lowercase());
    }
    no
  }

  /// Whether `host` matches an entry: the same host, or a subdomain of a domain entry.
  pub(crate) fn matches(&self, host: &str) -> bool {
    if self.all {
      return true;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    self.entries.iter().any(|e| {
      let e = e.trim_start_matches('[').trim_end_matches(']');
      let e = match e.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h,
        _ => e,
      };
      host == e || host.ends_with(&format!(".{e}"))
    })
  }
}

/// The proxies a client sends through.
#[derive(Debug, Clone, Default)]
pub(crate) struct Proxies {
  pub http: Option<Proxy>,
  pub https: Option<Proxy>,
  pub no_proxy: NoProxy,
}

impl Proxies {
  /// From `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`, read through `lookup`, lowercase first. Uppercase `HTTP_PROXY` is ignored when `REQUEST_METHOD` is set, since a CGI request can set it (httpoxy).
  pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Proxies {
    let get = |lower: &str, upper: &str| lookup(lower).or_else(|| lookup(upper)).filter(|v| !v.trim().is_empty());
    let cgi = lookup("REQUEST_METHOD").is_some();
    let all = get("all_proxy", "ALL_PROXY");
    let http = if cgi { lookup("http_proxy").filter(|v| !v.trim().is_empty()) } else { get("http_proxy", "HTTP_PROXY") }.or_else(|| all.clone());
    let https = get("https_proxy", "HTTPS_PROXY").or(all);
    Proxies {
      http: http.and_then(|u| Proxy::parse(&u).ok()),
      https: https.and_then(|u| Proxy::parse(&u).ok()),
      no_proxy: NoProxy::parse(&get("no_proxy", "NO_PROXY").unwrap_or_default()),
    }
  }

  pub(crate) fn from_env() -> Proxies {
    Proxies::from_lookup(|name| std::env::var(name).ok())
  }

  /// The proxy for a request to `host`, if any.
  pub(crate) fn for_target(&self, https: bool, host: &str) -> Option<&Proxy> {
    let proxy = if https { self.https.as_ref() } else { self.http.as_ref() }?;
    (!self.no_proxy.matches(host)).then_some(proxy)
  }
}

fn connect_request(proxy: &Proxy, host: &str, port: u16, user_agent: &str) -> String {
  let authority = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
  let mut request = format!("CONNECT {authority} HTTP/1.1\r\nhost: {authority}\r\nuser-agent: {user_agent}\r\n");
  if let Some(auth) = &proxy.auth {
    request.push_str(&format!("proxy-authorization: {auth}\r\n"));
  }
  request.push_str("\r\n");
  request
}

fn connect_status(head: &[u8]) -> Result<(), Error> {
  let line = String::from_utf8_lossy(head);
  let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
  match status {
    200..=299 => Ok(()),
    407 => Err(Error::ProxyAuth),
    other => Err(Error::Protocol(format!("proxy refused CONNECT with {other}"))),
  }
}

/// Opens a `CONNECT` tunnel to `host:port` over `sock`, which is connected to the proxy and non-blocking.
pub(crate) fn connect_tunnel_sync(sock: &mut TcpStream, proxy: &Proxy, host: &str, port: u16, user_agent: &str, deadline: Option<Instant>) -> Result<(), Error> {
  use std::os::fd::AsRawFd;
  let request = connect_request(proxy, host, port, user_agent);
  let fd = sock.as_raw_fd();
  let mut out = request.as_bytes();
  while !out.is_empty() {
    match sock.write(out) {
      Ok(n) => out = &out[n..],
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        if !wait::block(fd, Interest::WRITE, deadline)? {
          return Err(Error::Timeout);
        }
      }
      Err(e) => return Err(e.into()),
    }
  }
  let mut head = Vec::new();
  let mut byte = [0u8; 1];
  while !head.ends_with(b"\r\n\r\n") {
    if head.len() > 16 * 1024 {
      return Err(Error::Protocol("proxy CONNECT reply head too large".into()));
    }
    match sock.read(&mut byte) {
      Ok(0) => return Err(Error::Protocol("proxy closed during CONNECT".into())),
      Ok(_) => head.push(byte[0]),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        if !wait::block(fd, Interest::READ, deadline)? {
          return Err(Error::Timeout);
        }
      }
      Err(e) => return Err(e.into()),
    }
  }
  connect_status(&head)
}

/// The async form of [`connect_tunnel_sync`], waiting through `reg`.
pub(crate) async fn connect_tunnel_async(sock: &mut TcpStream, reg: &crate::wait::Registration, proxy: &Proxy, host: &str, port: u16, user_agent: &str, deadline: Option<Instant>) -> Result<(), Error> {
  use std::os::fd::AsRawFd;
  let request = connect_request(proxy, host, port, user_agent);
  let fd = sock.as_raw_fd();
  let mut out = request.as_bytes();
  while !out.is_empty() {
    match sock.write(out) {
      Ok(n) => out = &out[n..],
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        if !reg.wait(fd, Interest::WRITE, deadline).await? {
          return Err(Error::Timeout);
        }
      }
      Err(e) => return Err(e.into()),
    }
  }
  let mut head = Vec::new();
  let mut byte = [0u8; 1];
  while !head.ends_with(b"\r\n\r\n") {
    if head.len() > 16 * 1024 {
      return Err(Error::Protocol("proxy CONNECT reply head too large".into()));
    }
    match sock.read(&mut byte) {
      Ok(0) => return Err(Error::Protocol("proxy closed during CONNECT".into())),
      Ok(_) => head.push(byte[0]),
      Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
        if !reg.wait(fd, Interest::READ, deadline).await? {
          return Err(Error::Timeout);
        }
      }
      Err(e) => return Err(e.into()),
    }
  }
  connect_status(&head)
}

fn percent_decode(s: &str) -> String {
  let bytes = s.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
        out.push(b);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i]);
    i += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

fn base64(input: &[u8]) -> String {
  const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
  for chunk in input.chunks(3) {
    let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
    let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
    out.push(T[(n >> 18) as usize & 63] as char);
    out.push(T[(n >> 12) as usize & 63] as char);
    out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
    out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn proxy_urls_parse_with_credentials_and_ports() {
    let p = Proxy::parse("http://al%40ice:s3cr%3At@proxy.test:3128").unwrap();
    assert_eq!((p.host.as_str(), p.port), ("proxy.test", 3128));
    assert_eq!(p.auth.as_deref(), Some(format!("Basic {}", base64(b"al@ice:s3cr:t")).as_str()));
    let p = Proxy::parse("proxy.test").unwrap();
    assert_eq!((p.host.as_str(), p.port, p.auth), ("proxy.test", 80, None));
    let p = Proxy::parse("http://[::1]:8080/").unwrap();
    assert_eq!((p.host.as_str(), p.port), ("::1", 8080));
    assert!(Proxy::parse("socks5://proxy.test:1080").is_err());
  }

  #[test]
  fn base64_matches_rfc_4648_vectors() {
    for (input, out) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
      assert_eq!(base64(input.as_bytes()), out);
    }
  }

  #[test]
  fn no_proxy_matches_hosts_and_subdomains() {
    let no = NoProxy::parse("example.com, .internal.test,10.0.0.1, [::1], localhost:8080");
    assert!(no.matches("example.com"));
    assert!(no.matches("api.example.com"));
    assert!(!no.matches("notexample.com"));
    assert!(no.matches("svc.internal.test"));
    assert!(no.matches("internal.test"));
    assert!(no.matches("10.0.0.1"));
    assert!(!no.matches("10.0.0.2"));
    assert!(no.matches("[::1]"));
    assert!(no.matches("localhost"));
    assert!(NoProxy::parse("*").matches("anything.test"));
    assert!(!NoProxy::parse("").matches("anything.test"));
  }

  #[test]
  fn environment_variables_pick_the_proxies() {
    let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
    let p = Proxies::from_lookup(env(&[("HTTP_PROXY", "http://h:1"), ("https_proxy", "http://s:2"), ("NO_PROXY", "skip.test")]));
    assert_eq!(p.for_target(false, "a.test").map(|p| p.port), Some(1));
    assert_eq!(p.for_target(true, "a.test").map(|p| p.port), Some(2));
    assert!(p.for_target(true, "skip.test").is_none());
    let p = Proxies::from_lookup(env(&[("ALL_PROXY", "http://all:3")]));
    assert_eq!(p.for_target(false, "a.test").map(|p| p.port), Some(3));
    assert_eq!(p.for_target(true, "a.test").map(|p| p.port), Some(3));
    let p = Proxies::from_lookup(env(&[("HTTP_PROXY", "http://evil:1"), ("REQUEST_METHOD", "GET")]));
    assert!(p.for_target(false, "a.test").is_none());
    let p = Proxies::from_lookup(env(&[("http_proxy", "http://ok:4"), ("REQUEST_METHOD", "GET")]));
    assert_eq!(p.for_target(false, "a.test").map(|p| p.port), Some(4));
  }
}
