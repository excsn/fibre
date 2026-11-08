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
  pub h2_prior_knowledge: bool,
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
    None => !key.https && dialer.h2_prior_knowledge,
  };
  let (engine, proto): (Box<dyn Engine + Send>, Proto) = if h2 {
    #[cfg(feature = "http2")]
    {
      (Box::new(crate::h2::Engine::new()), Proto::H2)
    }
    #[cfg(not(feature = "http2"))]
    {
      return Err(Error::Protocol("server chose HTTP/2 but the http2 feature is off".into()));
    }
  } else {
    #[cfg(feature = "http1")]
    {
      (Box::new(crate::h1::Engine::new()), Proto::H1)
    }
    #[cfg(not(feature = "http1"))]
    {
      return Err(Error::Protocol("HTTP/1.1 needs the http1 feature".into()));
    }
  };
  Ok(Conn::new(engine, io, proto, reg))
}

pub(crate) fn dial_sync(dialer: &Dialer, key: &Key, deadline: Option<Instant>) -> Result<Arc<Conn>, Error> {
  let addrs: Vec<SocketAddr> = (key.host.as_str(), key.port).to_socket_addrs()?.collect();
  let mut last = Error::Io(format!("no address for {}", key.host));
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
  let sock = sock.ok_or(last)?;
  let mut io = transport(dialer, key, sock)?;
  while let Some(interest) = io.handshake().map_err(|e| Error::Tls(e.to_string()))? {
    if !wait::block(io.fd(), interest, deadline)? {
      return Err(Error::Timeout);
    }
  }
  finish(dialer, key, io, Registration::default())
}

pub(crate) async fn dial_async(dialer: &Dialer, key: &Key, deadline: Option<Instant>) -> Result<Arc<Conn>, Error> {
  let limit = remaining(deadline)?;
  let connect = async {
    let mut last = Error::Io(format!("no address for {}", key.host));
    for addr in tokio::net::lookup_host((key.host.as_str(), key.port)).await? {
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
  let sock = sock.into_std()?;
  let mut io = transport(dialer, key, sock)?;
  let reg = Registration::default();
  while let Some(interest) = io.handshake().map_err(|e| Error::Tls(e.to_string()))? {
    if !reg.wait(io.fd(), interest, deadline).await? {
      return Err(Error::Timeout);
    }
  }
  finish(dialer, key, io, reg)
}
