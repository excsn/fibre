//! Oneshot adapters, one per library and one per fibre variant.
//!
//! fibre ships four, and they differ in what backs a pair rather than in what
//! the pair does, so each reports under its own name and gets its own column.
//! kanal, flume, crossbeam, async-channel and std have no oneshot; a
//! `bounded(1)` is a standing channel that happens to hold one item, which is a
//! different thing, so they are absent rather than approximated.

use crate::adapters::macros::{oneshot_async_adapter, oneshot_sync_adapter};
use crate::channel::Payload;

use fibre::oneshot::{OneshotHostPool, OneshotPairPool, PoolSlot};

pub const FIBRE: &str = "fibre";
pub const FIBRE_EXCLUSIVE: &str = "fibre-exclusive";
pub const FIBRE_POOL: &str = "fibre-pool";
pub const FIBRE_POOL_HOST: &str = "fibre-pool-host";
pub const TOKIO: &str = "tokio";
pub const FUTURES: &str = "futures";
pub const ONESHOT: &str = "oneshot";
pub const ASYNC_ONESHOT: &str = "async-oneshot";
pub const LITE_SYNC: &str = "lite-sync";
pub const SYNC_ONESHOT: &str = "sync-oneshot";

/// The host pool exists to embed a reply slot in a record the pool already
/// owns. Nothing else in the arena carries a record, so the record here is the
/// slot alone, which measures the pooling and leaves the embedding to fibre's
/// own request/reply bench.
type HostPool = OneshotHostPool<PoolSlot<Payload>, Payload>;

fn host_pool(live: usize) -> HostPool {
  OneshotHostPool::new(live, PoolSlot::new, |slot| slot)
}

oneshot_sync_adapter! {
  name: FibreSync,
  sender: fibre::oneshot::Sender<Payload>,
  receiver: fibre::oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(fibre::oneshot::oneshot::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv_blocking().ok(),
}

oneshot_async_adapter! {
  name: FibreAsync,
  sender: fibre::oneshot::Sender<Payload>,
  receiver: fibre::oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(fibre::oneshot::oneshot::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

oneshot_sync_adapter! {
  name: FibreExclusiveSync,
  sender: fibre::oneshot::ExclusiveSender<Payload>,
  receiver: fibre::oneshot::ExclusiveReceiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(fibre::oneshot::exclusive::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv_blocking().ok(),
}

oneshot_async_adapter! {
  name: FibreExclusiveAsync,
  sender: fibre::oneshot::ExclusiveSender<Payload>,
  receiver: fibre::oneshot::ExclusiveReceiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(fibre::oneshot::exclusive::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

oneshot_sync_adapter! {
  name: FibrePoolSync,
  sender: fibre::oneshot::PooledSender<Payload>,
  receiver: fibre::oneshot::PooledReceiver<Payload>,
  store: OneshotPairPool<Payload>,
  make_store: |live| Some(fibre::oneshot::pair_pool::<Payload>(live)),
  pair: |store| store.pair(),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv_blocking().ok(),
}

oneshot_async_adapter! {
  name: FibrePoolAsync,
  sender: fibre::oneshot::PooledSender<Payload>,
  receiver: fibre::oneshot::PooledReceiver<Payload>,
  store: OneshotPairPool<Payload>,
  make_store: |live| Some(fibre::oneshot::pair_pool::<Payload>(live)),
  pair: |store| store.pair(),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

oneshot_sync_adapter! {
  name: FibrePoolHostSync,
  sender: fibre::oneshot::HostSender<PoolSlot<Payload>, Payload>,
  receiver: fibre::oneshot::HostReceiver<PoolSlot<Payload>, Payload>,
  store: HostPool,
  make_store: |live| Some(host_pool(live)),
  pair: |store| store.pair_init(|_| {}),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv_blocking().ok(),
}

oneshot_async_adapter! {
  name: FibrePoolHostAsync,
  sender: fibre::oneshot::HostSender<PoolSlot<Payload>, Payload>,
  receiver: fibre::oneshot::HostReceiver<PoolSlot<Payload>, Payload>,
  store: HostPool,
  make_store: |live| Some(host_pool(live)),
  pair: |store| store.pair_init(|_| {}),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().await.ok(),
}

oneshot_sync_adapter! {
  name: TokioSync,
  sender: tokio::sync::oneshot::Sender<Payload>,
  receiver: tokio::sync::oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(tokio::sync::oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.blocking_recv().ok(),
}

oneshot_async_adapter! {
  name: TokioAsync,
  sender: tokio::sync::oneshot::Sender<Payload>,
  receiver: tokio::sync::oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(tokio::sync::oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.await.ok(),
}

oneshot_async_adapter! {
  name: FuturesAsync,
  sender: futures_channel::oneshot::Sender<Payload>,
  receiver: futures_channel::oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(futures_channel::oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.await.ok(),
}

oneshot_sync_adapter! {
  name: OneshotCrateSync,
  sender: oneshot::Sender<Payload>,
  receiver: oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}

oneshot_async_adapter! {
  name: OneshotCrateAsync,
  sender: oneshot::Sender<Payload>,
  receiver: oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.await.ok(),
}

oneshot_async_adapter! {
  name: AsyncOneshotAsync,
  sender: async_oneshot::Sender<Payload>,
  receiver: async_oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(async_oneshot::oneshot::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.await.ok(),
}

oneshot_sync_adapter! {
  name: LiteSyncSync,
  sender: lite_sync::oneshot::generic::Sender<Payload>,
  receiver: lite_sync::oneshot::generic::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(lite_sync::oneshot::generic::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.blocking_recv().ok(),
}

oneshot_async_adapter! {
  name: LiteSyncAsync,
  sender: lite_sync::oneshot::generic::Sender<Payload>,
  receiver: lite_sync::oneshot::generic::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(lite_sync::oneshot::generic::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.await.ok(),
}

oneshot_sync_adapter! {
  name: SyncOneshotSync,
  sender: sync_oneshot::Sender<Payload>,
  receiver: sync_oneshot::Receiver<Payload>,
  store: (),
  make_store: |_live| Some(()),
  pair: |_store| Some(sync_oneshot::channel::<Payload>()),
  send: |tx, item| tx.send(item).is_ok(),
  recv: |rx| rx.recv().ok(),
}
