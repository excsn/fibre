# fibre

[![crates.io](https://img.shields.io/crates/v/fibre.svg)](https://crates.io/crates/fibre)
[![docs.rs](https://docs.rs/fibre/badge.svg)](https://docs.rs/fibre)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

Fibre provides a suite of high-performance, memory-efficient sync/async channels for Rust. It is designed to offer the best possible performance for a given concurrency pattern by providing specialized channel implementations rather than a single, general-purpose one. This allows developers to solve concurrency problems with tools that are tailored for their specific needs, from blazing-fast SPSC queues to flexible MPMC channels.

## Note

`fibre` is in BETA. The API is generally stable, but minor breaking changes may occur before version 1.0 as feedback is incorporated and improvements are made.