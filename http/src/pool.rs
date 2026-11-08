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

const WAIT_SLICE: Duration = Duration::from_millis(50);
const H2_SLICE: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
  pub https: bool,
  pub host: String,
  pub port: u16,
}

impl Key {
  pub(crate) fn from_uri(uri: &http::Uri) -> Result<Key, Error> {
    let https = match uri.scheme_str() {
      Some("https") => true,
      Some("http") => false,
      other => return Err(Error::Url(format!("unsupported scheme {other:?}"))),
    };
    let host = uri.host().ok_or_else(|| Error::Url("no host".into()))?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
    Ok(Key { https, host, port })
  }
}

struct Host {
  idle_tx: mpmc::Sender<Arc<Conn>>,
  idle_rx: mpmc::Receiver<Arc<Conn>>,
  idle_rx_async: mpmc::AsyncReceiver<Arc<Conn>>,
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
    Pool { inner: Arc::new(PoolInner { hosts: HybridMutex::new(HashMap::new()), config: self }) }
  }
}

struct PoolInner {
  hosts: HybridMutex<HashMap<Key, Arc<Host>>>,
  config: PoolBuilder,
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
    if self.conn.proto == Proto::H2 {
      return;
    }
    if self.conn.is_closed() {
      self.host.count.fetch_sub(1, Ordering::AcqRel);
      return;
    }
    self.conn.mark_idle();
    if self.host.idle_tx.try_send(self.conn).is_err() {
      self.host.count.fetch_sub(1, Ordering::AcqRel);
    }
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

  fn host(&self, key: &Key) -> Arc<Host> {
    let mut hosts = self.inner.hosts.lock();
    hosts
      .entry(key.clone())
      .or_insert_with(|| {
        let (idle_tx, idle_rx) = mpmc::bounded(self.inner.config.max_per_host);
        let idle_rx_async = idle_rx.clone().to_async();
        Arc::new(Host { idle_tx, idle_rx, idle_rx_async, count: AtomicUsize::new(0), dialing: AtomicUsize::new(0), proto: AtomicU8::new(0), shared: HybridMutex::new(Vec::new()) })
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
    lease
  }

  /// How long a waiter sleeps before looking again. Short while the origin may be HTTP/2, since a new HTTP/2 connection does not arrive through the idle channel.
  fn slice(&self, host: &Host, deadline: Option<Instant>) -> Result<(Duration, bool), Error> {
    let h2_wait = host.proto.load(Ordering::Acquire) != 1;
    let slice = if h2_wait { H2_SLICE } else { WAIT_SLICE };
    match deadline {
      Some(d) => {
        let now = Instant::now();
        if now >= d {
          return Err(Error::Timeout);
        }
        Ok(((d - now).min(slice), h2_wait))
      }
      None => Ok((slice, h2_wait)),
    }
  }

  pub(crate) fn checkout_sync(&self, key: &Key, dialer: &Dialer, deadline: Option<Instant>) -> Result<Lease, Error> {
    let host = self.host(key);
    loop {
      let found = {
        let mut shared = host.shared.lock();
        self.find(&host, &mut shared)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_sync(dialer, key, deadline)),
        Found::Wait => {
          let (slice, h2_wait) = self.slice(&host, deadline)?;
          if h2_wait {
            std::thread::sleep(slice);
            continue;
          }
          if let Ok(conn) = host.idle_rx.recv_timeout(slice) {
            if self.usable(&conn) {
              return Ok(Lease { conn, host: host.clone(), reused: true });
            }
            host.count.fetch_sub(1, Ordering::AcqRel);
          }
        }
      }
    }
  }

  pub(crate) async fn checkout_async(&self, key: &Key, dialer: &Dialer, deadline: Option<Instant>) -> Result<Lease, Error> {
    let host = self.host(key);
    loop {
      let found = {
        let mut shared = host.shared.lock_async().await;
        self.find(&host, &mut shared)
      };
      match found {
        Found::Lease(lease) => return Ok(lease),
        Found::Dial => return self.dialed(&host, dial::dial_async(dialer, key, deadline).await),
        Found::Wait => {
          let (slice, h2_wait) = self.slice(&host, deadline)?;
          if h2_wait {
            tokio::time::sleep(slice).await;
            continue;
          }
          if let Ok(Ok(conn)) = tokio::time::timeout(slice, host.idle_rx_async.recv()).await {
            if self.usable(&conn) {
              return Ok(Lease { conn, host: host.clone(), reused: true });
            }
            host.count.fetch_sub(1, Ordering::AcqRel);
          }
        }
      }
    }
  }
}
