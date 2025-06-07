use fibre::error::RecvError;
use fibre::spsc;
use std::thread;

mod common_async; // Assuming this helper exists

fn main() {
  let capacity = 2;

  println!("--- SPSC: Sync Sender, Sync Receiver ---");
  {
    let (mut tx, mut rx) = spsc::bounded_sync::<String>(capacity);

    let sender_handle = thread::spawn(move || {
      for i in 0..5 {
        let msg = format!("SyncSPSC-{}", i);
        println!("[Sync Send Thread] Sending: {}", msg);
        if tx.send(msg).is_err() {
          println!("[Sync Send Thread] Receiver dropped.");
          break;
        }
        thread::yield_now();
      }
      println!("[Sync Send Thread] Done sending.");
    });

    let receiver_handle = thread::spawn(move || {
      for i in 0..5 {
        match rx.recv() {
          Ok(value) => println!(
            "[Sync Recv Thread] Received: {} (expected SyncSPSC-{})",
            value, i
          ),
          Err(RecvError::Disconnected) => {
            println!("[Sync Recv Thread] Sender dropped.");
            break;
          }
        }
      }
      println!("[Sync Recv Thread] Done receiving.");
    });
    sender_handle.join().unwrap();
    receiver_handle.join().unwrap();
  }

  println!("\n--- SPSC: Async Sender, Async Receiver ---");
  common_async::run_async(async {
    let (tx, mut rx) = spsc::bounded_async::<String>(capacity);

    tokio::spawn(async move {
      // tx is moved here
      for i in 0..5 {
        let msg = format!("AsyncSPSC-{}", i);
        println!("[Async Send Task] Sending: {}", msg);
        if tx.send(msg).await.is_err() {
          println!("[Async Send Task] Receiver dropped.");
          break;
        }
        tokio::task::yield_now().await;
      }
      println!("[Async Send Task] Done sending.");
    });

    for i in 0..5 {
      match rx.recv().await {
        Ok(value) => println!(
          "[Async Recv Task] Received: {} (expected AsyncSPSC-{})",
          value, i
        ),
        Err(RecvError::Disconnected) => {
          println!("[Async Recv Task] Sender dropped.");
          break;
        }
      }
    }
    println!("[Async Recv Task] Done receiving.");
  });

  println!("\n--- SPSC: Sync Sender (Thread) to Async Receiver ---");
  common_async::run_async(async {
    // Start with a sync channel
    let (mut tx_s, rx_s) = spsc::bounded_sync::<String>(capacity);

    // Convert the receiver to async
    let mut rx_a = rx_s.to_async();
    // tx_s remains sync and is moved to the thread

    let sender_thread = thread::spawn(move || {
      // tx_s is moved here
      for i in 0..3 {
        let msg = format!("SyncToAsyncSPSC-{}", i);
        println!("[Sync Sender Thread] Sending: {}", msg);
        if tx_s.send(msg).is_err() {
          break;
        }
        thread::yield_now();
      }
      println!("[Sync Sender Thread] Done sending.");
      // tx_s drops here, signaling producer_dropped
    });

    println!("[Async Receiver Task] Waiting for messages...");
    for i in 0..3 {
      match rx_a.recv().await {
        Ok(val) => println!(
          "[Async Receiver Task] Received: {} (expected SyncToAsyncSPSC-{})",
          val, i
        ),
        Err(e) => {
          println!("[Async Receiver Task] Error: {:?}", e);
          break;
        }
      }
    }
    println!("[Async Receiver Task] Done receiving.");
    sender_thread.join().unwrap();
  });

  println!("\n--- SPSC: Async Sender to Sync Receiver ---");
  {
    // New scope for this test
    // Start with an async channel
    let (tx_a, rx_a) = spsc::bounded_async::<String>(capacity);

    // Convert the receiver to sync
    let mut rx_s = rx_a.to_sync();
    // tx_a remains async and is moved to the tokio task

    // Run the async producer in a separate OS thread with its own runtime
    let sender_os_thread_handle = thread::spawn(move || {
      // tx_a is moved here
      common_async::block_on_tokio_task(async move {
        // tx_a is moved into this async block
        for i in 0..3 {
          let msg = format!("AsyncToSyncSPSC-{}", i);
          println!("[Async Sender Task in OS Thread] Sending: {}", msg);
          if tx_a.send(msg).await.is_err() {
            break;
          }
          tokio::task::yield_now().await;
        }
        println!("[Async Sender Task in OS Thread] Done sending.");
        // tx_a drops here when the async block finishes
      });
    });

    println!("[Sync Receiver Thread] Waiting for messages...");
    for i in 0..3 {
      match rx_s.recv() {
        Ok(val) => println!(
          "[Sync Receiver Thread] Received: {} (expected AsyncToSyncSPSC-{})",
          val, i
        ),
        Err(e) => {
          println!("[Sync Receiver Thread] Error: {:?}", e);
          break;
        }
      }
    }
    println!("[Sync Receiver Thread] Done receiving.");
    sender_os_thread_handle.join().unwrap();
  }
  println!("\nSPSC examples finished.");
}
