// examples/spmc_examples.rs
use fibre::error::{RecvError, SendError};
use fibre::spmc;
use std::{
  sync::atomic::{AtomicUsize, Ordering},
  sync::Arc,
  thread,
  time::Duration,
};

mod common_async;

fn main() {
  let capacity = 2; // Small capacity to show backpressure if needed

  println!("--- SPMC: Sync Producer, Sync Receivers ---");
  {
    let (mut tx, rx1_orig) = spmc::channel::<String>(capacity);
    let mut rx1 = rx1_orig.clone(); // Keep original alive for other clones
    let mut rx2 = rx1_orig.clone();
    drop(rx1_orig); // Drop the temporary original
    let num_messages = 3;

    let producer_handle = thread::spawn(move || {
      for i in 0..num_messages {
        let msg = format!("SyncSPMC-M{}", i);
        println!("[Sync Producer] Sending: {}", msg);
        if tx.send(msg).is_err() {
          println!("[Sync Producer] All receivers dropped.");
          break;
        }
        thread::sleep(Duration::from_millis(20)); // Give receivers time
      }
    });

    let receiver1_handle = thread::spawn(move || {
      for _ in 0..num_messages {
        match rx1.recv() {
          Ok(value) => println!("[Sync Receiver 1] Received: {}", value),
          Err(RecvError::Disconnected) => {
            println!("[Sync Receiver 1] Producer dropped.");
            break;
          }
        }
      }
    });
    let receiver2_handle = thread::spawn(move || {
      for _ in 0..num_messages {
        match rx2.recv() {
          Ok(value) => println!("[Sync Receiver 2] Received: {}", value),
          Err(RecvError::Disconnected) => {
            println!("[Sync Receiver 2] Producer dropped.");
            break;
          }
        }
      }
    });
    producer_handle.join().unwrap();
    receiver1_handle.join().unwrap();
    receiver2_handle.join().unwrap();
  }

  println!("\n--- SPMC: Async Producer, Async Receivers ---");
  common_async::run_async(async {
    let (mut tx, rx1_orig) = spmc::channel_async::<String>(capacity);
    let mut rx1 = rx1_orig.clone();
    let mut rx2 = rx1_orig.clone();
    drop(rx1_orig);
    let num_messages = 3;

    tokio::spawn(async move {
      for i in 0..num_messages {
        let msg = format!("AsyncSPMC-M{}", i);
        println!("[Async Producer] Sending: {}", msg);
        if tx.send(msg).await.is_err() {
          println!("[Async Producer] All receivers dropped.");
          break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
      }
    });

    let recv1_task = tokio::spawn(async move {
      for _ in 0..num_messages {
        match rx1.recv().await {
          Ok(value) => println!("[Async Receiver 1] Received: {}", value),
          Err(RecvError::Disconnected) => {
            println!("[Async Receiver 1] Producer dropped.");
            break;
          }
        }
      }
    });
    let recv2_task = tokio::spawn(async move {
      for _ in 0..num_messages {
        match rx2.recv().await {
          Ok(value) => println!("[Async Receiver 2] Received: {}", value),
          Err(RecvError::Disconnected) => {
            println!("[Async Receiver 2] Producer dropped.");
            break;
          }
        }
      }
    });
    recv1_task.await.unwrap();
    recv2_task.await.unwrap();
  });

  println!("\n--- SPMC: Sync Producer (Thread) to Async Receivers ---");
  common_async::run_async(async {
    let (tx_async, rx_async1_orig) = spmc::channel_async::<String>(capacity);
    let mut tx_sync_converted = tx_async.to_sync(); // Convert producer
    let mut rx_async1 = rx_async1_orig.clone();
    let mut rx_async2 = rx_async1_orig.clone();
    drop(rx_async1_orig);
    let num_messages = 2;

    thread::spawn(move || {
      for i in 0..num_messages {
        let msg = format!("SyncToAsyncSPMC-M{}", i);
        println!("[Sync Producer] Sending: {}", msg);
        if tx_sync_converted.send(msg).is_err() {
          break;
        }
        thread::sleep(Duration::from_millis(30));
      }
      println!("[Sync Producer] Done sending.");
    });

    let r1 = tokio::spawn(async move {
      for _ in 0..num_messages {
        if let Ok(val) = rx_async1.recv().await {
          println!("[Async Receiver 1] Received: {}", val);
        } else {
          break;
        }
      }
    });
    let r2 = tokio::spawn(async move {
      for _ in 0..num_messages {
        if let Ok(val) = rx_async2.recv().await {
          println!("[Async Receiver 2] Received: {}", val);
        } else {
          break;
        }
      }
    });
    r1.await.unwrap();
    r2.await.unwrap();
    println!("[Async Receivers] Done.");
  });

  println!("\n--- SPMC: Async Producer to Sync Receivers ---");
  {
    let (tx_async, rx_async1_orig) = spmc::channel_async::<String>(capacity);
    let mut rx_sync1 = rx_async1_orig.clone().to_sync(); // Convert receivers
    let mut rx_sync2 = rx_async1_orig.to_sync();
    let num_messages = 2;

    // Run async producer in a separate tokio task/runtime
    let producer_task_handle = common_async::block_on_tokio_task(async move {
      let mut tx_async_local = tx_async; // Avoid capturing the original tx_async if it's used elsewhere
      for i in 0..num_messages {
        let msg = format!("AsyncToSyncSPMC-M{}", i);
        println!("[Async Producer] Sending: {}", msg);
        if tx_async_local.send(msg).await.is_err() {
          break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
      }
      println!("[Async Producer] Done sending.");
    });

    let r1_handle = thread::spawn(move || {
      for _ in 0..num_messages {
        if let Ok(val) = rx_sync1.recv() {
          println!("[Sync Receiver 1] Received: {}", val);
        } else {
          break;
        }
      }
    });
    let r2_handle = thread::spawn(move || {
      for _ in 0..num_messages {
        if let Ok(val) = rx_sync2.recv() {
          println!("[Sync Receiver 2] Received: {}", val);
        } else {
          break;
        }
      }
    });

    r1_handle.join().unwrap();
    r2_handle.join().unwrap();
    // producer_task_handle is joined by block_on_tokio_task
    println!("[Sync Receivers] Done.");
  }
}
