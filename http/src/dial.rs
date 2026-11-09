//! Opening a connection: resolve, connect, TLS and the engine the handshake agreed on.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::Error;
use crate::conn::{Conn, Proto};
use crate::engine::Engine;
use crate::io::Io;
use crate::pool::Key;
use crate::wait::{self, Registration};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Dialer {
  #[cfg(feature = "rustls")]
  pub tls: Option<Arc<rustls::ClientConfig>>,
  /// Set when HTTPS goes through native-tls instead of rustls.
  #[cfg(feature = "native-tls")]
  pub native: Option<native_tls::TlsConnector>,
  pub h2_prior_knowledge: bool,
  /// HTTP/2 PING interval and PONG timeout.
  #[cfg_attr(not(feature = "http2"), allow(dead_code))]
  pub h2_keep_alive: Option<(Duration, Duration)>,
  pub proxies: crate::proxy::Proxies,
  pub user_agent: String,
}

/// Where the TCP connection goes: the tunnel's proxy, or the key's host, which is the proxy for a forwarded key.
fn endpoint(key: &Key) -> (&str, u16) {
  match &key.via {
    Some(via) if via.tunnel => (via.proxy.host.as_str(), via.proxy.port),
    _ => (key.host.as_str(), key.port),
  }
}

fn remaining(deadline: Option<Instant>) -> Result<Duration, Error> {
  match deadline {
    Some(d) => {
      let now = Instant::now();
      if now >= d {
        return Err(Error::Timeout);
      }
      Ok((d - now).min(CONNECT_TIMEOUT))
    }
    None => Ok(CONNECT_TIMEOUT),
  }
}

fn transport(dialer: &Dialer, key: &Key, sock: TcpStream) -> Result<Io, Error> {
  sock.set_nodelay(true)?;
  sock.set_nonblocking(true)?;
  if !key.https {
    return Ok(Io::Plain(sock));
  }
  #[cfg(feature = "native-tls")]
  if let Some(connector) = dialer.native.clone() {
    return Ok(Io::Native(Box::new(crate::io::native::NativeIo::new(sock, connector, key.host.clone()))));
  }
  #[cfg(feature = "rustls")]
  {
    let config = dialer.tls.clone().ok_or_else(|| Error::Tls("no TLS configuration".into()))?;
    let name = rustls::pki_types::ServerName::try_from(key.host.clone()).map_err(|e| Error::Tls(e.to_string()))?;
    let tls = rustls::ClientConnection::new(config, name).map_err(|e| Error::Tls(e.to_string()))?;
    Ok(Io::Tls(Box::new((sock, tls))))
  }
  #[cfg(not(feature = "rustls"))]
  {
    let _ = dialer;
    Err(Error::Tls("https needs the rustls feature".into()))
  }
}

#[cfg_attr(not(all(feature = "http1", feature = "http2")), allow(unreachable_code, unused_variables))]
fn finish(dialer: &Dialer, key: &Key, io: Io, reg: Registration) -> Result<Arc<Conn>, Error> {
  let h2 = match io.alpn() {
    Some(p) => p == b"h2",
    None => !key.https && dialer.h2_prior_knowledge && key.via.is_none(),
  };
  let (engine, proto): (Box<dyn Engine + Send>, Proto) = if h2 {
    #[cfg(feature = "http2")]
    {
      let engine = match dialer.h2_keep_alive {
        Some((interval, timeout)) => crate::h2::Engine::new().keep_alive(interval, timeout),
        None => crate::h2::Engine::new(),
      };
      (Box::new(engine), Proto::H2)
    }
    #[cfg(not(feature = "http2"))]
    {
      return Err(Error::Protocol("server chose HTTP/2 but the http2 feature is off".into()));
    }
  } else {
    #[cfg(feature = "http1")]
    {
      let engine = if key.forwarded().is_some() { crate::h1::Engine::absolute_form() } else { crate::h1::Engine::new() };
      (Box::new(engine), Proto::H1)
    }
    #[cfg(not(feature = "http1"))]
    {
      return Err(Error::Protocol("HTTP/1.1 needs the http1 feature".into()));
    }
  };
  Ok(Conn::new(engine, io, proto, reg))
}

pub(crate) fn dial_sync(dialer: &Dialer, key: &Key, deadline: Option<Instant>) -> Result<Arc<Conn>, Error> {
  let (host, port) = endpoint(key);
  let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
  let mut last = Error::Io(format!("no address for {host}"));
  let mut sock = None;
  for addr in addrs {
    match TcpStream::connect_timeout(&addr, remaining(deadline)?) {
      Ok(s) => {
        sock = Some(s);
        break;
      }
      Err(e) => last = e.into(),
    }
  }
  let mut sock = sock.ok_or(last)?;
  if let Some(via) = key.via.as_ref().filter(|v| v.tunnel) {
    sock.set_nonblocking(true)?;
    crate::proxy::connect_tunnel_sync(&mut sock, &via.proxy, &key.host, key.port, &dialer.user_agent, deadline)?;
  }
  let mut io = transport(dialer, key, sock)?;
  while let Some(interest) = io.handshake().map_err(|e| Error::Tls(e.to_string()))? {
    if !wait::block(io.fd(), interest, deadline)? {
      return Err(Error::Timeout);
    }
  }
  finish(dialer, key, io, Registration::default())
}

/// The addresses for `host`, through hickory when that feature is on and the OS resolver on tokio's blocking pool otherwise.
async fn lookup_async(host: &str, port: u16) -> Result<Vec<SocketAddr>, Error> {
  if let Ok(ip) = host.parse::<std::net::IpAddr>() {
    return Ok(vec![SocketAddr::new(ip, port)]);
  }
  #[cfg(feature = "hickory")]
  {
    static RESOLVER: std::sync::OnceLock<Result<hickory_resolver::TokioResolver, String>> = std::sync::OnceLock::new();
    let resolver = RESOLVER.get_or_init(|| hickory_resolver::TokioResolver::builder_tokio().and_then(|b| b.build()).map_err(|e| e.to_string()));
    let resolver = resolver.as_ref().map_err(|e| Error::Io(format!("resolver: {e}")))?;
    let ips = resolver.lookup_ip(host).await.map_err(|e| Error::Io(format!("lookup {host}: {e}")))?;
    Ok(ips.iter().map(|ip| SocketAddr::new(ip, port)).collect())
  }
  #[cfg(not(feature = "hickory"))]
  Ok(tokio::net::lookup_host((host, port)).await?.collect())
}

pub(crate) async fn dial_async(dialer: &Dialer, key: &Key, deadline: Option<Instant>) -> Result<Arc<Conn>, Error> {
  let limit = remaining(deadline)?;
  let (host, port) = endpoint(key);
  let connect = async {
    let mut last = Error::Io(format!("no address for {host}"));
    for addr in lookup_async(host, port).await? {
      match tokio::net::TcpStream::connect(addr).await {
        Ok(s) => return Ok(s),
        Err(e) => last = e.into(),
      }
    }
    Err(last)
  };
  let sock = match tokio::time::timeout(limit, connect).await {
    Ok(r) => r?,
    Err(_) => return Err(Error::Timeout),
  };
  let mut sock = sock.into_std()?;
  let reg = Registration::default();
  if let Some(via) = key.via.as_ref().filter(|v| v.tunnel) {
    sock.set_nonblocking(true)?;
    let tunnel = crate::proxy::connect_tunnel_async(&mut sock, &reg, &via.proxy, &key.host, key.port, &dialer.user_agent, deadline).await;
    reg.forget();
    tunnel?;
  }
  let mut io = transport(dialer, key, sock)?;
  while let Some(interest) = io.handshake().map_err(|e| Error::Tls(e.to_string()))? {
    if !reg.wait(io.fd(), interest, deadline).await? {
      return Err(Error::Timeout);
    }
  }
  finish(dialer, key, io, reg)
}
