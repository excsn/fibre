// examples/mpmc_examples.rs
use fibre::error::{RecvError, SendError};
use fibre::mpmc;
use std::{
  sync::atomic::{AtomicBool, AtomicUsize, Ordering},
  sync::Arc,
  thread,
  time::Duration,
};

mod common_async;

fn main() {
  let capacity = 4;
  let num_producers = 2;
  let num_consumers = 2;
  let messages_per_producer = 3;
  let total_messages = num_producers * messages_per_producer;

  println!("--- MPMC: Sync Producers, Sync Consumers ---");
  {
    let (tx, rx) = mpmc::channel::<String>(capacity);
    let received_count = Arc::new(AtomicUsize::new(0));
    let all_sent = Arc::new(AtomicBool::new(false));

    let mut producer_handles = Vec::new();
    for p_id in 0..num_producers {
      let tx_clone = tx.clone();
      producer_handles.push(thread::spawn(move || {
        for m_id in 0..messages_per_producer {
          let msg = format!("SyncMPMC-P{}-M{}", p_id, m_id);
          println!("[Sync Producer {}] Sending: {}", p_id, msg);
          if tx_clone.send(msg).is_err() {
            break;
          }
          thread::sleep(Duration::from_millis(10));
        }
      }));
    }
    drop(tx); // Drop original sender after cloning for producers

    let mut consumer_handles = Vec::new();
    for c_id in 0..num_consumers {
      let rx_clone = rx.clone();
      let received_count_clone = Arc::clone(&received_count);
      let all_sent_clone = Arc::clone(&all_sent);
      consumer_handles.push(thread::spawn(move || {
        loop {
          match rx_clone.recv() {
            Ok(value) => {
              println!("[Sync Consumer {}] Received: {}", c_id, value);
              received_count_clone.fetch_add(1, Ordering::Relaxed);
            }
            Err(RecvError::Disconnected) => {
              // Check if it's a "true" disconnect or just end of messages
              if received_count_clone.load(Ordering::Relaxed) < total_messages
                && !all_sent_clone.load(Ordering::Relaxed)
              {
                println!("[Sync Consumer {}] Disconnected prematurely.", c_id);
              } else {
                println!("[Sync Consumer {}] Disconnected (expected).", c_id);
              }
              break;
            }
          }
        }
      }));
    }
    drop(rx); // Drop original receiver

    for handle in producer_handles {
      handle.join().unwrap();
    }
    all_sent.store(true, Ordering::Relaxed); // Signal producers are done
                                             // Note: Consumers might still be draining. Their recv loop handles disconnect.
    for handle in consumer_handles {
      handle.join().unwrap();
    }
    assert_eq!(received_count.load(Ordering::Relaxed), total_messages);
  }

  println!("\n--- MPMC: Async Producers, Async Consumers ---");
  common_async::run_async(async {
    let (tx, rx) = mpmc::channel_async::<String>(capacity);
    let received_count = Arc::new(AtomicUsize::new(0));

    let mut producer_handles = Vec::new();
    for p_id in 0..num_producers {
      let tx_clone = tx.clone();
      producer_handles.push(tokio::spawn(async move {
        for m_id in 0..messages_per_producer {
          let msg = format!("AsyncMPMC-P{}-M{}", p_id, m_id);
          println!("[Async Producer {}] Sending: {}", p_id, msg);
          if tx_clone.send(msg).await.is_err() {
            break;
          }
          tokio::time::sleep(Duration::from_millis(10)).await;
        }
      }));
    }
    drop(tx);

    let mut consumer_handles = Vec::new();
    for c_id in 0..num_consumers {
      let rx_clone = rx.clone();
      let received_count_clone = Arc::clone(&received_count);
      consumer_handles.push(tokio::spawn(async move {
        while let Ok(value) = rx_clone.recv().await {
          println!("[Async Consumer {}] Received: {}", c_id, value);
          received_count_clone.fetch_add(1, Ordering::Relaxed);
        }
        println!("[Async Consumer {}] Disconnected.", c_id);
      }));
    }
    drop(rx);

    for handle in producer_handles {
      handle.await.unwrap();
    }
    for handle in consumer_handles {
      handle.await.unwrap();
    }
    assert_eq!(received_count.load(Ordering::Relaxed), total_messages);
  });

  println!("\n--- MPMC: Sync Producers (Threads) to Async Consumers ---");
  common_async::run_async(async {
    let (tx_async, rx_async) = mpmc::channel_async::<String>(capacity);
    let received_count = Arc::new(AtomicUsize::new(0));

    let mut producer_handles = Vec::new();
    for p_id in 0..num_producers {
      let tx_sync_converted = tx_async.clone().to_sync(); // Convert for each thread
      producer_handles.push(thread::spawn(move || {
        for m_id in 0..messages_per_producer {
          let msg = format!("SyncToAsyncMPMC-P{}-M{}", p_id, m_id);
          if tx_sync_converted.send(msg).is_err() {
            break;
          }
        }
      }));
    }
    drop(tx_async); // Drop original async sender after cloning for producers

    let mut consumer_handles = Vec::new();
    for c_id in 0..num_consumers {
      let rx_clone = rx_async.clone();
      let received_count_clone = Arc::clone(&received_count);
      consumer_handles.push(tokio::spawn(async move {
        while let Ok(value) = rx_clone.recv().await {
          received_count_clone.fetch_add(1, Ordering::Relaxed);
        }
      }));
    }
    drop(rx_async);

    for handle in producer_handles {
      handle.join().unwrap();
    }
    for handle in consumer_handles {
      handle.await.unwrap();
    }
    assert_eq!(received_count.load(Ordering::Relaxed), total_messages);
  });

  println!("\n--- MPMC: Async Producers to Sync Consumers ---");
  {
    let (tx_async, rx_async) = mpmc::channel_async::<String>(capacity);
    let received_count = Arc::new(AtomicUsize::new(0));
    let producers_finished_count = Arc::new(AtomicUsize::new(0)); // For tracking producer completion

    // Store std::thread::JoinHandle for the OS threads hosting async producers
    let mut os_thread_handles: Vec<thread::JoinHandle<()>> = Vec::new();

    for p_id in 0..num_producers {
      let tx_clone = tx_async.clone();
      let producers_finished_clone = producers_finished_count.clone();
      // Each async producer runs in its own OS thread with its own mini-runtime
      os_thread_handles.push(thread::spawn(move || {
        common_async::block_on_tokio_task(async move {
          for m_id in 0..messages_per_producer {
            let msg = format!("AsyncToSyncMPMC-P{}-M{}", p_id, m_id);
            println!("[Async Producer {} in OS Thread] Sending: {}", p_id, msg);
            if tx_clone.send(msg).await.is_err() {
              println!(
                "[Async Producer {} in OS Thread] Consumer side closed.",
                p_id
              );
              break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await; // Small delay
          }
          producers_finished_clone.fetch_add(1, Ordering::Relaxed);
          println!("[Async Producer {} in OS Thread] Done sending.", p_id);
        });
      }));
    }
    // Important: Drop the original tx_async *after* all producer threads have cloned it
    // and *before* consumers start trying to determine if all producers are truly done.
    // The drop order matters for MPMC disconnect logic.
    // If we drop tx_async here, the consumers might see "disconnected" too early if
    // the spawned threads haven't finished all their sends yet.
    // A better pattern: Producers signal completion, then drop tx_async.
    // For simplicity here, we'll drop it after joining the OS threads.

    let mut consumer_handles = Vec::new();
    for c_id in 0..num_consumers {
      let rx_sync_converted = rx_async.clone().to_sync(); // Convert for each sync consumer thread
      let received_count_clone = Arc::clone(&received_count);
      let producers_finished_count_clone = Arc::clone(&producers_finished_count); // Pass to consumer
      consumer_handles.push(thread::spawn(move || {
                loop {
                    match rx_sync_converted.recv() {
                        Ok(value) => {
                            println!("[Sync Consumer {}] Received: {}", c_id, value);
                            received_count_clone.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(RecvError::Disconnected) => {
                            // Check if producers are actually done or if it's a premature disconnect.
                            if producers_finished_count_clone.load(Ordering::Relaxed) == num_producers {
                                println!("[Sync Consumer {}] Disconnected (all producers finished).", c_id);
                            } else {
                                println!("[Sync Consumer {}] Disconnected (producers might not be done, or channel issue).", c_id);
                            }
                            break;
                        }
                    }
                }
            }));
    }
    drop(rx_async); // Drop original async receiver

    // Wait for all OS threads hosting async producers to complete
    for handle in os_thread_handles {
      handle.join().unwrap();
    }
    // Now that all producer OS threads are joined, all their sends *should* have completed or failed.
    // It's safer to drop the last tx_async here.
    drop(tx_async);

    for handle in consumer_handles {
      handle.join().unwrap();
    }
    assert_eq!(
      received_count.load(Ordering::Relaxed),
      total_messages,
      "Mismatch in received items count"
    );
    assert_eq!(
      producers_finished_count.load(Ordering::Relaxed),
      num_producers,
      "Not all producers signaled completion"
    );
    println!("[Main Thread] Async Producers to Sync Consumers example finished.");
  }
}
