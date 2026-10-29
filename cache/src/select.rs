//! Which built-in policy a shard runs when no `cache_policy_factory` is set
//! (S3-FIFO unless `CacheBuilder::tinylfu` was called) and how its reads reach it.

use crate::hpolicy::{Builtin, HS3Fifo, HTinyLfu};
use crate::shared::BuiltinReads;

pub(crate) type BuiltinPolicy = Builtin;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BuiltinChoice {
  TinyLfu,
  S3Fifo,
}

pub(crate) fn builtin(choice: BuiltinChoice, shard_capacity: u64) -> BuiltinPolicy {
  match choice {
    BuiltinChoice::TinyLfu => Builtin::TinyLfu(HTinyLfu::new(shard_capacity)),
    BuiltinChoice::S3Fifo => Builtin::S3Fifo(HS3Fifo::new(shard_capacity)),
  }
}

/// TinyLFU takes reads from the shard's ring under the write lock; S3-FIFO
/// bumps its per-node counter under the read lock.
pub(crate) fn reads(choice: BuiltinChoice) -> BuiltinReads {
  match choice {
    BuiltinChoice::TinyLfu => BuiltinReads::Ring,
    BuiltinChoice::S3Fifo => BuiltinReads::Freq,
  }
}
