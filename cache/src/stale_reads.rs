//! A stale read queues its key's refresh even while another caller holds the
//! key's pending-load lock, so a key read once is not left stale.

use crate::CacheBuilder;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

const HOLD: Duration = Duration::from_millis(200);
const TTL: Duration = Duration::from_millis(30);
const GRACE: Duration = Duration::from_secs(600);

#[test]
fn a_sync_stale_read_queues_its_refresh_past_a_held_pending_lock() {
  let calls = Arc::new(AtomicUsize::new(0));
  let counter = calls.clone();
  let cache = CacheBuilder::<u64, u64>::new()
    .time_to_live(TTL)
    .stale_while_revalidate(GRACE)
    .loader(move |k| {
      counter.fetch_add(1, Ordering::SeqCst);
      (k, 1)
    })
    .build()
    .unwrap();
  cache.insert(1, 1, 1);
  std::thread::sleep(TTL * 2);

  let hash = crate::store::hash_key(&cache.shared.store.hasher, &1u64);
  let index = hash as usize & (cache.shared.pending_loads.len() - 1);
  let holder = cache.clone();
  let (held_tx, held_rx) = mpsc::channel();
  let release = std::thread::spawn(move || {
    let guard = holder.shared.pending_loads[index].lock();
    held_tx.send(()).unwrap();
    std::thread::sleep(HOLD);
    drop(guard);
  });
  held_rx.recv().unwrap();

  assert_eq!(*cache.fetch_with(&1), 1);
  release.join().unwrap();
  std::thread::sleep(HOLD);
  assert_eq!(calls.load(Ordering::SeqCst), 1, "the only read of a stale key left it unrefreshed");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_stale_read_queues_its_refresh_past_a_held_pending_lock() {
  let calls = Arc::new(AtomicUsize::new(0));
  let counter = calls.clone();
  let cache = CacheBuilder::<u64, u64>::new()
    .time_to_live(TTL)
    .stale_while_revalidate(GRACE)
    .async_loader(move |k| {
      let counter = counter.clone();
      async move {
        counter.fetch_add(1, Ordering::SeqCst);
        (k, 1)
      }
    })
    .build_async()
    .unwrap();
  cache.insert(1, 1, 1).await;
  tokio::time::sleep(TTL * 2).await;

  let hash = crate::store::hash_key(&cache.shared.store.hasher, &1u64);
  let index = hash as usize & (cache.shared.pending_loads.len() - 1);
  let holder = cache.clone();
  let (held_tx, held_rx) = mpsc::channel();
  let release = std::thread::spawn(move || {
    let guard = holder.shared.pending_loads[index].lock();
    held_tx.send(()).unwrap();
    std::thread::sleep(HOLD);
    drop(guard);
  });
  held_rx.recv().unwrap();

  assert_eq!(*cache.fetch_with(&1).await, 1);
  tokio::task::spawn_blocking(move || release.join().unwrap()).await.unwrap();
  tokio::time::sleep(HOLD).await;
  assert_eq!(calls.load(Ordering::SeqCst), 1, "the only read of a stale key left it unrefreshed");
}
