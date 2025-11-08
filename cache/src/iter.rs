//! Contains types for iterating over a cache's contents.

use crate::handles::{AsyncCache, Cache};

use std::collections::VecDeque;
use std::future::Future;
use std::hash::{BuildHasher, Hash};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A resumable cursor representing a position within the cache's iteration.
///
/// This struct is `Copy`, `Clone`, and `Default`, allowing it to be easily
/// stored and used to resume an iteration later. A default cursor starts
/// at the beginning of the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
  shard_index: usize,
  items_seen_in_shard: usize,
}

pub const DEFAULT_ITER_BATCH_SIZE: usize = 64;

// --- Synchronous Iterator ---

/// A synchronous iterator over the key-value pairs of a `Cache`.
///
/// This iterator is designed for low-impact, concurrent-friendly iteration.
/// It works by fetching items in batches, holding a lock on only one shard at
/// a time for a very brief period.
///
/// **Important**: This iterator does **not** provide a point-in-time snapshot
/// of the cache. Items inserted after a shard has been scanned will be missed.
/// Items may be modified or deleted by other threads while iteration is in
/// progress.
pub struct Iter<'a, K: Send, V: Send + Sync, H> {
  cache: &'a Cache<K, V, H>,
  buffer: VecDeque<(K, Arc<V>)>,
  cursor: Cursor,
  batch_size: usize,
  finished: bool,
}

impl<'a, K, V, H> Iter<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  pub(crate) fn new(cache: &'a Cache<K, V, H>, batch_size: usize) -> Self {
    Self {
      cache,
      buffer: VecDeque::with_capacity(batch_size),
      cursor: Cursor::default(),
      batch_size,
      finished: false,
    }
  }

  /// Fills the internal buffer with the next batch of items from the cache.
  fn refill_buffer(&mut self) {
    if self.finished {
      return;
    }

    let num_shards = self.cache.shared.store.shards.len();

    while self.cursor.shard_index < num_shards && self.buffer.len() < self.batch_size {
      let shard = &self.cache.shared.store.shards[self.cursor.shard_index];
      let guard = shard.map.read();

      let items_in_shard = guard.len();
      if self.cursor.items_seen_in_shard >= items_in_shard {
        self.cursor.shard_index += 1;
        self.cursor.items_seen_in_shard = 0;
        continue;
      }

      let batch = guard
        .iter()
        .skip(self.cursor.items_seen_in_shard)
        .take(self.batch_size - self.buffer.len())
        .filter_map(|(key, entry)| {
          if entry.is_expired(self.cache.shared.time_to_idle) {
            None
          } else {
            Some((key.clone(), entry.value()))
          }
        })
        .collect::<VecDeque<_>>();

      self.cursor.items_seen_in_shard += batch.len();
      self.buffer.extend(batch);
    } // Lock on shard is released here

    if self.cursor.shard_index >= num_shards {
      self.finished = true;
    }
  }
}

impl<'a, K, V, H> Iterator for Iter<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  type Item = (K, Arc<V>);

  fn next(&mut self) -> Option<Self::Item> {
    // Fast path: serve from the buffer if possible
    if let Some(item) = self.buffer.pop_front() {
      return Some(item);
    }

    // If buffer is empty but we're finished, end now.
    if self.finished {
      return None;
    }

    // Slow path: refill the buffer and try again
    self.refill_buffer();
    self.buffer.pop_front()
  }
}

// --- Asynchronous Stream ---

/// An asynchronous stream over the key-value pairs of an `AsyncCache`.
///
/// This stream is the `async` equivalent of [`Iter`]. It is designed for
/// low-impact, concurrent-friendly iteration and shares the same weak
/// consistency guarantees.
pub struct IterStream<K: Send, V: Send + Sync, H> {
  cache: AsyncCache<K, V, H>,
  buffer: VecDeque<(K, Arc<V>)>,
  cursor: Cursor,
  batch_size: usize,
  finished: bool,
  refill_future:
    Option<Pin<Box<dyn Future<Output = (VecDeque<(K, Arc<V>)>, Cursor, bool)> + Send + 'static>>>,
}

impl<K, V, H> IterStream<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
  H: BuildHasher + Clone + Send,
{
  /// Accepts a `&AsyncCache` and clones it so callers don't need to change.
  pub(crate) fn new(cache: &AsyncCache<K, V, H>, batch_size: usize) -> Self {
    Self {
      cache: cache.clone(),
      buffer: VecDeque::with_capacity(batch_size),
      cursor: Cursor::default(),
      batch_size,
      finished: false,
      refill_future: None,
    }
  }
}

impl<K, V, H> futures_util::Stream for IterStream<K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync + 'static,
  V: Send + Sync + 'static,
  H: BuildHasher + Clone + Send + Sync + 'static,
{
  type Item = (K, Arc<V>);

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    let this = unsafe { self.as_mut().get_unchecked_mut() };

    // Fast path
    if let Some(item) = this.buffer.pop_front() {
      return Poll::Ready(Some(item));
    }

    if this.finished {
      return Poll::Ready(None);
    }

    // If a refill is in flight, poll it
    if let Some(ref mut fut) = this.refill_future {
      match fut.as_mut().poll(cx) {
        Poll::Ready((batch, new_cursor, finished_flag)) => {
          this.refill_future = None;
          this.cursor = new_cursor;
          this.finished = finished_flag;
          this.buffer.extend(batch);
          if let Some(item) = this.buffer.pop_front() {
            Poll::Ready(Some(item))
          } else {
            debug_assert!(this.finished);
            Poll::Ready(None)
          }
        }
        Poll::Pending => return Poll::Pending,
      }
    } else {
      // No refill in flight: create one that doesn't borrow `this`.
      let cache_clone = this.cache.clone();
      let cursor_snapshot = this.cursor;
      let batch_size = this.batch_size;
      // If time_to_idle is Copy/Clone, capture it; otherwise clone appropriately.
      let time_to_idle = cache_clone.shared.time_to_idle;

      let mut fut = Box::pin(async move {
        let mut cursor = cursor_snapshot;
        let mut local_buf = VecDeque::new();

        let num_shards = cache_clone.shared.store.shards.len();
        while cursor.shard_index < num_shards && local_buf.len() < batch_size {
          let shard = &cache_clone.shared.store.shards[cursor.shard_index];
          let guard = shard.map.read_async().await;

          let items_in_shard = guard.len();
          if cursor.items_seen_in_shard >= items_in_shard {
            cursor.shard_index += 1;
            cursor.items_seen_in_shard = 0;
            continue;
          }

          let batch = guard
            .iter()
            .skip(cursor.items_seen_in_shard)
            .take(batch_size - local_buf.len())
            .filter_map(|(key, entry)| {
              if entry.is_expired(time_to_idle) {
                None
              } else {
                Some((key.clone(), entry.value()))
              }
            })
            .collect::<VecDeque<_>>();

          cursor.items_seen_in_shard += batch.len();
          local_buf.extend(batch);
        }

        let finished = cursor.shard_index >= num_shards;
        (local_buf, cursor, finished)
      });

      // Poll new future immediately
      match fut.as_mut().poll(cx) {
        Poll::Ready((batch, new_cursor, finished_flag)) => {
          this.cursor = new_cursor;
          this.finished = finished_flag;
          this.buffer.extend(batch);
          if let Some(item) = this.buffer.pop_front() {
            Poll::Ready(Some(item))
          } else {
            debug_assert!(this.finished);
            Poll::Ready(None)
          }
        }
        Poll::Pending => {
          this.refill_future = Some(fut);
          Poll::Pending
        }
      }
    }
  }
}

// --- Synchronous Shard-Snapshot Iterator ---

/// A synchronous iterator over a semi-consistent snapshot of the cache's keys.
///
/// This iterator provides stronger consistency guarantees than [`Iter`]. It works
/// by taking a point-in-time snapshot of all keys from one shard at a time.
///
/// # Consistency Guarantees
/// - The set of keys for any given shard is fixed at the moment that shard is first
///   scanned. Inserts into a shard that has already been scanned will be missed.
/// - Inserts into shards that have *not yet* been scanned may be included.
/// - The collection of all items yielded does not represent a single point-in-time
///   snapshot of the entire cache.
/// - The *values* are fetched at the moment `next()` is called and may have been
///   updated or deleted after the keys were snapshotted.
pub struct SnapshotIter<'a, K: Send, V: Send + Sync, H> {
  cache: &'a Cache<K, V, H>,
  shard_keys: Vec<K>,
  key_idx: usize,
  shard_idx: usize,
}

impl<'a, K, V, H> SnapshotIter<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  pub(crate) fn new(cache: &'a Cache<K, V, H>) -> Self {
    Self {
      cache,
      shard_keys: Vec::new(),
      key_idx: 0,
      shard_idx: 0,
    }
  }

  /// Fills the internal `shard_keys` buffer by snapshotting the next available shard.
  /// Returns `true` if a shard was successfully loaded, `false` if there are no more shards.
  fn load_next_shard(&mut self) -> bool {
    let num_shards = self.cache.shared.store.shards.len();
    while self.shard_idx < num_shards {
      let shard = &self.cache.shared.store.shards[self.shard_idx];
      self.shard_idx += 1;

      let guard = shard.map.read();

      // If the shard is empty, we don't need to do anything with it.
      // The outer while loop will cause us to check the next one.
      if !guard.is_empty() {
        self.shard_keys = guard.keys().cloned().collect();
        self.key_idx = 0;
        return true;
      }
    }
    // We've run out of shards to check.
    false
  }
}

impl<'a, K, V, H> Iterator for SnapshotIter<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send,
  V: Send + Sync,
  H: BuildHasher + Clone,
{
  type Item = (K, Arc<V>);

  fn next(&mut self) -> Option<Self::Item> {
    loop {
      // Try to get the next key from the current shard's snapshot.
      if let Some(key) = self.shard_keys.get(self.key_idx) {
        self.key_idx += 1;
        // The value might have been deleted between the snapshot and now.
        // If so, we simply skip it and try the next key.
        if let Some(value) = self.cache.fetch(key) {
          return Some((key.clone(), value));
        }
      } else {
        // We've exhausted the keys for the current shard. Try to load the next one.
        if self.load_next_shard() {
          // `load_next_shard` loaded a new set of keys, so we loop
          // again to start processing them.
          continue;
        } else {
          // No more shards to load, the iteration is complete.
          return None;
        }
      }
    }
  }
}

// --- Asynchronous Shard-Snapshot Iterator ---

/// An asynchronous iterator over a semi-consistent snapshot of the cache's keys.
///
/// This provides stronger consistency than `IterStream` by snapshotting keys from
/// one shard at a time. It is not a `Stream` but provides an async `next()` method.
pub struct AsyncSnapshotIter<'a, K: Send, V: Send + Sync, H> {
  cache: &'a AsyncCache<K, V, H>,
  shard_keys: Vec<K>,
  key_idx: usize,
  shard_idx: usize,
}

impl<'a, K, V, H> AsyncSnapshotIter<'a, K, V, H>
where
  K: Eq + Hash + Clone + Send + Sync,
  V: Send + Sync,
  H: BuildHasher + Clone + Send,
{
  pub(crate) fn new(cache: &'a AsyncCache<K, V, H>) -> Self {
    Self {
      cache,
      shard_keys: Vec::new(),
      key_idx: 0,
      shard_idx: 0,
    }
  }

  /// The async equivalent of the `Iterator::next()` method.
  pub async fn next(&mut self) -> Option<(K, Arc<V>)> {
    loop {
      if let Some(key) = self.shard_keys.get(self.key_idx) {
        self.key_idx += 1;
        // Fetch the value asynchronously. It might have been deleted.
        if let Some(value) = self.cache.fetch(key).await {
          return Some((key.clone(), value));
        }
        // If fetch returned None, loop again to try the next key.
      } else {
        // We've run out of keys for this shard. Load the next one.
        if self.load_next_shard().await {
          continue;
        } else {
          return None; // No more shards.
        }
      }
    }
  }

  async fn load_next_shard(&mut self) -> bool {
    let num_shards = self.cache.shared.store.shards.len();
    while self.shard_idx < num_shards {
      let shard = &self.cache.shared.store.shards[self.shard_idx];
      self.shard_idx += 1;

      let guard = shard.map.read_async().await;
      if !guard.is_empty() {
        self.shard_keys = guard.keys().cloned().collect();
        self.key_idx = 0;
        return true;
      }
    }
    false
  }
}
