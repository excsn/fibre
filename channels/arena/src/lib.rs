//! Cross-implementation channel benchmark arena.
//!
//! One workload driver, one measurement loop, one report writer, and a thin
//! adapter per library. Adding an implementation means writing a `build`/
//! `send`/`recv` triple, nothing else.

pub mod adapters;
pub mod bench;
pub mod channel;
pub mod driver;
pub mod matrix;
pub mod measure;
pub mod registry;
pub mod report;
pub mod spec;
