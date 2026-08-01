use crate::adapters::{async_channel_ch, crossbeam_ch, fibre_ch, flume_ch, kanal_ch, std_ch, tokio_ch};
use crate::bench::{AsyncEntry, Bench, SyncEntry};
use crate::spec::Flavor;

/// Shapes a general-purpose work-sharing channel is measured under. Broadcast
/// (SPMC) is excluded on purpose: a fan-out channel that clones every item to
/// every consumer is not the same workload, and libraries whose broadcast
/// channels drop for lagging consumers (tokio) are not comparable to ones that
/// apply backpressure (fibre).
const WORK_SHARING: [Flavor; 3] = [Flavor::Spsc, Flavor::Mpsc, Flavor::Mpmc];

/// Shapes servable by a channel with a single, non-cloneable receiver.
const SINGLE_CONSUMER: [Flavor; 2] = [Flavor::Spsc, Flavor::Mpsc];

pub fn registry() -> Vec<Box<dyn Bench>> {
  let mut entries: Vec<Box<dyn Bench>> = Vec::new();

  entries.push(SyncEntry::<fibre_ch::SpscSync>::boxed(fibre_ch::LIBRARY, Flavor::Spsc));
  entries.push(SyncEntry::<fibre_ch::SpscRendezvousSync>::boxed(fibre_ch::LIBRARY, Flavor::Spsc));
  entries.push(AsyncEntry::<fibre_ch::SpscAsync>::boxed(fibre_ch::LIBRARY, Flavor::Spsc));
  entries.push(AsyncEntry::<fibre_ch::SpscRendezvousAsync>::boxed(fibre_ch::LIBRARY, Flavor::Spsc));

  entries.push(SyncEntry::<fibre_ch::MpscSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));
  entries.push(SyncEntry::<fibre_ch::MpscUnboundedSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));
  entries.push(SyncEntry::<fibre_ch::MpscRendezvousSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));
  entries.push(AsyncEntry::<fibre_ch::MpscAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));
  entries.push(AsyncEntry::<fibre_ch::MpscUnboundedAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));
  entries.push(AsyncEntry::<fibre_ch::MpscRendezvousAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpsc));

  entries.push(SyncEntry::<fibre_ch::SpmcSync>::boxed(fibre_ch::LIBRARY, Flavor::Spmc));
  entries.push(AsyncEntry::<fibre_ch::SpmcAsync>::boxed(fibre_ch::LIBRARY, Flavor::Spmc));

  entries.push(SyncEntry::<fibre_ch::MpmcSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));
  entries.push(SyncEntry::<fibre_ch::MpmcUnboundedSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));
  entries.push(SyncEntry::<fibre_ch::MpmcRendezvousSync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));
  entries.push(AsyncEntry::<fibre_ch::MpmcAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));
  entries.push(AsyncEntry::<fibre_ch::MpmcUnboundedAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));
  entries.push(AsyncEntry::<fibre_ch::MpmcRendezvousAsync>::boxed(fibre_ch::LIBRARY, Flavor::Mpmc));

  for flavor in WORK_SHARING {
    entries.push(SyncEntry::<flume_ch::Sync_>::boxed(flume_ch::LIBRARY, flavor));
    entries.push(AsyncEntry::<flume_ch::Async_>::boxed(flume_ch::LIBRARY, flavor));
    entries.push(SyncEntry::<kanal_ch::Sync_>::boxed(kanal_ch::LIBRARY, flavor));
    entries.push(AsyncEntry::<kanal_ch::Async_>::boxed(kanal_ch::LIBRARY, flavor));
    entries.push(SyncEntry::<crossbeam_ch::Sync_>::boxed(crossbeam_ch::LIBRARY, flavor));
    entries.push(AsyncEntry::<async_channel_ch::Async_>::boxed(
      async_channel_ch::LIBRARY,
      flavor,
    ));
  }

  for flavor in SINGLE_CONSUMER {
    entries.push(SyncEntry::<std_ch::Sync_>::boxed(std_ch::LIBRARY, flavor));
    entries.push(SyncEntry::<std_ch::UnboundedSync>::boxed(std_ch::LIBRARY, flavor));
    entries.push(AsyncEntry::<tokio_ch::Async_>::boxed(tokio_ch::LIBRARY, flavor));
    entries.push(AsyncEntry::<tokio_ch::UnboundedAsync>::boxed(tokio_ch::LIBRARY, flavor));
  }

  entries
}
