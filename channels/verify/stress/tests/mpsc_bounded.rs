//! Lost-wakeup gates for the bounded MPSC. 2 tokio workers is the setting
//! that reproduced the release-after-send hang (1 in ~2500 channels).

use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use fibre::mpsc::{bounded, bounded_async};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_cap1_many_producers_2_workers() {
  const ITEMS: usize = 100_000;
  for iter in 0..3000 {
    let producers = if iter % 2 == 0 { 4 } else { 14 };
    let (tx, rx) = bounded_async::<u64>(1);
    let consumer = tokio::spawn(async move {
      for _ in 0..ITEMS {
        rx.recv().await.unwrap();
      }
    });
    let mut handles = Vec::new();
    for p in 0..producers {
      let n = ITEMS / producers + usize::from(p < ITEMS % producers);
      let tx = tx.clone();
      handles.push(tokio::spawn(async move {
        for i in 0..n {
          tx.send(i as u64).await.unwrap();
        }
      }));
    }
    drop(tx);
    let all = async {
      for h in handles {
        h.await.unwrap();
      }
      consumer.await.unwrap();
    };
    if tokio::time::timeout(Duration::from_secs(5), all).await.is_err() {
      panic!("iteration {iter} hung ({producers} async producers, cap 1)");
    }
  }
}

#[test]
fn sync_small_caps_many_producers() {
  const ITEMS: usize = 50_000;
  for iter in 0..1000 {
    let cap = if iter % 2 == 0 { 1 } else { 4 };
    let producers = if iter % 4 < 2 { 4 } else { 14 };
    let (tx, rx) = bounded::<u64>(cap);
    let (done_tx, done_rx) = std_mpsc::channel();
    let consumer = thread::spawn(move || {
      for _ in 0..ITEMS {
        rx.recv().unwrap();
      }
      let _ = done_tx.send(());
    });
    for p in 0..producers {
      let n = ITEMS / producers + usize::from(p < ITEMS % producers);
      let tx = tx.clone();
      thread::spawn(move || {
        for i in 0..n {
          if tx.send(i as u64).is_err() {
            return;
          }
        }
      });
    }
    drop(tx);
    if done_rx.recv_timeout(Duration::from_secs(5)).is_err() {
      panic!("iteration {iter} hung (cap {cap}, {producers} sync producers)");
    }
    consumer.join().unwrap();
  }
}

/// The sender parks first (the sleep), then one sync recv must release it.
#[test]
fn async_send_parked_on_full_sync_receiver_resumes_on_single_drain() {
  for iter in 0..200 {
    let (tx, rx) = fibre::mpsc::bounded::<u32>(2);
    tx.try_send(1).unwrap();
    tx.try_send(2).unwrap();
    let atx = tx.to_async();
    let (done_tx, done_rx) = std_mpsc::channel();
    std::thread::spawn(move || {
      futures_executor::block_on(atx.send(3)).unwrap();
      let _ = done_tx.send(());
    });
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(rx.recv().unwrap(), 1);
    if done_rx.recv_timeout(Duration::from_secs(2)).is_err() {
      panic!("iteration {iter}: async send stayed pending after a single drain");
    }
    assert_eq!(rx.recv().unwrap(), 2);
    assert_eq!(rx.recv().unwrap(), 3);
  }
}
