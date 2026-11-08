# fibre_http

An HTTP/1.1 and HTTP/2 client with blocking and async handles that share one connection pool. `client.to_sync()` and `client.to_async()` switch between them the way fibre's channels do. A response body can switch mid-read too.

```rust
let client = fibre_http::AsyncClient::new();
let text = client.get("https://example.com/").send().await?.text().await?;

let blocking = client.to_sync();
let text = blocking.get("https://example.com/").send()?.text()?;
```

## Why

Most Rust HTTP clients are async at heart. Their blocking client runs a runtime on a background thread and passes every request through a channel to it. fibre_http has no background thread. A blocking caller reads and writes the socket itself. An async caller does the same on its own tokio runtime. Both kinds of caller can share one HTTP/2 connection at the same time: whichever is waiting reads the socket for everyone and hands each caller its own response.

The protocol code does no I/O. HTTP/1.1 runs on `ureq-proto` and HTTP/2 on `h2`, each turned into bytes in and events out, so blocking and async callers drive the same engine. An async caller is not tied to the runtime that opened a connection.

## Performance

A local hyper 1.12 server answering 64-byte GETs, 10 s per cell, one run each, on an Apple M4 Pro in high power mode (2026-11-08). Requests per second:

| | Requesters | fibre_http async | fibre_http blocking | hyper | reqwest async | reqwest blocking |
|---|---|---|---|---|---|---|
| HTTP/1.1 | 1 | 45,364 | 45,272 | 46,377 | 38,987 | 29,356 |
| HTTP/1.1 | 8 | 145,657 | 133,776 | 147,623 | 129,940 | 83,773 |
| h2c | 1 | 42,130 | 43,138 | 43,140 | 34,734 | 26,342 |
| h2c | 8 | 91,112 | 67,094 | 84,315 | 74,225 | 83,994 |

Inside an FSR server whose page calls an upstream over HTTP, moving FSR's transport from reqwest to fibre_http served 17% more requests a second at 8 clients. At 256 clients it served the same rate with 132 MB of peak memory against 237 MB.

Blocking HTTP/2 from many threads at once is the weak spot today. Eight threads sharing one connection reach 74% of what eight async tasks do.

## Features

- HTTP/1.1 keep-alive and HTTP/2 multiplexing, chosen by ALPN or by prior knowledge for h2c.
- A pool shared across clients and modes, with a per-origin connection limit, idle expiry and a health check for idle HTTP/1.1 connections.
- rustls with ring and the bundled Mozilla roots by default. The OS root store and native-tls are features.
- Redirects, with method and credential rules matching browsers.
- One deadline per request, from checkout to the end of the body.
- Retries of requests the server never processed. Idempotent requests are also retried on a connection that died before answering.
- HTTP proxies, from the environment or the builder, with `CONNECT` for HTTPS.
- `Expect: 100-continue`.
- Streamed request bodies from a reader or a stream.
- Features for gzip and zstd decoding, JSON, an in-memory cookie store and the hickory resolver.

| Feature | Default |
|---|---|
| `http1`, `http2`, `rustls`, `webpki-roots` | on |
| `native-roots`, `native-tls`, `hickory`, `gzip`, `zstd`, `json`, `cookies` | off |

## Status

0.1, not yet published. Tested on macOS against real HTTP/1.1, h2c and TLS servers, with tests drawn from the contracts reqwest's and hyper's suites check.

Known limitations:

- Unix only. Windows is not supported yet.
- No HTTP/3.
- HTTP/1.1 response trailers are dropped.
- No HTTP/2 keepalive pings.

## Documentation

- [Usage guide](README.USAGE.md)
- [API reference](API_REFERENCE.md)

## License

MPL-2.0.
