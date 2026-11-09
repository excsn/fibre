# Fibre HTTP

Simple High Performance Multi-mode Sync/Async HTTP 1/2 client.

## Why Fibre HTTP

- **Fast.** Level with hyper on HTTP/1.1. Ahead of it on a shared HTTP/2 connection.
- **Faster blocking calls.** Ahead of reqwest's blocking client on HTTP/1.1.
- **One pool for sync and async code.** `to_sync()` and `to_async()` switch a client for free. Threads and tasks can share one HTTP/2 connection.
- **Bodies switch too.** Start a request in async code and read the body on a blocking thread.
- **No background runtime.** A blocking call does its own I/O on its own thread.
- **Not tied to a runtime.** Async callers can use connections another runtime opened.

## Performance

Requests a second for 64-byte GETs to a local hyper server with N concurrent callers, on an Apple M4 Pro.

| | N | Fibre HTTP async | Fibre HTTP blocking | hyper | reqwest async | reqwest blocking |
|---|---|---|---|---|---|---|
| HTTP/1.1 | 1 | 42,498 | 43,149 | 42,855 | 35,762 | 26,945 |
| HTTP/1.1 | 8 | 141,966 | 132,618 | 146,463 | 127,205 | 82,051 |
| HTTP/1.1 | 32 | 169,772 | 124,303 | 174,848 | 171,444 | 85,636 |
| h2c | 1 | 36,040 | 39,506 | 40,714 | 31,806 | 23,837 |
| h2c | 8 | 102,049 | 84,630 | 83,779 | 71,631 | 77,187 |
| h2c | 32 | 126,029 | 73,086 | 81,692 | 74,301 | 112,085 |

## Features

- HTTP/1.1 keep-alive and HTTP/2 multiplexing, by ALPN or h2c prior knowledge.
- HTTP/2 keepalive pings.
- One pool across clients and modes, with per-origin limits and idle expiry.
- HTTP proxies with `CONNECT` for HTTPS.
- rustls by default, native-tls optional, with client certificates for mutual TLS.
- Per-request deadlines, browser-style redirects and safe retries.
- Streamed request bodies and `Expect: 100-continue`.

| Feature | Default |
|---|---|
| `http1`, `http2`, `rustls`, `webpki-roots` | on |
| `native-roots`, `native-tls`, `hickory`, `gzip`, `zstd`, `json`, `cookies` | off |

## Getting Started

```toml
[dependencies]
fibre_http = "0"
```

```rust
use fibre_http::AsyncClient;

#[tokio::main]
async fn main() -> Result<(), fibre_http::Error> {
  let client = AsyncClient::new();
  let page = client.get("https://example.com/").send().await?.text().await?;
  println!("async: {} bytes", page.len());

  let blocking = client.clone().to_sync();
  let page = tokio::task::spawn_blocking(move || blocking.get("https://example.com/").send()?.text()).await.unwrap()?;
  println!("blocking, same pool: {} bytes", page.len());
  Ok(())
}
```

- [Usage guide](README.USAGE.md)
- [API reference](API_REFERENCE.md)

## Project Status: Beta

The API may still change.

## Limitations

- Unix only. Windows is not supported.
- No HTTP/3.
- HTTP/1.1 response trailers are dropped.

## License

MPL-2.0.
