//! Connections by origin. Idle HTTP/1 connections wait in a bounded fibre channel that blocking and async callers both receive from. HTTP/2 and HTTP/3 connections are shared while they have room.
//!
//! An origin that advertised HTTP/3 with `Alt-Svc` gets its next connection over QUIC, while its TCP connections keep serving. A QUIC dial that fails marks the alternative broken for a while and the request goes over TCP.

use std::collections::HashMap;
use std::sync::Arc;
use crate::sync::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fibre::mpmc;
use fibre::sync::HybridMutex;

use crate::Error;
use crate::conn::{Conn, Proto};
use crate::dial::{self, Dialer, H3Target};
use crate::notify::Notify;

/// The longest a waiter sleeps before looking again even without a notify.
const WAIT_LIMIT: Duration = Duration::from_millis(100);
/// How long an HTTP/3 alternative stays unused after its first failed dial. Each further failure in a row doubles it, up to 64 times.
#[cfg(feature = "http3")]
const H3_BROKEN: Duration = Duration::from_secs(5 * 60);

/// How a connection reaches its origin when a proxy is in the way.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Via {
  pub proxy: crate::proxy::Proxy,
  /// `CONNECT` to the origin, for HTTPS. Otherwise requests go to the proxy in absolute form.
  pub tunnel: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
  pub https: bool,
  pub host: String,
  pub port: u16,
  pub via: Option<Via>,
}

impl Key {
  /// The pool key for `uri`. A forwarded `http://` request keys on the proxy, so its connections serve every origin.
  pub(crate) fn for_target(uri: &http::Uri, proxies: &crate::proxy::Proxies) -> Result<Key, Error> {
    let origin = Key::from_uri(uri)?;
    Ok(match proxies.for_target(origin.https, &origin.host) {
      None => origin,
      Some(proxy) if origin.https => Key { via: Some(Via { proxy: proxy.clone(), tunnel: true }), ..origin },
      Some(proxy) => Key { https: false, host: proxy.host.clone(), port: proxy.port, via: Some(Via { proxy: proxy.clone(), tunnel: false }) },
    })
  }

  /// Whether requests on this key's connections are written in absolute form for a forward proxy.
  pub(crate) fn forwarded(&self) -> Option<&crate::proxy::Proxy> {
    self.via.as_ref().filter(|v| !v.tunnel).map(|v| &v.proxy)
  }

  fn from_uri(uri: &http::Uri) -> Result<Key, Error> {
    let https = match uri.scheme_str() {
      Some("https") => true,
      Some("http") => false,
      other => return Err(Error::Url(format!("unsupported scheme {other:?}"))),
    };
    let host = uri.host().ok_or_else(|| Error::Url("no host".into()))?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
    Ok(Key { https, host, port, via: None })
  }
}

struct Host {
  idle_tx: mpmc::Sender<Arc<Conn>>,
  idle_rx: mpmc::Receiver<Arc<Conn>>,
  /// Raised when a connection may have become free: a dial ended, a lease came back or a connection closed.
  notify: Notify,
  count: AtomicUsize,
  dialing: AtomicUsize,
  /// What the last dial to this origin agreed: 0 not known yet, 1 HTTP/1.1, 2 HTTP/2, 3 HTTP/3.
  proto: AtomicU8,
  shared: HybridMutex<Vec<Arc<Conn>>>,
  /// A QUIC dial is under way, so another caller does not start one.
  h3_dialing: AtomicBool,
  #[cfg_attr(not(feature = "http3"), allow(dead_code))]
  alt: crate::sync::Mutex<Alt>,
}

/// What the origin advertised with `Alt-Svc`.
#[derive(Default)]
#[cfg_attr(not(feature = "http3"), allow(dead_code))]
struct Alt {
  /// The HTTP/3 alternative and when it expires.
  h3: Option<(String, u16, Instant)>,
  /// A dial to the alternative failed. TCP until then.
  broken_until: Option<Instant>,
  failures: u32,
}

#[derive(Debug, Clone)]
pub struct PoolBuilder {
  max_per_host: usize,
  idle_timeout: Duration,
  peek_after: Duration,
}

impl Default for PoolBuilder {
  fn default() -> Self {
    Self { max_per_host: 32, idle_timeout: Duration::from_secs(90), peek_after: Duration::from_secs(1) }
  }
}

impl PoolBuilder {
  /// Connections per origin, for both protocols.
  pub fn max_per_host(mut self, n: usize) -> Self {
    self.max_per_host = n.max(1);
    self
  }

  pub fn idle_timeout(mut self, d: Duration) -> Self {
    self.idle_timeout = d;
    self
  }

  /// An idle HTTP/1 connection is peeked for a server close before reuse once it has been idle this long.
  pub fn peek_after(mut self, d: Duration) -> Self {
    self.peek_after = d;
    self
  }

  pub fn build(self) -> Pool {
    Pool { inner: Arc::new(PoolInner { hosts: HybridMutex::new(HashMap::new()), config: self, opened: AtomicUsize::new(0) }) }
  }
}

struct PoolInner {
  hosts: HybridMutex<HashMap<Key, Arc<Host>>>,
  config: PoolBuilder,
  opened: AtomicUsize,
}

/// Connections shared by every client built from it, blocking and async alike.
#[derive(Clone)]
pub struct Pool {
  inner: Arc<PoolInner>,
}

impl Default for Pool {
  fn default() -> Self {
    Pool::builder().build()
  }
}

pub(crate) struct Lease {
  pub conn: Arc<Conn>,
  host: Arc<Host>,
  pub reused: bool,
}

impl Lease {
  /// Returns an HTTP/1 connection to the idle set, or drops a closed one.
  pub(crate) fn release(self) {
    let host = self.host.clone();
    if self.conn.proto == Proto::H1 {
      if self.conn.is_closed() {
        host.count.fetch_sub(1, Ordering::AcqRel);
      } else {
        self.conn.mark_idle();
        if host.idle_tx.try_send(self.conn).is_err() {
          host.count.fetch_sub(1, Ordering::AcqRel);
        }
      }
    }
    host.notify.notify_all();
  }
}

enum Found {
  Lease(Lease),
  Dial,
  #[cfg_attr(not(feature = "http3"), allow(dead_code))]
  DialH3(H3Target),
  Wait,
}

impl Pool {
  pub fn builder() -> PoolBuilder {
    PoolBuilder::default()
  }

  /// Connections this pool has opened since it was built.
  pub fn connections_opened(&self) -> usize {
    self.inner.opened.load(Ordering::Relaxed)
  }

  fn host(&self, key: &Key) -> Arc<Host> {
    let mut hosts = self.inner.hosts.lock();
    hosts
      .entry(key.clone())
      .or_insert_with(|| {
        let (idle_tx, idle_rx) = mpmc::bounded(self.inner.config.max_per_host);
        Arc::new(Host {
          idle_tx,
          idle_rx,
          notify: Notify::default(),
          count: AtomicUsize::new(0),
          dialing: AtomicUsize::new(0),
          proto: AtomicU8::new(0),
          shared: HybridMutex::new(Vec::new()),
          h3_dialing: AtomicBool::new(false),
          alt: crate::sync::Mutex::new(Alt::default()),
        })
      })
      .clone()
  }

  fn usable(&self, conn: &Conn) -> bool {
    conn.usable(self.inner.config.peek_after, self.inner.config.idle_timeout)
  }

  /// Takes a slot for a new connection to `host`, if the per-host limit leaves one.
  fn reserve(&self, host: &Host) -> bool {
    let max = self.inner.config.max_per_host;
    if host.count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < max).then_some(n + 1)).is_ok() {
      host.dialing.fetch_add(1, Ordering::AcqRel);
      return true;
    }
    false
  }

  /// Where an HTTP/3 connection for `key` would go: everywhere with prior knowledge, otherwise to a fresh Alt-Svc alternative not marked broken.
  #[cfg_attr(not(feature = "http3"), allow(unused_variables))]
  fn h3_target(&self, host: &Host, key: &Key, dialer: &Dialer) -> Option<H3Target> {
    #[cfg(feature = "http3")]
    {
      let h3 = dialer.h3.as_ref()?;
      if !key.https || key.via.is_some() {
        return None;
      }
      if h3.prior_knowledge {
        return Some(H3Target { host: key.host.clone(), port: key.port, fallback: false });
      }
      if !h3.alt_svc {
        return None;
      }
      let alt = host.alt.lock();
      let now = Instant::now();
      if alt.broken_until.is_some_and(|t| now < t) {
        return None;
      }
      return alt.h3.as_ref().filter(|(_, _, expires)| now < *expires).map(|(h, p, _)| H3Target { host: h.clone(), port: *p, fallback: true });
    }
    #[cfg(not(feature = "http3"))]
    None
  }

  fn find(&self, host: &Arc<Host>, shared: &mut Vec<Arc<Conn>>, h3: Option<H3Target>) -> Found {
    shared.retain(|c| {
      let keep = !c.is_closed();
      if !keep {
        host.count.fetch_sub(1, Ordering::AcqRel);
      }
      keep
    });
    if let Some(conn) = shared.iter().find(|c| c.proto == Proto::H3 && c.has_room()) {
      return Found::Lease(Lease { conn: conn.clone(), host: host.clone(), reused: true });
    }
    if let Some(target) = h3 {
      let first = !shared.iter().any(|c| c.proto == Proto::H3) && !host.h3_dialing.load(Ordering::Acquire);
      let another = !target.fallback && host.dialing.load(Ordering::Acquire) == 0;
      if (first || another) && self.reserve(host) {
        host.h3_dialing.store(true, Ordering::Release);
        return Found::DialH3(target);
      }
      if !target.fallback {
        return Found::Wait;
      }
    }
    if let Some(conn) = shared.iter().find(|c| c.has_room()) {
      return Found::Lease(Lease { conn: conn.clone(), host: host.clone(), reused: true });
    }
    while let Ok(conn) = host.idle_rx.try_recv() {
      if self.usable(&conn) {
        return Found::Lease(Lease { conn, host: host.clone(), reused: true });
      }
      host.count.fetch_sub(1, Ordering::AcqRel);
    }
    if host.proto.load(Ordering::Acquire) != 1 && host.dialing.load(Ordering::Acquire) > 0 {
      return Found::Wait;
    }
    if self.reserve(host) {
      return Found::Dial;
    }
    Found::Wait
  }

  fn dialed(&self, host: &Arc<Host>, result: Result<Arc<Conn>, Error>) -> Result<Lease, Error> {
    let lease = match result {
      Ok(conn) => {
        self.inner.opened.fetch_add(1, Ordering::Relaxed);
        let proto = match conn.proto {
          Proto::H1 => 1,
          Proto::H2 => 2,
          Proto::H3 => 3,
        };
        host.proto.store(proto, Ordering::Release);
        if conn.proto != Proto::H1 {
          host.shared.lock().push(conn.clone());
        }
        Ok(Lease { conn, host: host.clone(), reused: false })
      }
      Err(e) => {
        host.count.fetch_sub(1, Ordering::AcqRel);
        Err(e)
      }
    };
    host.dialing.fetch_sub(1, Ordering::AcqRel);
    host.notify.notify_all();
    lease
  }

  /// Ends a QUIC dial. A failed one learnt from Alt-Svc marks the alternative broken and gives `None`, so the caller looks again and goes over TCP.
  #[cfg(feature = "http3")]
  fn dialed_h3(&self, host: &Arc<Host>, target: &H3Target, result: Result<Arc<Conn>, Error>) -> Option<Result<Lease, Error>> {
    host.h3_dialing.store(false, Ordering::Release);
    match result {
      Ok(conn) => {
        host.alt.lock().failures = 0;
        Some(self.dialed(host, Ok(conn)))
      }
      Err(e) if !target.fallback => Some(self.dialed(host, Err(e))),
      Err(_) => {
        {
          let mut alt = host.alt.lock();
          alt.broken_until = Some(Instant::now() + H3_BROKEN * (1u32 << alt.failures.min(6)));
          alt.failures += 1;
        }
        host.count.fetch_sub(1, Ordering::AcqRel);
        host.dialing.fetch_sub(1, Ordering::AcqRel);
        host.notify.notify_all();
        None
      }
    }
  }

  /// Records the `Alt-Svc` values of a response from `key`'s origin.
  #[cfg(feature = "http3")]
  pub(crate) fn alt_svc(&self, key: &Key, headers: &http::HeaderMap) {
    if !key.https || key.via.is_some() {
      return;
    }
    let mut values = headers.get_all(http::header::ALT_SVC).iter().filter_map(|v| v.to_str().ok()).peekable();
    if values.peek().is_none() {
      return;
    }
    let host = self.host(key);
    let mut alt = host.alt.lock();
    for value in values {
      match crate::altsvc::parse(value) {
        crate::altsvc::AltSvc::Clear => alt.h3 = None,
        crate::altsvc::AltSvc::H3 { host: h, port, max_age } => {
          let h = if h.is_empty() { key.host.clone() } else { h };
          alt.h3 = Some((h, port, Instant::now() + max_age));
        }
        crate::altsvc::AltSvc::Other => {}
      }
    }
  }

  fn limit(deadline: Option<Instant>) -> Result<Duration, Error> {
    match deadline {
      Some(d) => {
        let now = Instant::now();
        if now >= d {
          return Err(Error::Timeout);
        }
        Ok((d - now).min(WAIT_LIMIT))
      }
      None => Ok(WAIT_LIMIT),
    }
  }

  pub(crate) fn checkout_sync(&self, key: &Key, dialer: &Dialer, deadline: Option<Instant>) -> Result<Lease, Error> {
    let host = self.host(key);
    loop {
      let seen = host.notify.epoch();
      let h3 = self.h3_target(&host, key, dialer);
      let found = {
        let mut shared = host.shared.lock();
        self.find(&host, &mut shared, h3)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_sync(dialer, key, deadline)),
        #[cfg(feature = "http3")]
        Found::DialH3(target) => {
          if let Some(r) = self.dialed_h3(&host, &target, dial::dial_h3_sync(dialer, key, &target, deadline)) {
            return r;
          }
        }
        #[cfg(not(feature = "http3"))]
        Found::DialH3(_) => unreachable!("no HTTP/3 target without the http3 feature"),
        Found::Wait => host.notify.wait_sync(seen, Self::limit(deadline)?),
      }
    }
  }

  pub(crate) async fn checkout_async(&self, key: &Key, dialer: &Dialer, deadline: Option<Instant>) -> Result<Lease, Error> {
    let host = self.host(key);
    loop {
      let seen = host.notify.epoch();
      let h3 = self.h3_target(&host, key, dialer);
      let found = {
        let mut shared = host.shared.lock_async().await;
        self.find(&host, &mut shared, h3)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_async(dialer, key, deadline).await),
        #[cfg(feature = "http3")]
        Found::DialH3(target) => {
          if let Some(r) = self.dialed_h3(&host, &target, dial::dial_h3_async(dialer, key, &target, deadline).await) {
            return r;
          }
        }
        #[cfg(not(feature = "http3"))]
        Found::DialH3(_) => unreachable!("no HTTP/3 target without the http3 feature"),
        Found::Wait => host.notify.wait_async(seen, Self::limit(deadline)?).await,
      }
    }
  }
}
