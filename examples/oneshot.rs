// examples/oneshot_examples.rs
use fibre::error::{RecvError, TrySendError}; // Keep this
use fibre::oneshot;
use std::{thread, time::Duration};

mod common_async; // If in the same directory, or use `crate::common_async`

fn main() {
  println!("--- Oneshot: Sync Sender, Sync Receiver (Simulated with Async) ---");
  common_async::run_async(async {
    let (tx, mut rx) = oneshot::channel::<String>();
    let message = "Hello from sync-simulated oneshot!".to_string();

    let sender_handle = thread::spawn(move || {
      match tx.send(message.clone()) {
        Ok(_) => println!("[Sync Send Thread] Message sent."),
        Err(TrySendError::Closed(_)) => {
          println!("[Sync Send Thread] Send failed: receiver closed.")
        }
        Err(TrySendError::Sent(_)) => println!("[Sync Send Thread] Send failed: already sent."),
        Err(TrySendError::Full(_)) => {
          // This case should not be reachable for a oneshot channel.
          // The TrySendError enum is shared, hence this variant exists.
          eprintln!("[Sync Send Thread] Send failed: 'Full' - UNEXPECTED for oneshot!");
          unreachable!("Oneshot channel should not report TrySendError::Full");
        }
      }
    });

    match rx.recv().await {
      Ok(value) => println!("[Async Main] Received: {}", value),
      Err(RecvError::Disconnected) => println!("[Async Main] Channel disconnected."),
    }
    sender_handle.join().unwrap();
  });

  println!("\n--- Oneshot: Async Sender, Async Receiver ---");
  common_async::run_async(async {
    let (tx, mut rx) = oneshot::channel::<String>();
    let message = "Hello from async oneshot!".to_string();

    tokio::spawn(async move {
      match tx.send(message.clone()) {
        Ok(_) => println!("[Async Send Task] Message sent."),
        Err(TrySendError::Closed(val)) => println!(
          "[Async Send Task] Send failed: receiver closed. Value: {}",
          val
        ),
        Err(TrySendError::Sent(val)) => println!(
          "[Async Send Task] Send failed: already sent. Value: {}",
          val
        ),
        Err(TrySendError::Full(val)) => {
          eprintln!(
            "[Async Send Task] Send failed: 'Full' - UNEXPECTED for oneshot! Value: {}",
            val
          );
          unreachable!("Oneshot channel should not report TrySendError::Full");
        }
      }
    });

    match rx.recv().await {
      Ok(value) => println!("[Async Main] Received: {}", value),
      Err(RecvError::Disconnected) => println!("[Async Main] Channel disconnected."),
    }
  });

  println!("\n--- Oneshot: Sync Sender (Thread) to Async Receiver ---");
  common_async::run_async(async {
    let (tx, mut rx) = oneshot::channel::<String>();
    let message = "Sync sender -> async receiver".to_string();

    let sender_thread = thread::spawn(move || {
      println!("[Sync Thread] Sending message...");
      thread::sleep(Duration::from_millis(50));
      match tx.send(message) {
        Ok(_) => println!("[Sync Thread] Message sent."),
        Err(TrySendError::Closed(val)) => {
          println!("[Sync Thread] Send failed: receiver closed. Value: {}", val)
        }
        Err(TrySendError::Sent(val)) => {
          println!("[Sync Thread] Send failed: already sent. Value: {}", val)
        }
        Err(TrySendError::Full(val)) => {
          eprintln!(
            "[Sync Thread] Send failed: 'Full' - UNEXPECTED for oneshot! Value: {}",
            val
          );
          unreachable!("Oneshot channel should not report TrySendError::Full");
        }
      }
    });

    println!("[Async Task] Waiting for message...");
    match rx.recv().await {
      Ok(value) => println!("[Async Task] Received: {}", value),
      Err(e) => println!("[Async Task] Receive failed: {:?}", e),
    }
    sender_thread.join().unwrap();
  });

  println!("\n--- Oneshot: Async Sender to Sync Receiver (Simulated) ---");
  let (tx_async_to_sync, mut rx_async_for_sync) = oneshot::channel::<String>();
  let message_async_to_sync = "Async sender -> sync receiver (simulated)".to_string();

  common_async::block_on_tokio_task(async move {
    println!("[Async Task] Sending message...");
    tokio::time::sleep(Duration::from_millis(50)).await;
    match tx_async_to_sync.send(message_async_to_sync.clone()) {
      // Clone here
      Ok(_) => println!("[Async Task] Message sent."),
      Err(TrySendError::Closed(val)) => {
        println!("[Async Task] Send failed: receiver closed. Value: {}", val)
      }
      Err(TrySendError::Sent(val)) => {
        println!("[Async Task] Send failed: already sent. Value: {}", val)
      }
      Err(TrySendError::Full(val)) => {
        eprintln!(
          "[Async Task] Send failed: 'Full' - UNEXPECTED for oneshot! Value: {}",
          val
        );
        unreachable!("Oneshot channel should not report TrySendError::Full");
      }
    }
  });

  println!("[Sync Thread] Waiting for message...");
  match common_async::run_async(rx_async_for_sync.recv()) {
    // Changed variable name
    Ok(value) => println!("[Sync Thread] Received: {}", value),
    Err(e) => println!("[Sync Thread] Receive failed: {:?}", e),
  }

  println!("\n--- Oneshot: Dropped Sender ---");
  common_async::run_async(async {
    let (tx, mut rx) = oneshot::channel::<String>();
    drop(tx);
    match rx.recv().await {
      Err(RecvError::Disconnected) => {
        println!("Receiver correctly saw disconnected (sender dropped).")
      }
      Ok(v) => panic!("Should not receive value: {}", v),
      Err(e) => panic!("Unexpected error: {:?}", e),
    }
  });

  println!("\n--- Oneshot: Dropped Receiver ---");
  let (tx_for_dropped_rx, rx_for_dropped_rx) = oneshot::channel::<String>();
  drop(rx_for_dropped_rx);
  match tx_for_dropped_rx.send("test".to_string()) {
    // Changed variable name
    Err(TrySendError::Closed(v)) => println!(
      "Sender correctly saw closed (receiver dropped), value: {}",
      v
    ),
    Ok(_) => panic!("Send should have failed."),
    Err(TrySendError::Sent(v)) => {
      // This could also happen if another part of the code sent.
      eprintln!(
        "Sender saw 'Sent' after receiver dropped - unexpected. Value: {}",
        v
      );
      panic!("Send reported 'Sent' unexpectedly after receiver drop.");
    }
    Err(TrySendError::Full(v)) => {
      eprintln!(
        "Sender saw 'Full' after receiver dropped - UNEXPECTED for oneshot! Value: {}",
        v
      );
      unreachable!("Oneshot channel should not report TrySendError::Full");
    }
  }
}
