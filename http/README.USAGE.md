# Usage Guide: fibre_http

How to send HTTP requests with fibre_http from blocking code, async code and both at once.

## Table of Contents

* [Core Concepts](#core-concepts)
* [Quick Start](#quick-start)
  * [Blocking](#blocking)
  * [Async](#async)
* [Building a Client](#building-a-client)
  * [Choosing defaults](#choosing-defaults)
  * [Sharing a pool](#sharing-a-pool)
* [Switching Between Blocking and Async](#switching-between-blocking-and-async)
  * [Switching a client](#switching-a-client)
  * [Switching a body](#switching-a-body)
* [Sending a Request](#sending-a-request)
  * [Headers and query](#headers-and-query)
  * [Bodies](#bodies)
  * [Streaming a body](#streaming-a-body)
  * [Expect: 100-continue](#expect-100-continue)
* [Reading a Response](#reading-a-response)
  * [Whole bodies](#whole-bodies)
  * [Chunks and readers](#chunks-and-readers)
  * [Statuses](#statuses)
* [Using HTTP/2](#using-http2)
* [Configuring TLS](#configuring-tls)
* [Following Redirects](#following-redirects)
* [Setting Timeouts](#setting-timeouts)
* [Going Through a Proxy](#going-through-a-proxy)
* [Decompressing Responses](#decompressing-responses)
* [Keeping Cookies](#keeping-cookies)
* [Error Handling](#error-handling)

## Core Concepts

* **Client:** `SyncClient` or `AsyncClient`. Two handles to one client: the same configuration and the same pool.
* **Pool:** connections by origin, shared by every client built with it, blocking and async alike.
* **Origin:** scheme, host and port. A pool keeps up to `max_per_host` connections per origin.
* **Request builder:** `client.get(url)` and the other verbs. Collects headers, a body and a timeout, then `send`.
* **Response:** status, version, headers and a body.
* **Body:** `SyncBody` or `AsyncBody`, read in pieces or whole. Either can become the other mid-read.
* **Exchange:** one request and its response on a connection.
* **HTTP/1.1 connection:** one exchange at a time. Back in the pool once the body is read.
* **HTTP/2 connection:** many exchanges at once, up to the server's stream limit.
* **ALPN:** how a TLS handshake picks HTTP/2 or HTTP/1.1.
* **h2c:** HTTP/2 without TLS, by prior knowledge.
* **Deadline:** one per request, covering the wait for a connection, the request, the response head and the body.
* **Feature:** a Cargo feature turning on an optional part: `gzip`, `zstd`, `json`, `cookies`, `native-tls`, `native-roots`, `hickory`.

## Quick Start

### Blocking

```rust
use fibre_http::SyncClient;

fn main() -> Result<(), fibre_http::Error> {
  let client = SyncClient::new();
  let text = client.get("https://example.com/").send()?.error_for_status()?.text()?;
  println!("{text}");
  Ok(())
}
```

### Async

```rust
use fibre_http::AsyncClient;

#[tokio::main]
async fn main() -> Result<(), fibre_http::Error> {
  let client = AsyncClient::new();
  let text = client.get("https://example.com/").send().await?.error_for_status()?.text().await?;
  println!("{text}");
  Ok(())
}
```

## Building a Client

### Choosing defaults

```rust
use std::time::Duration;

let client = fibre_http::SyncClient::builder()
  .timeout(Duration::from_secs(10))
  .redirects(5)
  .user_agent("my-app/1.0")?
  .max_response_size(16 * 1024 * 1024)
  .build_sync()?;
```

`SyncClient::new()` and `AsyncClient::new()` take every default: no timeout, 10 redirects, a pool of their own.

### Sharing a pool

```rust
use std::time::Duration;
use fibre_http::{AsyncClient, Pool, SyncClient};

let pool = Pool::builder().max_per_host(64).idle_timeout(Duration::from_secs(30)).build();
let api = AsyncClient::builder().pool(&pool).timeout(Duration::from_secs(5)).build_async()?;
let uploads = SyncClient::builder().pool(&pool).build_sync()?;
println!("{} connections so far", pool.connections_opened());
```

Clients built on one pool reuse each other's connections whatever their mode.

## Switching Between Blocking and Async

### Switching a client

```rust
let client = fibre_http::AsyncClient::new();
let blocking = client.clone().to_sync();

let text = tokio::task::spawn_blocking(move || blocking.get("https://example.com/").send()?.text()).await.unwrap()?;
let back = client.to_sync().to_async();
```

The switch costs nothing and keeps the pool.

### Switching a body

```rust
use std::io::Read;

let response = client.get("https://example.com/large.bin").send().await?;
let mut body = response.into_body().to_sync();
let copied = tokio::task::spawn_blocking(move || {
  let mut file = std::fs::File::create("large.bin")?;
  std::io::copy(&mut body, &mut file)
})
.await
.unwrap()?;
```

`Response::to_sync` and `Response::to_async` switch the body and keep the status and headers.

## Sending a Request

### Headers and query

```rust
let response = client
  .get("https://api.example.com/search")
  .query(&[("q", "rust http"), ("page", "2")])
  .header("accept", "application/json")
  .send()?;
```

Query pairs are form-encoded and added after any query already in the URL.

### Bodies

```rust
client.post(url).body("raw bytes").send()?;
client.post(url).form(&[("name", "fibre"), ("kind", "http")]).send()?;
client.post(url).json(&serde_json::json!({ "name": "fibre" })).send()?; // feature `json`
```

`form` and `json` set `Content-Type` unless the request already has one.

### Streaming a body

```rust
let file = std::fs::File::open("upload.bin")?;
let length = file.metadata()?.len();
let response = sync_client.put(url).body_reader(file, Some(length)).send()?;
```

```rust
use futures_util::stream;

let chunks = stream::iter(vec![Ok(bytes::Bytes::from("part one, ")), Ok(bytes::Bytes::from("part two"))]);
let response = async_client.post(url).body_stream(chunks, None).send().await?;
```

With no length the body goes out chunked over HTTP/1.1. A streamed body is read once, so its request is never retried and a 307 or 308 comes back as the response.

### Expect: 100-continue

```rust
let response = client.post(url).header("expect", "100-continue").body(big_payload).send()?;
```

The body waits until the server answers 100, up to 1 s. A server that answers with a final status instead never receives the body.

## Reading a Response

### Whole bodies

```rust
let response = client.get(url).send()?;
println!("{} {:?}", response.status(), response.headers().get("content-type"));
let bytes = response.bytes()?;
```

`text` decodes UTF-8. `json` deserialises under the `json` feature.

### Chunks and readers

```rust
let mut body = client.get(url).send()?.into_body();
while let Some(chunk) = body.chunk()? {
  handle(&chunk);
}
```

```rust
let mut body = async_client.get(url).send().await?.into_body();
while let Some(chunk) = body.chunk().await? {
  handle(&chunk);
}
```

`SyncBody` also implements `std::io::Read`. Dropping a body before its end resets the exchange.

### Statuses

```rust
match client.get(url).send()?.error_for_status() {
  Ok(response) => println!("{}", response.text()?),
  Err(fibre_http::Error::Status(404)) => println!("not found"),
  Err(e) => return Err(e),
}
```

## Using HTTP/2

```rust
let tls = fibre_http::AsyncClient::new();
let h2c = fibre_http::AsyncClient::builder().http2_prior_knowledge().build_async()?;
let response = h2c.get("http://127.0.0.1:8080/").send().await?;
assert_eq!(response.version(), http::Version::HTTP_2);
```

Over TLS, ALPN picks HTTP/2 when the server offers it. Over plain `http://`, `http2_prior_knowledge` speaks HTTP/2 from the first byte. Concurrent requests to one origin share one HTTP/2 connection until the server's stream limit.

Keepalive pings find a dead connection while a request waits on it:

```rust
let client = fibre_http::AsyncClient::builder()
  .http2_keep_alive_interval(Duration::from_secs(10))
  .http2_keep_alive_timeout(Duration::from_secs(5))
  .build_async()?;
```

With nothing read for 10 s while a request is in flight, the client sends a PING. Without a PONG within 5 s the connection closes and its requests fail with `Error::Timeout`. Pings are off by default and idle pooled connections are never pinged.

## Configuring TLS

```rust
let pem_root: Vec<u8> = std::fs::read("internal-ca.der")?;
let internal = fibre_http::SyncClient::builder().add_root_certificate(pem_root).no_bundled_roots().build_sync()?;
let native = fibre_http::SyncClient::builder().use_native_tls().build_sync()?; // feature `native-tls`
```

rustls with the ring provider and the bundled Mozilla roots is the default. `native-roots` adds the OS root store to rustls. `native-tls` uses Security.framework, SChannel or OpenSSL.

## Following Redirects

```rust
let follow_three = fibre_http::SyncClient::builder().redirects(3).build_sync()?;
let none = fibre_http::SyncClient::builder().redirects(0).build_sync()?;
let redirect = none.get(url).send()?;
println!("{:?}", redirect.headers().get("location"));
```

301 and 302 turn a POST into a GET, 303 turns any method but HEAD into a GET, 307 and 308 keep the method and body. A redirect to another origin drops `Authorization`, `Cookie` and `Proxy-Authorization`. Past the limit the request fails with `Error::TooManyRedirects`.

## Setting Timeouts

```rust
use std::time::Duration;

let client = fibre_http::SyncClient::builder().timeout(Duration::from_secs(10)).build_sync()?;
let quick = client.get(url).timeout(Duration::from_millis(500)).send();
```

The deadline covers waiting for a connection, sending, the response head and the whole body. Past it the call fails with `Error::Timeout`.

## Going Through a Proxy

```rust
let proxy = fibre_http::Proxy::parse("http://user:secret@proxy.internal:3128")?;
let client = fibre_http::SyncClient::builder().proxy(proxy).no_proxy_for("localhost, .internal").build_sync()?;
let direct = fibre_http::SyncClient::builder().no_proxy().build_sync()?;
```

Without either call a client reads `http_proxy`, `https_proxy`, `all_proxy` and `no_proxy` when it is built. `https://` goes through `CONNECT`. A 407 fails with `Error::ProxyAuth`.

## Decompressing Responses

```toml
fibre_http = { version = "0.1", features = ["gzip", "zstd"] }
```

```rust
let decoded = fibre_http::SyncClient::new().get(url).send()?.bytes()?;
let raw = fibre_http::SyncClient::builder().decompress(false).build_sync()?.get(url).send()?.bytes()?;
let mine = fibre_http::SyncClient::new().get(url).header("accept-encoding", "br").send()?;
```

With a feature on, the client sends `Accept-Encoding` and decodes as the body streams. A request that sets its own `Accept-Encoding` gets the body as the server sent it.

## Keeping Cookies

```rust
let client = fibre_http::SyncClient::builder().cookie_store(true).build_sync()?; // feature `cookies`
client.post("https://example.com/login").form(&[("user", "me")]).send()?;
let page = client.get("https://example.com/account").send()?.text()?;
```

`Set-Cookie` from every response, redirects included, goes into an in-memory store. Later requests that match by domain and path carry the cookies. A request that sets its own `Cookie` header keeps it.

## Error Handling

`Error` is `#[non_exhaustive]`. The variants worth matching on:

```rust
use fibre_http::Error;

match client.get(url).send().and_then(|r| r.error_for_status()) {
  Ok(response) => Ok(Some(response.text()?)),
  Err(Error::Timeout) => Ok(None),
  Err(Error::Status(code)) if code >= 500 => Ok(None),
  Err(Error::Tls(why)) => Err(format!("tls: {why}")),
  Err(Error::Refused | Error::Closed) => Err("the server went away".to_string()),
  Err(e) => Err(e.to_string()),
}
```

`Refused` means the server never processed the request. The client has already sent a `Refused` request again on another connection, so seeing it means the retries ran out.
