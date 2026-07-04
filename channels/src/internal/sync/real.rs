//! Normal-build primitives: plain re-exports, zero cost. Keep this file's
//! export list in lockstep with `mocked.rs` — the two must expose identical
//! names/APIs or migrated channels break under one cfg but not the other.

pub(crate) use std::sync::atomic::{
  fence, AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering,
};

pub(crate) use std::hint;

/// For spin-budget consts: loom counts every tracked op against its branch
/// cap, so budgets like "spin 200 then park" must collapse to ~1 in models.
/// `const N: usize = if IS_LOOM { 1 } else { 200 };` keeps call sites cfg-free.
pub(crate) const IS_LOOM: bool = false;

pub(crate) use std::sync::Arc;

pub(crate) use std::thread;
pub(crate) use std::thread::Thread;

pub(crate) use parking_lot::Mutex;
