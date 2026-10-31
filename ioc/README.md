# fibre_ioc

[![Crates.io](https://img.shields.io/crates/v/fibre_ioc.svg)](https://crates.io/crates/fibre_ioc)
[![Docs.rs](https://docs.rs/fibre_ioc/badge.svg)](https://docs.rs/fibre_ioc)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

An Inversion of Control container for Rust where services can be registered at any time, except from inside a factory running on the same container. It uses `dashmap` and `std::sync::OnceLock`, resolves through panicking or `Option`-returning macros and panics on a circular dependency instead of overflowing the stack. The task guide is [README.USAGE.md](README.USAGE.md) and the call surface is [API_REFERENCE.md](API_REFERENCE.md).

## Install

```sh
cargo add fibre_ioc
```

| Feature | Default | What it does |
|---|---|---|
| `local` | off | Adds `LocalContainer`, a single-threaded container for types that are not `Send` or `Sync`. |

## What to reach for

| Problem | Piece |
|---|---|
| One shared container for the application | `global()` |
| Keep registrations separate, as in a test | `Container::new()` |
| Store `Rc`, `RefCell` or other `!Send` types | `LocalContainer` |
| One instance created on first use | `add_singleton` |
| A new instance per resolution | `add_transient` |
| Register an object you already built | `add_instance` |
| Resolve through a trait | `add_singleton_trait` and `resolve!(trait MyTrait)` |
| Several providers of one type | the `_with_name` variants |
| A dependency that must exist | `resolve!` and `resolve_from!` |
| A dependency that may be absent | `maybe_resolve!` and `maybe_resolve_from!` |

## Notable Users

[Hi Stakes Markets Game](https://www.histakesgame.com) -  The worlds most advanced financial simulator, available on iPhone and Android.
