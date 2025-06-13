//! This module contains the background tasks for the cache, such as the
//! janitor for handling TTL/TTI and the notifier for eviction callbacks.

pub(crate) mod janitor;
pub(crate) mod notifier;
pub(crate) mod timer;