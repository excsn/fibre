//! `SyncClient` and `AsyncClient`: two handles to one client, switched with `to_sync` and `to_async`.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{CONTENT_LENGTH, HeaderName, HeaderValue, USER_AGENT};
use http::{HeaderMap, Method, StatusCode, Uri, Version};

use crate::Error;
use crate::body::{AsyncBody, BodyState, SyncBody};
use crate::conn::Proto;
use crate::dial::Dialer;
use crate::engine::{Event, OpenError, RequestHead};
use crate::pool::{Key, Lease, Pool};
use crate::response::Response;

const DEFAULT_USER_AGENT: &str = concat!("fibre_http/", env!("CARGO_PKG_VERSION"));
const ATTEMPTS: usize = 3;
const DEFAULT_REDIRECTS: usize = 10;
const DEFAULT_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);
const BODY_CHUNK: usize = 64 * 1024;

type BodyReader = Box<dyn std::io::Read + Send>;
type BodyStream = std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// A request body. A reader or a stream is read once, so a request carrying one is not sent again.
#[derive(Clone)]
enum ReqBody {
  Full(Bytes),
  Reader(Arc<parking_lot::Mutex<Option<BodyReader>>>, Option<u64>),
  Stream(Arc<parking_lot::Mutex<Option<BodyStream>>>, Option<u64>),
}

impl ReqBody {
  fn replayable(&self) -> bool {
    matches!(self, ReqBody::Full(_))
  }

  fn length(&self) -> Option<u64> {
    match self {
      ReqBody::Full(b) => Some(b.len() as u64),
      ReqBody::Reader(_, n) | ReqBody::Stream(_, n) => *n,
    }
  }
}
/// A redirect's own body is read up to this much so its connection can be reused; a longer one is dropped.
const REDIRECT_DRAIN: usize = 64 * 1024;

struct Inner {
  pool: Pool,
  dialer: Dialer,
  timeout: Option<Duration>,
  user_agent: HeaderValue,
  redirects: usize,
  decompress: bool,
  default_headers: HeaderMap,
  max_response_size: Option<usize>,
  #[cfg(feature = "cookies")]
  cookies: Option<Arc<parking_lot::Mutex<cookie_store::CookieStore>>>,
}

pub struct ClientBuilder {
  pool: Option<Pool>,
  timeout: Option<Duration>,
  redirects: usize,
  decompress: bool,
  default_headers: HeaderMap,
  max_response_size: Option<usize>,
  /// `None` reads the environment when the client is built.
  proxies: Option<crate::proxy::Proxies>,
  #[cfg(feature = "cookies")]
  cookie_store: bool,
  user_agent: HeaderValue,
  h2_prior_knowledge: bool,
  h2_keep_alive_interval: Option<Duration>,
  h2_keep_alive_timeout: Duration,
  roots: Vec<Vec<u8>>,
  identity: Option<(Vec<u8>, Vec<u8>)>,
  bundled_roots: bool,
  #[cfg(feature = "native-tls")]
  native_tls: bool,
}

impl Default for ClientBuilder {
  fn default() -> Self {
    Self {
      pool: None,
      timeout: None,
      redirects: DEFAULT_REDIRECTS,
      decompress: true,
      default_headers: HeaderMap::new(),
      max_response_size: None,
      proxies: None,
      #[cfg(feature = "cookies")]
      cookie_store: false,
      user_agent: HeaderValue::from_static(DEFAULT_USER_AGENT),
      h2_prior_knowledge: false,
      h2_keep_alive_interval: None,
      h2_keep_alive_timeout: DEFAULT_KEEP_ALIVE_TIMEOUT,
      roots: Vec::new(),
      identity: None,
      bundled_roots: true,
      #[cfg(feature = "native-tls")]
      native_tls: !cfg!(feature = "rustls"),
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

  /// The most redirects a request follows, 10 by default. 0 hands every redirect back as the response.
  pub fn redirects(mut self, n: usize) -> Self {
    self.redirects = n;
    self
  }

  /// Keeps cookies from `Set-Cookie` in memory and sends them on later requests that match.
  #[cfg(feature = "cookies")]
  pub fn cookie_store(mut self, on: bool) -> Self {
    self.cookie_store = on;
    self
  }

  /// Sends every request through `proxy`, in place of the proxies from the environment.
  pub fn proxy(mut self, proxy: crate::proxy::Proxy) -> Self {
    let mut p = self.proxies.take().unwrap_or_default();
    p.http = Some(proxy.clone());
    p.https = Some(proxy);
    self.proxies = Some(p);
    self
  }

  /// Hosts that skip the proxy, in `NO_PROXY` form: `a.test, .b.test, 10.0.0.1, *`.
  pub fn no_proxy_for(mut self, list: &str) -> Self {
    let mut p = self.proxies.take().unwrap_or_default();
    p.no_proxy = crate::proxy::NoProxy::parse(list);
    self.proxies = Some(p);
    self
  }

  /// No proxy, whatever the environment says.
  pub fn no_proxy(mut self) -> Self {
    self.proxies = Some(crate::proxy::Proxies::default());
    self
  }

  /// The most bytes a response body may decode to. A longer one fails with `Error::TooLarge`.
  pub fn max_response_size(mut self, bytes: usize) -> Self {
    self.max_response_size = Some(bytes);
    self
  }

  /// Headers every request carries. A request header of the same name replaces them.
  pub fn default_headers(mut self, headers: HeaderMap) -> Self {
    self.default_headers = headers;
    self
  }

  /// Whether gzip and zstd responses are decoded, on by default. Off, `Accept-Encoding` is not sent and bodies arrive as the server encoded them.
  pub fn decompress(mut self, on: bool) -> Self {
    self.decompress = on;
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

  /// Sends an HTTP/2 PING after `interval` with nothing read while a request is in flight, off by default. A connection idle in the pool is not pinged.
  pub fn http2_keep_alive_interval(mut self, interval: Duration) -> Self {
    self.h2_keep_alive_interval = Some(interval);
    self
  }

  /// How long to wait for the PONG before the connection is closed and its requests fail with `Error::Timeout`, 20 s by default.
  pub fn http2_keep_alive_timeout(mut self, timeout: Duration) -> Self {
    self.h2_keep_alive_timeout = timeout;
    self
  }

  /// Trusts `der` as a root certificate, beside the bundled roots.
  pub fn add_root_certificate(mut self, der: Vec<u8>) -> Self {
    self.roots.push(der);
    self
  }

  /// Presents `cert_chain_pem` and its `key_pem` when a server asks for a client certificate: mutual TLS. A chain or key that does not parse fails the build with `Error::Tls`.
  pub fn identity(mut self, cert_chain_pem: Vec<u8>, key_pem: Vec<u8>) -> Self {
    self.identity = Some((cert_chain_pem, key_pem));
    self
  }

  /// HTTPS through the platform's TLS (Security.framework, SChannel or OpenSSL) instead of rustls.
  #[cfg(feature = "native-tls")]
  pub fn use_native_tls(mut self) -> Self {
    self.native_tls = true;
    self
  }

  /// Trusts only the roots added with `add_root_certificate`, leaving out the bundled roots and, with `native-roots` or native-tls, the OS store.
  pub fn no_bundled_roots(mut self) -> Self {
    self.bundled_roots = false;
    self
  }

  fn inner(self) -> Result<Arc<Inner>, Error> {
    #[cfg(feature = "native-tls")]
    let native = match self.native_tls {
      false => None,
      true => {
        let mut builder = native_tls::TlsConnector::builder();
        #[allow(unused_mut)]
        let mut alpn: Vec<&str> = Vec::new();
        #[cfg(feature = "http2")]
        alpn.push("h2");
        #[cfg(feature = "http1")]
        alpn.push("http/1.1");
        builder.request_alpns(&alpn);
        for der in &self.roots {
          builder.add_root_certificate(native_tls::Certificate::from_der(der).map_err(|e| Error::Tls(e.to_string()))?);
        }
        builder.disable_built_in_roots(!self.bundled_roots);
        if let Some((chain, key)) = &self.identity {
          builder.identity(native_tls::Identity::from_pkcs8(chain, key).map_err(|e| Error::Tls(e.to_string()))?);
        }
        Some(builder.build().map_err(|e| Error::Tls(e.to_string()))?)
      }
    };
    #[cfg(feature = "rustls")]
    let tls = {
      let mut store = rustls::RootCertStore::empty();
      #[cfg(feature = "webpki-roots")]
      if self.bundled_roots {
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
      }
      #[cfg(feature = "native-roots")]
      if self.bundled_roots {
        for der in rustls_native_certs::load_native_certs().certs {
          let _ = store.add(der);
        }
      }
      for der in &self.roots {
        store.add(rustls::pki_types::CertificateDer::from(der.clone())).map_err(|e| Error::Tls(e.to_string()))?;
      }
      let builder = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?
        .with_root_certificates(store);
      let mut config = match &self.identity {
        None => builder.with_no_client_auth(),
        Some((chain, key)) => {
          use rustls::pki_types::pem::PemObject;
          let chain = rustls::pki_types::CertificateDer::pem_slice_iter(chain).collect::<Result<Vec<_>, _>>().map_err(|e| Error::Tls(format!("client certificate chain: {e}")))?;
          if chain.is_empty() {
            return Err(Error::Tls("client certificate chain: no certificate".into()));
          }
          let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(key).map_err(|e| Error::Tls(format!("client key: {e}")))?;
          builder.with_client_auth_cert(chain, key).map_err(|e| Error::Tls(e.to_string()))?
        }
      };
      #[allow(unused_mut)]
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
        #[cfg(feature = "native-tls")]
        native,
        h2_prior_knowledge: self.h2_prior_knowledge,
        h2_keep_alive: self.h2_keep_alive_interval.map(|interval| (interval, self.h2_keep_alive_timeout)),
        proxies: self.proxies.unwrap_or_else(crate::proxy::Proxies::from_env),
        user_agent: self.user_agent.to_str().unwrap_or(DEFAULT_USER_AGENT).to_owned(),
      },
      timeout: self.timeout,
      user_agent: self.user_agent,
      redirects: self.redirects,
      decompress: self.decompress,
      default_headers: self.default_headers,
      max_response_size: self.max_response_size,
      #[cfg(feature = "cookies")]
      cookies: self.cookie_store.then(|| Arc::new(parking_lot::Mutex::new(cookie_store::CookieStore::default()))),
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
  body: ReqBody,
  timeout: Option<Duration>,
  error: Option<Error>,
  _mode: PhantomData<M>,
}

impl<M> RequestBuilder<M> {
  fn new(inner: Arc<Inner>, method: Method, url: &str) -> Self {
    let uri = url.parse::<Uri>().map_err(|e| Error::Url(e.to_string()));
    let timeout = inner.timeout;
    Self { inner, method, uri, headers: HeaderMap::new(), body: ReqBody::Full(Bytes::new()), timeout, error: None, _mode: PhantomData }
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

  /// Appends `pairs` to the URL's query, form-encoded.
  pub fn query<K: AsRef<str>, V: AsRef<str>>(mut self, pairs: &[(K, V)]) -> Self {
    if pairs.is_empty() {
      return self;
    }
    if let Ok(uri) = &self.uri {
      let mut parts = uri.clone().into_parts();
      let (path, existing) = match parts.path_and_query.as_ref() {
        Some(pq) => (pq.path().to_owned(), pq.query().map(str::to_owned)),
        None => ("/".to_owned(), None),
      };
      let mut query = existing.unwrap_or_default();
      for (k, v) in pairs {
        if !query.is_empty() {
          query.push('&');
        }
        form_encode(k.as_ref(), &mut query);
        query.push('=');
        form_encode(v.as_ref(), &mut query);
      }
      self.uri = format!("{path}?{query}")
        .parse()
        .map_err(|e: http::uri::InvalidUri| Error::Url(e.to_string()))
        .and_then(|pq| {
          parts.path_and_query = Some(pq);
          Uri::from_parts(parts).map_err(|e| Error::Url(e.to_string()))
        });
    }
    self
  }

  /// An `application/x-www-form-urlencoded` body of `pairs`.
  pub fn form<K: AsRef<str>, V: AsRef<str>>(mut self, pairs: &[(K, V)]) -> Self {
    let mut encoded = String::new();
    for (k, v) in pairs {
      if !encoded.is_empty() {
        encoded.push('&');
      }
      form_encode(k.as_ref(), &mut encoded);
      encoded.push('=');
      form_encode(v.as_ref(), &mut encoded);
    }
    self.body = ReqBody::Full(Bytes::from(encoded));
    if !self.headers.contains_key(http::header::CONTENT_TYPE) {
      self.headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/x-www-form-urlencoded"));
    }
    self
  }

  pub fn body(mut self, body: impl Into<Bytes>) -> Self {
    self.body = ReqBody::Full(body.into());
    self
  }

  #[cfg(feature = "json")]
  pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
    match serde_json::to_vec(value) {
      Ok(v) => {
        self.body = ReqBody::Full(Bytes::from(v));
        if !self.headers.contains_key(http::header::CONTENT_TYPE) {
          self.headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
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
    let key = Key::for_target(&uri, &self.inner.dialer.proxies)?;
    let mut headers = self.headers;
    for name in self.inner.default_headers.keys() {
      if !headers.contains_key(name) {
        for value in self.inner.default_headers.get_all(name) {
          headers.append(name.clone(), value.clone());
        }
      }
    }
    if !headers.contains_key(USER_AGENT) {
      headers.insert(USER_AGENT, self.inner.user_agent.clone());
    }
    let caller_encoding = headers.contains_key(http::header::ACCEPT_ENCODING);
    if self.inner.decompress && !caller_encoding {
      if let Some(accept) = crate::decode::accept_encoding() {
        headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static(accept));
      }
    }
    let sends_body = match &self.body {
      ReqBody::Full(b) => !b.is_empty() || matches!(self.method, Method::POST | Method::PUT | Method::PATCH),
      _ => {
        if matches!(self.method, Method::GET | Method::HEAD | Method::DELETE | Method::OPTIONS) {
          return Err(Error::Protocol(format!("a streamed body needs a method that carries one, not {}", self.method)));
        }
        true
      }
    };
    if sends_body && !headers.contains_key(CONTENT_LENGTH) {
      if let Some(n) = self.body.length() {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(n));
      }
    }
    let deadline = self.timeout.map(|t| Instant::now() + t);
    let decode = self.inner.decompress && !caller_encoding;
    let caller_cookie = headers.contains_key(http::header::COOKIE);
    Ok(Prepared { inner: self.inner, method: self.method, uri, headers, body: self.body, sends_body, key, deadline, decode, caller_cookie })
  }
}

/// `application/x-www-form-urlencoded`: unreserved bytes as they are, space as `+`, the rest as `%XX`.
fn form_encode(s: &str, out: &mut String) {
  for b in s.bytes() {
    match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => out.push(b as char),
      b' ' => out.push('+'),
      _ => out.push_str(&format!("%{b:02X}")),
    }
  }
}

#[derive(Clone)]
struct Prepared {
  inner: Arc<Inner>,
  method: Method,
  uri: Uri,
  headers: HeaderMap,
  body: ReqBody,
  sends_body: bool,
  key: Key,
  deadline: Option<Instant>,
  /// Decode the response body by its `Content-Encoding`; off when the caller asked for an encoding themselves.
  decode: bool,
  /// The caller set `Cookie` themselves, so the store adds none.
  caller_cookie: bool,
}

fn is_redirect(status: StatusCode) -> bool {
  matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// Resolves a `Location` against the URL that answered with it (RFC 3986 section 5.2).
fn resolve(base: &Uri, location: &str) -> Result<Uri, Error> {
  let bad = |e: String| Error::Url(format!("redirect to {location}: {e}"));
  if location.contains("://") {
    return location.parse().map_err(|e: http::uri::InvalidUri| bad(e.to_string()));
  }
  let scheme = base.scheme_str().unwrap_or("http");
  let authority = base.authority().map(|a| a.as_str()).unwrap_or("");
  if let Some(rest) = location.strip_prefix("//") {
    return format!("{scheme}://{rest}").parse().map_err(|e: http::uri::InvalidUri| bad(e.to_string()));
  }
  let (reference, query) = match location.split_once('?') {
    Some((p, q)) => (p, Some(q)),
    None => (location, None),
  };
  let merged = if reference.starts_with('/') {
    reference.to_owned()
  } else if reference.is_empty() {
    base.path().to_owned()
  } else {
    let dir = base.path().rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    format!("{dir}/{reference}")
  };
  let mut out: Vec<&str> = Vec::new();
  for segment in merged.split('/').skip(1) {
    match segment {
      "." => {}
      ".." => {
        out.pop();
      }
      s => out.push(s),
    }
  }
  let mut path = format!("/{}", out.join("/"));
  if (merged.ends_with("/.") || merged.ends_with("/..")) && !path.ends_with('/') {
    path.push('/');
  }
  let query = match query {
    Some(q) => format!("?{q}"),
    None if reference.is_empty() => base.query().map(|q| format!("?{q}")).unwrap_or_default(),
    None => String::new(),
  };
  format!("{scheme}://{authority}{path}{query}").parse().map_err(|e: http::uri::InvalidUri| bad(e.to_string()))
}

impl Prepared {
  /// The request a redirect asks for, or `None` when `status` is not one to follow.
  fn redirected(&self, status: StatusCode, headers: &HeaderMap, hops: usize) -> Result<Option<Prepared>, Error> {
    if self.inner.redirects == 0 || !is_redirect(status) {
      return Ok(None);
    }
    let Some(location) = headers.get(http::header::LOCATION).and_then(|v| v.to_str().ok()) else { return Ok(None) };
    if hops >= self.inner.redirects {
      return Err(Error::TooManyRedirects);
    }
    let Ok(uri) = resolve(&self.uri, location) else { return Ok(None) };
    let key = Key::for_target(&uri, &self.inner.dialer.proxies)?;
    let to_get = match status.as_u16() {
      303 => self.method != Method::HEAD,
      301 | 302 => self.method == Method::POST,
      _ => false,
    };
    if !to_get && self.sends_body && !self.body.replayable() {
      return Ok(None);
    }
    let mut next = self.clone();
    if to_get {
      next.method = Method::GET;
      next.body = ReqBody::Full(Bytes::new());
      next.sends_body = false;
      for name in [CONTENT_LENGTH, http::header::CONTENT_TYPE, http::header::CONTENT_ENCODING, http::header::TRANSFER_ENCODING] {
        next.headers.remove(name);
      }
    }
    if key != self.key {
      for name in [http::header::AUTHORIZATION, http::header::COOKIE, http::header::PROXY_AUTHORIZATION] {
        next.headers.remove(name);
      }
      next.caller_cookie = false;
    }
    next.uri = uri;
    next.key = key;
    Ok(Some(next))
  }

  /// The decoder for this response, taking `Content-Encoding` and `Content-Length` off its headers when there is one.
  fn decoder(&self, headers: &mut HeaderMap) -> Option<crate::decode::Decoder> {
    if !self.decode || self.method == Method::HEAD {
      return None;
    }
    let value = headers.get(http::header::CONTENT_ENCODING)?.to_str().ok()?;
    let decoder = crate::decode::Decoder::for_encoding(value)?;
    headers.remove(http::header::CONTENT_ENCODING);
    headers.remove(CONTENT_LENGTH);
    Some(decoder)
  }

  fn head(&self, proto: Proto) -> RequestHead {
    let version = if proto == Proto::H2 { Version::HTTP_2 } else { Version::HTTP_11 };
    let mut headers = self.headers.clone();
    if let Some(auth) = self.key.forwarded().and_then(|p| p.auth.as_deref()) {
      if let Ok(v) = HeaderValue::from_str(auth) {
        headers.insert(http::header::PROXY_AUTHORIZATION, v);
      }
    }
    #[cfg(feature = "cookies")]
    if !self.caller_cookie {
      if let Some(value) = self.stored_cookies() {
        headers.insert(http::header::COOKIE, value);
      }
    }
    RequestHead { method: self.method.clone(), uri: self.uri.clone(), version, headers }
  }

  #[cfg(feature = "cookies")]
  fn stored_cookies(&self) -> Option<HeaderValue> {
    let store = self.inner.cookies.as_ref()?;
    let url = url::Url::parse(&self.uri.to_string()).ok()?;
    let store = store.lock();
    let pairs: Vec<String> = store.get_request_values(&url).map(|(name, value)| format!("{name}={value}")).collect();
    if pairs.is_empty() {
      return None;
    }
    HeaderValue::from_str(&pairs.join("; ")).ok()
  }

  /// Keeps the response's `Set-Cookie` values when the client has a store.
  fn remember(&self, headers: &HeaderMap) {
    #[cfg(feature = "cookies")]
    if let Some(store) = self.inner.cookies.as_ref() {
      let Ok(url) = url::Url::parse(&self.uri.to_string()) else { return };
      let cookies = headers
        .get_all(http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| cookie_store::RawCookie::parse(v.to_owned()).ok());
      store.lock().store_response_cookies(cookies, &url);
    }
    #[cfg(not(feature = "cookies"))]
    let _ = headers;
  }

  fn idempotent(&self) -> bool {
    matches!(self.method, Method::GET | Method::HEAD | Method::OPTIONS | Method::PUT | Method::DELETE | Method::TRACE)
  }

  /// Whether a failed attempt may be sent again on another connection.
  fn retry(&self, error: &Error, lease: &Lease, saw_head: bool) -> bool {
    if saw_head || (self.sends_body && !self.body.replayable()) {
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
  /// The request failed on a connection and may be sent once more on another.
  Retry,
  /// The connection had no room. Waiting for room does not count as an attempt.
  Requeue,
  Fail(Error),
}

impl RequestBuilder<Blocking> {
  /// A body read from `reader`, `length` bytes long when known. With no length it goes out chunked over HTTP/1.1.
  pub fn body_reader(mut self, reader: impl std::io::Read + Send + 'static, length: Option<u64>) -> Self {
    self.body = ReqBody::Reader(Arc::new(parking_lot::Mutex::new(Some(Box::new(reader)))), length);
    self
  }

  pub fn send(self) -> Result<Response<SyncBody>, Error> {
    let mut req = self.prepare()?;
    let mut hops = 0;
    loop {
      let response = send_once_sync(&req)?;
      req.remember(response.headers());
      match req.redirected(response.status(), response.headers(), hops)? {
        Some(next) => {
          response.into_body().drain_sync(REDIRECT_DRAIN);
          req = next;
          hops += 1;
        }
        None => return Ok(response.map_body(SyncBody::from_state)),
      }
    }
  }
}

fn send_once_sync(req: &Prepared) -> Result<Response<BodyState>, Error> {
  {
    let mut attempts = 0;
    while attempts < ATTEMPTS {
      let lease = req.inner.pool.checkout_sync(&req.key, &req.inner.dialer, req.deadline)?;
      match attempt_sync(req, lease) {
        Attempt::Done(r) => return Ok(r),
        Attempt::Retry => attempts += 1,
        Attempt::Requeue if req.deadline.is_some_and(|d| Instant::now() >= d) => return Err(Error::Timeout),
        Attempt::Requeue => {}
        Attempt::Fail(e) => return Err(e),
      }
    }
    Err(Error::Closed)
  }
}

impl RequestBuilder<Async> {
  /// A body from `stream`, `length` bytes long when known. With no length it goes out chunked over HTTP/1.1.
  pub fn body_stream(mut self, stream: impl futures_core::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static, length: Option<u64>) -> Self {
    self.body = ReqBody::Stream(Arc::new(parking_lot::Mutex::new(Some(Box::pin(stream)))), length);
    self
  }

  pub async fn send(self) -> Result<Response<AsyncBody>, Error> {
    let mut req = self.prepare()?;
    let mut hops = 0;
    loop {
      let response = send_once_async(&req).await?;
      req.remember(response.headers());
      match req.redirected(response.status(), response.headers(), hops)? {
        Some(next) => {
          response.into_body().drain_async(REDIRECT_DRAIN).await;
          req = next;
          hops += 1;
        }
        None => return Ok(response.map_body(AsyncBody::from_state)),
      }
    }
  }
}

async fn send_once_async(req: &Prepared) -> Result<Response<BodyState>, Error> {
  {
    let mut attempts = 0;
    while attempts < ATTEMPTS {
      let lease = req.inner.pool.checkout_async(&req.key, &req.inner.dialer, req.deadline).await?;
      match attempt_async(req, lease).await {
        Attempt::Done(r) => return Ok(r),
        Attempt::Retry => attempts += 1,
        Attempt::Requeue if req.deadline.is_some_and(|d| Instant::now() >= d) => return Err(Error::Timeout),
        Attempt::Requeue => {}
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
      Err(Attempt::Requeue)
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
      lease.conn.reset(id);
      lease.release();
      Err(if retry { Attempt::Retry } else { Attempt::Fail(e) })
    }
  }
}

fn send_request_body_sync(req: &Prepared, lease: &Lease, id: crate::engine::ExchangeId) -> Result<(), Error> {
  match &req.body {
    ReqBody::Full(b) => lease.conn.send_body_sync(id, b.clone(), true, req.deadline),
    ReqBody::Reader(slot, _) => {
      let mut reader = slot.lock().take().ok_or_else(|| Error::Protocol("the request body was already read".into()))?;
      let mut buf = vec![0u8; BODY_CHUNK];
      loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
          return lease.conn.send_body_sync(id, Bytes::new(), true, req.deadline);
        }
        lease.conn.send_body_sync(id, Bytes::copy_from_slice(&buf[..n]), false, req.deadline)?;
      }
    }
    ReqBody::Stream(..) => Err(Error::Protocol("a stream body needs the async client".into())),
  }
}

async fn send_request_body_async(req: &Prepared, lease: &Lease, id: crate::engine::ExchangeId) -> Result<(), Error> {
  match &req.body {
    ReqBody::Full(b) => lease.conn.send_body_async(id, b.clone(), true, req.deadline).await,
    ReqBody::Stream(slot, _) => {
      let mut stream = slot.lock().take().ok_or_else(|| Error::Protocol("the request body was already read".into()))?;
      loop {
        match std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
          Some(Ok(chunk)) if chunk.is_empty() => {}
          Some(Ok(chunk)) => lease.conn.send_body_async(id, chunk, false, req.deadline).await?,
          Some(Err(e)) => return Err(e.into()),
          None => return lease.conn.send_body_async(id, Bytes::new(), true, req.deadline).await,
        }
      }
    }
    ReqBody::Reader(..) => Err(Error::Protocol("a reader body needs the blocking client".into())),
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
    if let Err(e) = send_request_body_sync(req, &lease, id) {
      let retry = req.retry(&e, &lease, false);
      lease.conn.reset(id);
      lease.release();
      return if retry { Attempt::Retry } else { Attempt::Fail(e) };
    }
  }
  let mut lease = lease;
  let mut queued = std::collections::VecDeque::new();
  loop {
    let event = match queued.pop_front() {
      Some(event) => Ok(event),
      None => match lease.conn.next_events_sync(id, req.deadline) {
        Ok(events) => {
          queued = events;
          continue;
        }
        Err(e) => Err(e),
      },
    };
    if let Ok(Event::Head(_)) = &event {
      let Ok(Event::Head(mut head)) = event else { unreachable!() };
      let decoder = req.decoder(&mut head.headers);
      let state = BodyState::new(lease, id, req.deadline, req.method == Method::HEAD, decoder, req.inner.max_response_size, queued);
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
    if let Err(e) = send_request_body_async(req, &lease, id).await {
      let retry = req.retry(&e, &lease, false);
      lease.conn.reset(id);
      lease.release();
      return if retry { Attempt::Retry } else { Attempt::Fail(e) };
    }
  }
  let mut lease = lease;
  let mut queued = std::collections::VecDeque::new();
  loop {
    let event = match queued.pop_front() {
      Some(event) => Ok(event),
      None => match lease.conn.next_events_async(id, req.deadline).await {
        Ok(events) => {
          queued = events;
          continue;
        }
        Err(e) => Err(e),
      },
    };
    if let Ok(Event::Head(_)) = &event {
      let Ok(Event::Head(mut head)) = event else { unreachable!() };
      let decoder = req.decoder(&mut head.headers);
      let state = BodyState::new(lease, id, req.deadline, req.method == Method::HEAD, decoder, req.inner.max_response_size, queued);
      return Attempt::Done(Response::new(head.status, head.version, head.headers, state));
    }
    match on_event(req, lease, id, event) {
      Ok((l, None)) => lease = l,
      Ok((_, Some(a))) => return a,
      Err(a) => return a,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::resolve;

  #[test]
  fn resolve_follows_rfc_3986_section_5_4_1() {
    let base: http::Uri = "http://a/b/c/d;p?q".parse().unwrap();
    let cases = [
      ("g", "http://a/b/c/g"),
      ("./g", "http://a/b/c/g"),
      ("g/", "http://a/b/c/g/"),
      ("/g", "http://a/g"),
      ("//g", "http://g/"),
      ("?y", "http://a/b/c/d;p?y"),
      ("g?y", "http://a/b/c/g?y"),
      ("", "http://a/b/c/d;p?q"),
      (".", "http://a/b/c/"),
      ("./", "http://a/b/c/"),
      ("..", "http://a/b/"),
      ("../", "http://a/b/"),
      ("../g", "http://a/b/g"),
      ("../..", "http://a/"),
      ("../../", "http://a/"),
      ("../../g", "http://a/g"),
      ("../../../g", "http://a/g"),
      ("/./g", "http://a/g"),
      ("/../g", "http://a/g"),
      ("https://other.test/x", "https://other.test/x"),
    ];
    for (reference, expected) in cases {
      assert_eq!(resolve(&base, reference).unwrap().to_string(), expected, "{reference}");
    }
  }
}
