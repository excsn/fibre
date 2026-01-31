// examples/mpsc_examples.rs
use fibre::error::{RecvError, SendError};
use fibre::mpsc;
use std::{
  sync::atomic::{AtomicUsize, Ordering},
  sync::Arc,
  thread,
  time::Duration,
};

mod common_async;

fn main() {
  println!("--- MPSC: Sync Senders, Sync Receiver ---");
  {
    let (tx, mut rx) = mpsc::unbounded_v1::<String>();
    let num_producers = 3;
    let messages_per_producer = 2;
    let total_messages = num_producers * messages_per_producer;
    let received_count = Arc::new(AtomicUsize::new(0));

    for i in 0..num_producers {
      let tx_clone = tx.clone();
      thread::spawn(move || {
        for j in 0..messages_per_producer {
          let msg = format!("SyncMPSC-P{}-M{}", i, j);
          println!("[Sync Sender {}] Sending: {}", i, msg);
          if tx_clone.send(msg).is_err() {
            println!("[Sync Sender {}] Receiver dropped.", i);
            break;
          }
          thread::sleep(Duration::from_millis(10 + i as u64 * 5));
        }
      });
    }
    drop(tx); // Drop original sender

    let recv_count_clone = Arc::clone(&received_count);
    let receiver_handle = thread::spawn(move || {
      for _ in 0..total_messages {
        match rx.recv() {
          Ok(value) => {
            println!("[Sync Receiver] Received: {}", value);
            recv_count_clone.fetch_add(1, Ordering::Relaxed);
          }
          Err(RecvError::Disconnected) => {
            println!("[Sync Receiver] All producers dropped.");
            break;
          }
        }
      }
    });
    receiver_handle.join().unwrap();
    assert_eq!(received_count.load(Ordering::Relaxed), total_messages);
  }

  println!("\n--- MPSC: Async Senders, Async Receiver ---");
  common_async::run_async(async {
    let (tx, mut rx) = mpsc::unbounded_v1_async::<String>();
    let num_producers = 3;
    let messages_per_producer = 2;
    let total_messages = num_producers * messages_per_producer;
    let received_count = Arc::new(AtomicUsize::new(0));

    for i in 0..num_producers {
      let tx_clone = tx.clone();
      tokio::spawn(async move {
        for j in 0..messages_per_producer {
          let msg = format!("AsyncMPSC-P{}-M{}", i, j);
          println!("[Async Sender {}] Sending: {}", i, msg);
          if tx_clone.send(msg).await.is_err() {
            println!("[Async Sender {}] Receiver dropped.", i);
            break;
          }
          tokio::time::sleep(Duration::from_millis(10 + i as u64 * 5)).await;
        }
      });
    }
    drop(tx); // Drop original async sender

    let recv_count_clone = Arc::clone(&received_count);
    for _ in 0..total_messages {
      match rx.recv().await {
        Ok(value) => {
          println!("[Async Receiver] Received: {}", value);
          recv_count_clone.fetch_add(1, Ordering::Relaxed);
        }
        Err(RecvError::Disconnected) => {
          println!("[Async Receiver] All producers dropped.");
          break;
        }
      }
    }
    assert_eq!(received_count.load(Ordering::Relaxed), total_messages);
  });

  println!("\n--- MPSC: Sync Senders (Threads) to Async Receiver ---");
  common_async::run_async(async {
    let (tx_async, mut rx_async) = mpsc::unbounded_v1_async::<String>(); // Start with async channel
    let num_producers = 2;

    for i in 0..num_producers {
      let tx_sync_converted = tx_async.clone().to_sync(); // Convert for each thread
      thread::spawn(move || {
        let msg = format!("SyncToAsyncMPSC-P{}", i);
        println!("[Sync Sender {}] Sending: {}", i, msg);
        if tx_sync_converted.send(msg).is_err() { /* ... */ }
      });
    }
    drop(tx_async); // Drop the original async sender last

    for _ in 0..num_producers {
      if let Ok(val) = rx_async.recv().await {
        println!("[Async Receiver] Received: {}", val);
      } else {
        break;
      }
    }
  });

  println!("\n--- MPSC: Async Senders to Sync Receiver ---");
  {
    let (tx_async, rx_async) = mpsc::unbounded_v1_async::<String>();
    let mut rx_sync = rx_async.to_sync(); // Convert receiver
    let num_producers = 2;

    for i in 0..num_producers {
      let tx_clone = tx_async.clone();
      // Need to run these async senders in a runtime if the main thread is sync
      common_async::block_on_tokio_task(async move {
        let msg = format!("AsyncToSyncMPSC-P{}", i);
        println!("[Async Sender {}] Sending: {}", i, msg);
        if tx_clone.send(msg).await.is_err() { /* ... */ }
      });
    }
    drop(tx_async);

    for _ in 0..num_producers {
      if let Ok(val) = rx_sync.recv() {
        println!("[Sync Receiver] Received: {}", val);
      } else {
        break;
      }
    }
  }
}
