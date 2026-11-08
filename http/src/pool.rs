//! Connections by origin. Idle HTTP/1 connections wait in a bounded fibre channel that blocking and async callers both receive from. HTTP/2 connections are shared while they have room.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fibre::mpmc;
use fibre::sync::HybridMutex;

use crate::Error;
use crate::conn::{Conn, Proto};
use crate::dial::{self, Dialer};
use crate::notify::Notify;

/// The longest a waiter sleeps before looking again even without a notify.
const WAIT_LIMIT: Duration = Duration::from_millis(100);

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
  /// What the last dial to this origin agreed: 0 not known yet, 1 HTTP/1.1, 2 HTTP/2.
  proto: AtomicU8,
  shared: HybridMutex<Vec<Arc<Conn>>>,
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
        Arc::new(Host { idle_tx, idle_rx, notify: Notify::default(), count: AtomicUsize::new(0), dialing: AtomicUsize::new(0), proto: AtomicU8::new(0), shared: HybridMutex::new(Vec::new()) })
      })
      .clone()
  }

  fn usable(&self, conn: &Conn) -> bool {
    conn.usable(self.inner.config.peek_after, self.inner.config.idle_timeout)
  }

  fn find(&self, host: &Arc<Host>, shared: &mut Vec<Arc<Conn>>) -> Found {
    shared.retain(|c| {
      let keep = !c.is_closed();
      if !keep {
        host.count.fetch_sub(1, Ordering::AcqRel);
      }
      keep
    });
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
    let max = self.inner.config.max_per_host;
    if host.count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < max).then_some(n + 1)).is_ok() {
      host.dialing.fetch_add(1, Ordering::AcqRel);
      return Found::Dial;
    }
    Found::Wait
  }

  fn dialed(&self, host: &Arc<Host>, result: Result<Arc<Conn>, Error>) -> Result<Lease, Error> {
    let lease = match result {
      Ok(conn) => {
        self.inner.opened.fetch_add(1, Ordering::Relaxed);
        host.proto.store(if conn.proto == Proto::H2 { 2 } else { 1 }, Ordering::Release);
        if conn.proto == Proto::H2 {
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
      let found = {
        let mut shared = host.shared.lock();
        self.find(&host, &mut shared)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_sync(dialer, key, deadline)),
        Found::Wait => host.notify.wait_sync(seen, Self::limit(deadline)?),
      }
    }
  }

  pub(crate) async fn checkout_async(&self, key: &Key, dialer: &Dialer, deadline: Option<Instant>) -> Result<Lease, Error> {
    let host = self.host(key);
    loop {
      let seen = host.notify.epoch();
      let found = {
        let mut shared = host.shared.lock_async().await;
        self.find(&host, &mut shared)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_async(dialer, key, deadline).await),
        Found::Wait => host.notify.wait_async(seen, Self::limit(deadline)?).await,
      }
    }
  }
}
