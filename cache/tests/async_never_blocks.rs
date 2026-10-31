use fibre_cache::{AsyncCache, CacheBuilder};
use futures_util::StreamExt;
use std::sync::mpsc;
use std::time::Duration;

fn with_held_entry<F, Fut>(introspect: bool, op: F)
where
  F: FnOnce(AsyncCache<u64, u64>) -> Fut + Send + 'static,
  Fut: std::future::Future<Output = ()>,
{
  let (done_tx, done_rx) = mpsc::channel();
  std::thread::spawn(move || {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async move {
      let cache = CacheBuilder::<u64, u64>::new().shards(1).time_to_idle(Duration::from_secs(60)).maintenance_on_introspection(introspect).build_async().unwrap();
      cache.insert(2, 2, 1).await;
      let holder_cache = cache.clone();
      let holder = async move {
        let entry = holder_cache.entry(1).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(entry);
      };
      let operation = async move {
        tokio::task::yield_now().await;
        op(cache).await;
      };
      tokio::join!(holder, operation);
    });
    let _ = done_tx.send(());
  });
  done_rx.recv_timeout(Duration::from_secs(5)).expect("the runtime thread blocked on a held AsyncEntry");
}

#[test]
fn test_run_maintenance_yields_to_a_held_entry() {
  with_held_entry(false, |c| async move { c.run_maintenance().await });
}

#[test]
fn test_metrics_with_introspection_does_not_block_on_a_held_entry() {
  with_held_entry(true, |c| async move { let _ = c.metrics(); });
}

#[test]
fn test_iter_stream_with_introspection_does_not_block_on_a_held_entry() {
  with_held_entry(true, |c| async move {
    let mut stream = c.iter_stream();
    while stream.next().await.is_some() {}
  });
}

#[test]
fn test_iter_snapshot_with_introspection_does_not_block_on_a_held_entry() {
  with_held_entry(true, |c| async move {
    let mut iter = c.iter_snapshot_async();
    while iter.next().await.is_some() {}
  });
}

#[test]
fn test_to_snapshot_with_introspection_yields_to_a_held_entry() {
  with_held_entry(true, |c| async move { let _ = c.to_snapshot().await; });
}
