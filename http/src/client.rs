//! `SyncClient` and `AsyncClient`: two handles to one client, switched with `to_sync` and `to_async`.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{CONTENT_LENGTH, HeaderName, HeaderValue, USER_AGENT};
use http::{HeaderMap, Method, Uri, Version};

use crate::Error;
use crate::body::{AsyncBody, BodyState, SyncBody};
use crate::conn::Proto;
use crate::dial::Dialer;
use crate::engine::{Event, OpenError, RequestHead};
use crate::pool::{Key, Lease, Pool};
use crate::response::Response;

const DEFAULT_USER_AGENT: &str = concat!("fibre_http/", env!("CARGO_PKG_VERSION"));
const ATTEMPTS: usize = 3;

struct Inner {
  pool: Pool,
  dialer: Dialer,
  timeout: Option<Duration>,
  user_agent: HeaderValue,
}

pub struct ClientBuilder {
  pool: Option<Pool>,
  timeout: Option<Duration>,
  user_agent: HeaderValue,
  h2_prior_knowledge: bool,
  #[cfg(feature = "rustls")]
  roots: Vec<rustls::pki_types::CertificateDer<'static>>,
  #[cfg(feature = "rustls")]
  bundled_roots: bool,
}

impl Default for ClientBuilder {
  fn default() -> Self {
    Self {
      pool: None,
      timeout: None,
      user_agent: HeaderValue::from_static(DEFAULT_USER_AGENT),
      h2_prior_knowledge: false,
      #[cfg(feature = "rustls")]
      roots: Vec::new(),
      #[cfg(feature = "rustls")]
      bundled_roots: true,
    }
  }
}

impl ClientBuilder {
  /// Shares `pool` with every other client built from it.
  pub fn pool(mut self, pool: &Pool) -> Self {
    self.pool = Some(pool.clone());
    self
  }

  /// Deadline for a whole request, from checkout to the response head. Bodies keep the same deadline.
  pub fn timeout(mut self, timeout: Duration) -> Self {
    self.timeout = Some(timeout);
    self
  }

  pub fn user_agent(mut self, value: &str) -> Result<Self, Error> {
    self.user_agent = HeaderValue::from_str(value).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(self)
  }

  /// Speaks HTTP/2 without TLS (h2c) to `http://` origins, with no upgrade.
  pub fn http2_prior_knowledge(mut self) -> Self {
    self.h2_prior_knowledge = true;
    self
  }

  /// Trusts `der` as a root certificate, beside the bundled roots.
  #[cfg(feature = "rustls")]
  pub fn add_root_certificate(mut self, der: Vec<u8>) -> Self {
    self.roots.push(rustls::pki_types::CertificateDer::from(der));
    self
  }

  /// Trusts only the roots added with `add_root_certificate`.
  #[cfg(feature = "rustls")]
  pub fn no_bundled_roots(mut self) -> Self {
    self.bundled_roots = false;
    self
  }

  fn inner(self) -> Result<Arc<Inner>, Error> {
    #[cfg(feature = "rustls")]
    let tls = {
      let mut store = rustls::RootCertStore::empty();
      #[cfg(feature = "webpki-roots")]
      if self.bundled_roots {
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
      }
      for der in self.roots {
        store.add(der).map_err(|e| Error::Tls(e.to_string()))?;
      }
      let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?
        .with_root_certificates(store)
        .with_no_client_auth();
      let mut alpn = Vec::new();
      #[cfg(feature = "http2")]
      alpn.push(b"h2".to_vec());
      #[cfg(feature = "http1")]
      alpn.push(b"http/1.1".to_vec());
      config.alpn_protocols = alpn;
      Some(Arc::new(config))
    };
    Ok(Arc::new(Inner {
      pool: self.pool.unwrap_or_default(),
      dialer: Dialer {
        #[cfg(feature = "rustls")]
        tls,
        h2_prior_knowledge: self.h2_prior_knowledge,
      },
      timeout: self.timeout,
      user_agent: self.user_agent,
    }))
  }

  pub fn build_sync(self) -> Result<SyncClient, Error> {
    Ok(SyncClient { inner: self.inner()? })
  }

  pub fn build_async(self) -> Result<AsyncClient, Error> {
    Ok(AsyncClient { inner: self.inner()? })
  }
}

#[derive(Clone)]
pub struct SyncClient {
  inner: Arc<Inner>,
}

#[derive(Clone)]
pub struct AsyncClient {
  inner: Arc<Inner>,
}

pub struct Blocking;
pub struct Async;

macro_rules! verbs {
  ($mode:ty) => {
    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::GET, url)
    }
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::POST, url)
    }
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::PUT, url)
    }
    pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::PATCH, url)
    }
    pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::DELETE, url)
    }
    pub fn head(&self, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      self.request(Method::HEAD, url)
    }
    pub fn request(&self, method: Method, url: impl AsRef<str>) -> RequestBuilder<$mode> {
      RequestBuilder::new(self.inner.clone(), method, url.as_ref())
    }
  };
}

impl SyncClient {
  pub fn new() -> Self {
    ClientBuilder::default().build_sync().expect("default client configuration is valid")
  }

  pub fn builder() -> ClientBuilder {
    ClientBuilder::default()
  }

  pub fn to_async(self) -> AsyncClient {
    AsyncClient { inner: self.inner }
  }

  verbs!(Blocking);
}

impl Default for SyncClient {
  fn default() -> Self {
    Self::new()
  }
}

impl AsyncClient {
  pub fn new() -> Self {
    ClientBuilder::default().build_async().expect("default client configuration is valid")
  }

  pub fn builder() -> ClientBuilder {
    ClientBuilder::default()
  }

  pub fn to_sync(self) -> SyncClient {
    SyncClient { inner: self.inner }
  }

  verbs!(Async);
}

impl Default for AsyncClient {
  fn default() -> Self {
    Self::new()
  }
}

pub struct RequestBuilder<M> {
  inner: Arc<Inner>,
  method: Method,
  uri: Result<Uri, Error>,
  headers: HeaderMap,
  body: Bytes,
  timeout: Option<Duration>,
  error: Option<Error>,
  _mode: PhantomData<M>,
}

impl<M> RequestBuilder<M> {
  fn new(inner: Arc<Inner>, method: Method, url: &str) -> Self {
    let uri = url.parse::<Uri>().map_err(|e| Error::Url(e.to_string()));
    let timeout = inner.timeout;
    Self { inner, method, uri, headers: HeaderMap::new(), body: Bytes::new(), timeout, error: None, _mode: PhantomData }
  }

  pub fn header(mut self, name: &str, value: &str) -> Self {
    match (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
      (Ok(n), Ok(v)) => {
        self.headers.append(n, v);
      }
      _ => self.error = Some(Error::Protocol(format!("invalid header {name}"))),
    }
    self
  }

  pub fn body(mut self, body: impl Into<Bytes>) -> Self {
    self.body = body.into();
    self
  }

  #[cfg(feature = "json")]
  pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
    match serde_json::to_vec(value) {
      Ok(v) => {
        self.body = Bytes::from(v);
        self.headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
      }
      Err(e) => self.error = Some(Error::Decode(e.to_string())),
    }
    self
  }

  pub fn timeout(mut self, timeout: Duration) -> Self {
    self.timeout = Some(timeout);
    self
  }

  fn prepare(self) -> Result<Prepared, Error> {
    if let Some(e) = self.error {
      return Err(e);
    }
    let uri = self.uri?;
    let key = Key::from_uri(&uri)?;
    let mut headers = self.headers;
    if !headers.contains_key(USER_AGENT) {
      headers.insert(USER_AGENT, self.inner.user_agent.clone());
    }
    let sends_body = !self.body.is_empty() || matches!(self.method, Method::POST | Method::PUT | Method::PATCH);
    if sends_body && !headers.contains_key(CONTENT_LENGTH) {
      headers.insert(CONTENT_LENGTH, HeaderValue::from(self.body.len()));
    }
    let deadline = self.timeout.map(|t| Instant::now() + t);
    Ok(Prepared { inner: self.inner, method: self.method, uri, headers, body: self.body, sends_body, key, deadline })
  }
}

struct Prepared {
  inner: Arc<Inner>,
  method: Method,
  uri: Uri,
  headers: HeaderMap,
  body: Bytes,
  sends_body: bool,
  key: Key,
  deadline: Option<Instant>,
}

impl Prepared {
  fn head(&self, proto: Proto) -> RequestHead {
    let version = if proto == Proto::H2 { Version::HTTP_2 } else { Version::HTTP_11 };
    RequestHead { method: self.method.clone(), uri: self.uri.clone(), version, headers: self.headers.clone() }
  }

  fn idempotent(&self) -> bool {
    matches!(self.method, Method::GET | Method::HEAD | Method::OPTIONS | Method::PUT | Method::DELETE | Method::TRACE)
  }

  /// Whether a failed attempt may be sent again on another connection.
  fn retry(&self, error: &Error, lease: &Lease, saw_head: bool) -> bool {
    if saw_head {
      return false;
    }
    match error {
      Error::Refused => true,
      Error::Closed | Error::Io(_) => lease.reused && self.idempotent(),
      _ => false,
    }
  }
}

enum Attempt {
  Done(Response<BodyState>),
  Retry,
  Fail(Error),
}

impl RequestBuilder<Blocking> {
  pub fn send(self) -> Result<Response<SyncBody>, Error> {
    let req = self.prepare()?;
    for _ in 0..ATTEMPTS {
      let lease = req.inner.pool.checkout_sync(&req.key, &req.inner.dialer, req.deadline)?;
      match attempt_sync(&req, lease) {
        Attempt::Done(r) => return Ok(r.map_body(SyncBody::from_state)),
        Attempt::Retry => continue,
        Attempt::Fail(e) => return Err(e),
      }
    }
    Err(Error::Closed)
  }
}

impl RequestBuilder<Async> {
  pub async fn send(self) -> Result<Response<AsyncBody>, Error> {
    let req = self.prepare()?;
    for _ in 0..ATTEMPTS {
      let lease = req.inner.pool.checkout_async(&req.key, &req.inner.dialer, req.deadline).await?;
      match attempt_async(&req, lease).await {
        Attempt::Done(r) => return Ok(r.map_body(AsyncBody::from_state)),
        Attempt::Retry => continue,
        Attempt::Fail(e) => return Err(e),
      }
    }
    Err(Error::Closed)
  }
}

fn opened(req: &Prepared, lease: Lease, result: Result<crate::engine::ExchangeId, OpenError>) -> Result<(Lease, crate::engine::ExchangeId), Attempt> {
  match result {
    Ok(id) => Ok((lease, id)),
    Err(OpenError::Full) | Err(OpenError::GoingAway) => {
      lease.release();
      Err(Attempt::Retry)
    }
    Err(_) => {
      let retry = lease.reused || !req.sends_body;
      lease.release();
      Err(if retry { Attempt::Retry } else { Attempt::Fail(Error::Closed) })
    }
  }
}

fn on_event(req: &Prepared, lease: Lease, id: crate::engine::ExchangeId, event: Result<Event, Error>) -> Result<(Lease, Option<Attempt>), Attempt> {
  match event {
    Ok(Event::Head(_)) => unreachable!("the caller handles the head"),
    Ok(Event::Informational(_)) | Ok(Event::Data(_)) | Ok(Event::Trailers(_)) | Ok(Event::Writable) => Ok((lease, None)),
    Ok(Event::End) => {
      lease.release();
      Err(Attempt::Fail(Error::Protocol("response ended before its head".into())))
    }
    Ok(Event::Reset(e)) | Err(e) => {
      let retry = req.retry(&e, &lease, false);
      if !matches!(e, Error::Timeout) {
        lease.conn.reset(id);
      }
      lease.release();
      Err(if retry { Attempt::Retry } else { Attempt::Fail(e) })
    }
  }
}

fn attempt_sync(req: &Prepared, lease: Lease) -> Attempt {
  let head = req.head(lease.conn.proto);
  let result = lease.conn.open_sync(head);
  let (lease, id) = match opened(req, lease, result) {
    Ok(pair) => pair,
    Err(a) => return a,
  };
  if req.sends_body {
    if let Err(e) = lease.conn.send_body_sync(id, req.body.clone(), req.deadline) {
      let retry = req.retry(&e, &lease, false);
      lease.conn.reset(id);
      lease.release();
      return if retry { Attempt::Retry } else { Attempt::Fail(e) };
    }
  }
  let mut lease = lease;
  loop {
    let event = lease.conn.next_event_sync(id, req.deadline);
    if let Ok(Event::Head(_)) = &event {
      let Ok(Event::Head(head)) = event else { unreachable!() };
      let state = BodyState::new(lease, id, req.deadline, req.method == Method::HEAD);
      return Attempt::Done(Response::new(head.status, head.version, head.headers, state));
    }
    match on_event(req, lease, id, event) {
      Ok((l, None)) => lease = l,
      Ok((_, Some(a))) => return a,
      Err(a) => return a,
    }
  }
}

async fn attempt_async(req: &Prepared, lease: Lease) -> Attempt {
  let head = req.head(lease.conn.proto);
  let result = lease.conn.open_async(head).await;
  let (lease, id) = match opened(req, lease, result) {
    Ok(pair) => pair,
    Err(a) => return a,
  };
  if req.sends_body {
    if let Err(e) = lease.conn.send_body_async(id, req.body.clone(), req.deadline).await {
      let retry = req.retry(&e, &lease, false);
      lease.conn.reset(id);
      lease.release();
      return if retry { Attempt::Retry } else { Attempt::Fail(e) };
    }
  }
  let mut lease = lease;
  loop {
    let event = lease.conn.next_event_async(id, req.deadline).await;
    if let Ok(Event::Head(_)) = &event {
      let Ok(Event::Head(head)) = event else { unreachable!() };
      let state = BodyState::new(lease, id, req.deadline, req.method == Method::HEAD);
      return Attempt::Done(Response::new(head.status, head.version, head.headers, state));
    }
    match on_event(req, lease, id, event) {
      Ok((l, None)) => lease = l,
      Ok((_, Some(a))) => return a,
      Err(a) => return a,
    }
  }
}
