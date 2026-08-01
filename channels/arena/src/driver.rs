use crate::channel::{AsyncChannel, Payload, SyncChannel};
use crate::spec::{Api, Cell, Semantics};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
  Unsupported,
  /// Item accounting did not match, so the timing is meaningless.
  Miscounted,
}

pub type RunResult = Result<Duration, RunError>;

struct Plan {
  per_producer: u64,
  expected: u64,
}

fn plan(cell: &Cell, items: u64) -> Plan {
  let producers = cell.pairing.producers as u64;
  let per_producer = (items / producers).max(1);
  let sent = per_producer * producers;
  let expected = match cell.flavor.semantics() {
    Semantics::WorkSharing => sent,
    Semantics::Broadcast => sent * cell.pairing.consumers as u64,
  };
  Plan {
    per_producer,
    expected,
  }
}

/// Total items actually pushed through the channel for a given request, which
/// is what throughput is computed against.
pub fn sent_items(cell: &Cell, items: u64) -> u64 {
  let producers = cell.pairing.producers as u64;
  (items / producers).max(1) * producers
}

fn produce_sync<C: SyncChannel>(tx: &mut C::Sender, count: u64, api: Api) {
  match api {
    Api::Single => {
      for i in 0..count {
        if !C::send(tx, i as Payload) {
          return;
        }
      }
    }
    Api::Batch(size) => {
      let mut buffer = Vec::with_capacity(size);
      let mut remaining = count;
      while remaining > 0 {
        let take = (size as u64).min(remaining);
        buffer.clear();
        buffer.extend(0..take);
        if !C::send_batch(tx, &mut buffer) {
          return;
        }
        remaining -= take;
      }
    }
  }
}

fn consume_sync<C: SyncChannel>(rx: &mut C::Receiver, api: Api) -> u64 {
  let mut received = 0u64;
  match api {
    Api::Single => {
      while C::recv(rx).is_some() {
        received += 1;
      }
    }
    Api::Batch(size) => {
      let mut buffer = Vec::with_capacity(size);
      loop {
        buffer.clear();
        let got = C::recv_batch(rx, &mut buffer, size);
        if got == 0 {
          break;
        }
        received += got as u64;
      }
    }
  }
  received
}

pub fn run_sync<C: SyncChannel>(cell: &Cell, items: u64) -> RunResult {
  let (senders, receivers) = C::build(cell.capacity, cell.pairing.producers, cell.pairing.consumers)
    .ok_or(RunError::Unsupported)?;
  let plan = plan(cell, items);

  let barrier = Arc::new(Barrier::new(cell.pairing.threads() + 1));
  let received = Arc::new(AtomicU64::new(0));
  let mut threads = Vec::with_capacity(cell.pairing.threads());
  let api = cell.api;

  for mut tx in senders {
    let barrier = Arc::clone(&barrier);
    let count = plan.per_producer;
    threads.push(std::thread::spawn(move || {
      barrier.wait();
      produce_sync::<C>(&mut tx, count, api);
    }));
  }

  for mut rx in receivers {
    let barrier = Arc::clone(&barrier);
    let received = Arc::clone(&received);
    threads.push(std::thread::spawn(move || {
      barrier.wait();
      let n = consume_sync::<C>(&mut rx, api);
      received.fetch_add(n, Ordering::Relaxed);
    }));
  }

  barrier.wait();
  let started = Instant::now();
  for t in threads {
    let _ = t.join();
  }
  let elapsed = started.elapsed();

  if received.load(Ordering::Relaxed) != plan.expected {
    return Err(RunError::Miscounted);
  }
  Ok(elapsed)
}

async fn produce_async<C: AsyncChannel>(tx: &mut C::Sender, count: u64, api: Api) {
  match api {
    Api::Single => {
      for i in 0..count {
        if !C::send(tx, i as Payload).await {
          return;
        }
      }
    }
    Api::Batch(size) => {
      let mut buffer = Vec::with_capacity(size);
      let mut remaining = count;
      while remaining > 0 {
        let take = (size as u64).min(remaining);
        buffer.clear();
        buffer.extend(0..take);
        if !C::send_batch(tx, &mut buffer).await {
          return;
        }
        remaining -= take;
      }
    }
  }
}

async fn consume_async<C: AsyncChannel>(rx: &mut C::Receiver, api: Api) -> u64 {
  let mut received = 0u64;
  match api {
    Api::Single => {
      while C::recv(rx).await.is_some() {
        received += 1;
      }
    }
    Api::Batch(size) => {
      let mut buffer = Vec::with_capacity(size);
      loop {
        buffer.clear();
        let got = C::recv_batch(rx, &mut buffer, size).await;
        if got == 0 {
          break;
        }
        received += got as u64;
      }
    }
  }
  received
}

pub fn run_async<C: AsyncChannel>(rt: &Runtime, cell: &Cell, items: u64) -> RunResult {
  let (senders, receivers) = C::build(cell.capacity, cell.pairing.producers, cell.pairing.consumers)
    .ok_or(RunError::Unsupported)?;
  let plan = plan(cell, items);
  let api = cell.api;

  rt.block_on(async move {
    let barrier = Arc::new(tokio::sync::Barrier::new(cell.pairing.threads() + 1));
    let received = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::with_capacity(cell.pairing.threads());

    for mut tx in senders {
      let barrier = Arc::clone(&barrier);
      let count = plan.per_producer;
      tasks.push(tokio::spawn(async move {
        barrier.wait().await;
        produce_async::<C>(&mut tx, count, api).await;
      }));
    }

    for mut rx in receivers {
      let barrier = Arc::clone(&barrier);
      let received = Arc::clone(&received);
      tasks.push(tokio::spawn(async move {
        barrier.wait().await;
        let n = consume_async::<C>(&mut rx, api).await;
        received.fetch_add(n, Ordering::Relaxed);
      }));
    }

    barrier.wait().await;
    let started = Instant::now();
    for t in tasks {
      let _ = t.await;
    }
    let elapsed = started.elapsed();

    if received.load(Ordering::Relaxed) != plan.expected {
      return Err(RunError::Miscounted);
    }
    Ok(elapsed)
  })
}
