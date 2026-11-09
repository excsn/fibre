# API Reference: fibre_http

An HTTP/1.1 and HTTP/2 client whose blocking and async handles share one connection pool.

## Contents

* [1. Features](#1-features)
* [2. Clients](#2-clients)
  * [`SyncClient`](#syncclient)
  * [`AsyncClient`](#asyncclient)
  * [`ClientBuilder`](#clientbuilder)
* [3. Requests](#3-requests)
  * [`RequestBuilder<M>`](#requestbuilderm)
  * [`Blocking` and `Async`](#blocking-and-async)
* [4. Responses](#4-responses)
  * [`Response<B>`](#responseb)
  * [`SyncBody`](#syncbody)
  * [`AsyncBody`](#asyncbody)
* [5. Pool](#5-pool)
  * [`Pool`](#pool)
  * [`PoolBuilder`](#poolbuilder)
* [6. Proxies](#6-proxies)
  * [`Proxy`](#proxy)
* [7. Engines](#7-engines)
  * [`Engine`](#engine)
  * [Engine types](#engine-types)
  * [`h1::Engine`](#h1engine)
  * [`h2::Engine`](#h2engine)
* [8. Error handling](#8-error-handling)
  * [`Error`](#error)

## 1. Features

| Feature | Default | Turns on |
|---|---|---|
| `http1` | yes | HTTP/1.1 through `ureq-proto` |
| `http2` | yes | HTTP/2 through `h2` |
| `rustls` | yes | TLS through rustls with the ring provider |
| `webpki-roots` | yes | The bundled Mozilla root list, for rustls |
| `native-roots` | no | The OS root store, for rustls |
| `native-tls` | no | TLS through the platform library. The default TLS when `rustls` is off |
| `hickory` | no | The hickory resolver for async lookups |
| `gzip` | no | gzip response decoding |
| `zstd` | no | zstd response decoding |
| `json` | no | `RequestBuilder::json` and `Response::json` |
| `cookies` | no | `ClientBuilder::cookie_store` |

## 2. Clients

### `SyncClient`

A blocking client. `Clone` shares the configuration and the pool.

* `pub fn new() -> SyncClient`
* `pub fn builder() -> ClientBuilder`
* `pub fn to_async(self) -> AsyncClient`: the same client with async methods.
* `pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn patch(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn head(&self, url: impl AsRef<str>) -> RequestBuilder<Blocking>`
* `pub fn request(&self, method: http::Method, url: impl AsRef<str>) -> RequestBuilder<Blocking>`

### `AsyncClient`

An async client on tokio. `Clone` shares the configuration and the pool.

* `pub fn new() -> AsyncClient`
* `pub fn builder() -> ClientBuilder`
* `pub fn to_sync(self) -> SyncClient`: the same client with blocking methods.
* `pub fn get`, `post`, `put`, `patch`, `delete`, `head`, `request`: as on `SyncClient`, returning `RequestBuilder<Async>`.

### `ClientBuilder`

Configuration for both clients. `Default` is the configuration `new()` uses.

* `pub fn pool(self, pool: &Pool) -> Self`: shares `pool`. Default: a pool of the client's own.
* `pub fn timeout(self, timeout: Duration) -> Self`: one deadline per request, from checkout through the end of the body. Default: none.
* `pub fn redirects(self, n: usize) -> Self`: the most redirects followed. Default: 10. `0` returns every redirect as the response.
* `pub fn decompress(self, on: bool) -> Self`: decode gzip and zstd bodies and send `Accept-Encoding`. Default: `true`, with effect only under `gzip` or `zstd`.
* `pub fn default_headers(self, headers: http::HeaderMap) -> Self`: sent on every request. A request header of the same name replaces them.
* `pub fn max_response_size(self, bytes: usize) -> Self`: the most bytes a body may decode to. Default: no limit.
* `pub fn user_agent(self, value: &str) -> Result<Self, Error>`. Default: `fibre_http/<version>`.
* `pub fn http2_prior_knowledge(self) -> Self`: HTTP/2 without TLS to `http://` origins.
* `pub fn http2_keep_alive_interval(self, interval: Duration) -> Self`: send a PING after `interval` with nothing read while a request is in flight. Off by default. Idle pooled connections are not pinged.
* `pub fn http2_keep_alive_timeout(self, timeout: Duration) -> Self`: how long to wait for the PONG, 20 s by default. Without one the connection closes and its requests fail with `Error::Timeout`.
* `pub fn add_root_certificate(self, der: Vec<u8>) -> Self`: trusts a DER root beside the default roots.
* `pub fn no_bundled_roots(self) -> Self`: trusts only added roots.
* `pub fn use_native_tls(self) -> Self`: under `native-tls`.
* `pub fn cookie_store(self, on: bool) -> Self`: under `cookies`. Default: `false`.
* `pub fn proxy(self, proxy: Proxy) -> Self`: every request through `proxy`, in place of the environment's.
* `pub fn no_proxy_for(self, list: &str) -> Self`: hosts in `NO_PROXY` form that skip the proxy.
* `pub fn no_proxy(self) -> Self`: no proxy, whatever the environment says.
* `pub fn build_sync(self) -> Result<SyncClient, Error>`
* `pub fn build_async(self) -> Result<AsyncClient, Error>`

Unless `proxy` or `no_proxy` is called, the proxies come from `http_proxy`, `https_proxy`, `all_proxy` and `no_proxy`, lowercase before uppercase, read when the client is built. `HTTP_PROXY` is ignored while `REQUEST_METHOD` is set.

## 3. Requests

### `RequestBuilder<M>`

One request, sent with `send`.

* `pub fn header(self, name: &str, value: &str) -> Self`: appends. An invalid name or value fails at `send`.
* `pub fn query<K: AsRef<str>, V: AsRef<str>>(self, pairs: &[(K, V)]) -> Self`: appends form-encoded pairs to the URL's query.
* `pub fn body(self, body: impl Into<Bytes>) -> Self`
* `pub fn form<K: AsRef<str>, V: AsRef<str>>(self, pairs: &[(K, V)]) -> Self`: sets `Content-Type: application/x-www-form-urlencoded` unless one is set.
* `pub fn json<T: Serialize + ?Sized>(self, value: &T) -> Self`: under `json`. Sets `Content-Type: application/json` unless one is set.
* `pub fn timeout(self, timeout: Duration) -> Self`: replaces the client's timeout for this request.

On `RequestBuilder<Blocking>`:

* `pub fn body_reader(self, reader: impl Read + Send + 'static, length: Option<u64>) -> Self`: with `None` the body goes out chunked over HTTP/1.1.
* `pub fn send(self) -> Result<Response<SyncBody>, Error>`

On `RequestBuilder<Async>`:

* `pub fn body_stream(self, stream: impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static, length: Option<u64>) -> Self`
* `pub async fn send(self) -> Result<Response<AsyncBody>, Error>`

`Content-Length` is set from a known body length unless the caller set it. A request sent with a reader or a stream is never retried. A 307 or 308 to it is returned instead of followed. A reader or stream body on `GET`, `HEAD`, `DELETE` or `OPTIONS` fails at `send`. A request with `Expect: 100-continue` holds its body until the server answers 100 or 1 s passes.

### `Blocking` and `Async`

Marker types selecting `RequestBuilder`'s `send`.

## 4. Responses

### `Response<B>`

Status, headers and a body `B`.

* `pub fn status(&self) -> http::StatusCode`
* `pub fn version(&self) -> http::Version`
* `pub fn headers(&self) -> &http::HeaderMap`: `Content-Encoding` and `Content-Length` are removed when the body is decoded.
* `pub fn body_mut(&mut self) -> &mut B`
* `pub fn into_body(self) -> B`
* `pub fn error_for_status(self) -> Result<Self, Error>`: `Error::Status` for 4xx and 5xx.

On `Response<SyncBody>`: `pub fn to_async(self) -> Response<AsyncBody>`, `pub fn bytes(self) -> Result<Bytes, Error>`, `pub fn text(self) -> Result<String, Error>` (UTF-8), `pub fn json<T: DeserializeOwned>(self) -> Result<T, Error>` (under `json`).

On `Response<AsyncBody>`: `pub fn to_sync(self) -> Response<SyncBody>`, `pub async fn bytes`, `pub async fn text`, `pub async fn json`.

### `SyncBody`

A body read with blocking calls. Implements `std::io::Read`.

* `pub fn to_async(self) -> AsyncBody`: the same exchange, read with async calls from here on.
* `pub fn chunk(&mut self) -> Result<Option<Bytes>, Error>`: `None` at the end.
* `pub fn bytes(self) -> Result<Bytes, Error>`
* `pub fn trailers(&self) -> Option<&http::HeaderMap>`: after the end, over HTTP/2.

Dropping a body before its end resets the exchange: an HTTP/1.1 connection closes, an HTTP/2 stream is cancelled. Bytes already received are read first, so a short body that has arrived whole returns its connection to the pool.

### `AsyncBody`

A body read with async calls.

* `pub fn to_sync(self) -> SyncBody`
* `pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error>`
* `pub async fn bytes(self) -> Result<Bytes, Error>`
* `pub fn trailers(&self) -> Option<&http::HeaderMap>`

## 5. Pool

### `Pool`

Connections by origin, shared by every client built with it, blocking and async alike. `Clone` shares it. `Default` is `Pool::builder().build()`.

* `pub fn builder() -> PoolBuilder`
* `pub fn connections_opened(&self) -> usize`: connections dialled since it was built.

### `PoolBuilder`

* `pub fn max_per_host(self, n: usize) -> Self`: connections per origin for both protocols. Default: 32. At least 1.
* `pub fn idle_timeout(self, d: Duration) -> Self`: an idle connection older than this is not reused. Default: 90 s.
* `pub fn peek_after(self, d: Duration) -> Self`: an HTTP/1.1 connection idle at least this long is peeked for a server close before reuse. Default: 1 s.
* `pub fn build(self) -> Pool`

An HTTP/2 connection takes up to the server's `SETTINGS_MAX_CONCURRENT_STREAMS` exchanges; a second opens only when the first is full.

## 6. Proxies

### `Proxy`

An HTTP proxy at `http://[user:pass@]host[:port]`. Port 80 when absent. Userinfo is percent-decoded and sent as `Proxy-Authorization: Basic`.

* `pub fn parse(url: &str) -> Result<Proxy, Error>`: any scheme but `http` fails.

`http://` requests go to the proxy in absolute form. `https://` requests go through `CONNECT`.

## 7. Engines

### `Engine`

A protocol engine: bytes in, events and bytes out, no I/O.

* `fn capacity(&self) -> Capacity`
* `fn open(&mut self, head: RequestHead, now: Instant, out: &mut Output) -> Result<ExchangeId, OpenError>`
* `fn send_body(&mut self, id: ExchangeId, data: Bytes, end: bool, out: &mut Output) -> usize`: bytes accepted, possibly fewer than offered.
* `fn consumed(&mut self, id: ExchangeId, n: usize, out: &mut Output)`: `n` body bytes were read.
* `fn reset(&mut self, id: ExchangeId, out: &mut Output)`
* `fn on_bytes(&mut self, input: &mut BytesMut, out: &mut Output)`: leaves a partial frame in `input`.
* `fn on_eof(&mut self, out: &mut Output)`
* `fn on_tick(&mut self, now: Instant, out: &mut Output)`
* `fn next_deadline(&self) -> Option<Instant>`
* `fn close(&mut self, out: &mut Output)`

### Engine types

* `ExchangeId(pub u64)`
* `Capacity`: `Unique { available: bool }` or `Shared { open: usize, max: usize }`. `#[non_exhaustive]`.
* `RequestHead { method, uri, version, headers }`
* `ResponseHead { status, version, headers }`
* `OpenError`: `Full`, `GoingAway`, `Closed`. `#[non_exhaustive]`.
* `NetAction`: `Send(Bytes)`, `Flush`, `Close`. `#[non_exhaustive]`.
* `Event`: `Informational(ResponseHead)`, `Head(ResponseHead)`, `Data(Bytes)`, `Trailers(HeaderMap)`, `End`, `Reset(Error)`, `Writable`. `#[non_exhaustive]`.
* `ConnEvent`: `GoingAway`, `Closed(Option<Error>)`. `#[non_exhaustive]`.
* `Output { net, events, conn }`: `pub fn is_empty(&self) -> bool`.

### `h1::Engine`

Under `http1`. One exchange at a time.

* `pub fn new() -> Engine`
* `pub fn absolute_form() -> Engine`: request lines carry the whole URI, for a forward proxy.

Never emits `Event::Informational`. A response head over 64 KiB is an error. Bytes after a complete response close the connection.

### `h2::Engine`

Under `http2`.

* `pub fn new() -> Engine`
* `pub fn keep_alive(self, interval: Duration, timeout: Duration) -> Engine`: PING after `interval` with nothing read while an exchange is open. With no PONG within `timeout`, every open exchange gets `Event::Reset(Error::Timeout)` and the connection closes.

`next_deadline` is `None` without keep-alive or with no exchange open. Otherwise it is the next PING or the PONG deadline.

## 8. Error handling

### `Error`

`#[non_exhaustive]`, `Clone`, `PartialEq`. Implements `std::error::Error`.

| Variant | Meaning |
|---|---|
| `Protocol(String)` | The peer broke the protocol or the request could not be built |
| `Reset(u32)` | The peer reset the exchange with this HTTP/2 error code |
| `Closed` | The connection ended before the exchange did |
| `Refused` | The peer never processed the exchange (GOAWAY or `REFUSED_STREAM`); safe to send again |
| `Timeout` | The request's deadline passed |
| `Io(String)` | A socket or body reader error |
| `Tls(String)` | The TLS handshake or configuration failed |
| `Url(String)` | The URL or a proxy URL is invalid |
| `Decode(String)` | The body could not be decoded |
| `TooManyRedirects` | More redirects than `redirects` allows |
| `TooLarge` | The body passed `max_response_size` |
| `Status(u16)` | From `error_for_status` |
| `ProxyAuth` | The proxy answered `CONNECT` with 407 |

A request that fails with `Refused` is sent again on another connection. One that fails with `Closed` or `Io` on a reused connection before its response head is sent again when its method is idempotent. A request makes at most three attempts. Neither retry applies to a reader or stream body.
