#[cfg(test)]
mod spmc_deadlock_tests {
  use fibre::spmc;
  use std::sync::{Arc, Barrier, Condvar, Mutex as StdMutex}; // StdMutex for Condvar
  use std::thread;
  use std::time::{Duration, Instant};

  const ITEM_VALUE: u64 = 42;

  fn run_spmc_iteration(
    num_consumers: usize,
    capacity: usize,
    items: usize,
    iteration_idx: usize,
    iteration_timeout_secs: u64,
  ) -> bool {
    println!(
      "\n--- Iteration {}, C={}, Cap={}, Items={} ---",
      iteration_idx, num_consumers, capacity, items
    );

    let (mut tx, rx_orig) = spmc::channel(capacity);
    let barrier = Arc::new(Barrier::new(num_consumers + 1));
    let mut consumer_handles = Vec::new();

    // For checking if consumers actually finished their loops vs just join timed out
    let consumers_completed_work = Arc::new(
      (0..num_consumers)
        .map(|_| Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .collect::<Vec<_>>(),
    );

    for i in 0..num_consumers {
      let mut rx_clone = rx_orig.clone();
      let barrier_clone = Arc::clone(&barrier);
      let completed_flag = Arc::clone(&consumers_completed_work[i]);
      let consumer_name = format!("C{}-Iter{}", i, iteration_idx);

      consumer_handles.push(
        thread::Builder::new()
          .name(consumer_name)
          .spawn(move || {
            barrier_clone.wait();
            // println!("[{}] Ready", thread::current().name().unwrap_or("C?"));
            for item_count in 0..items {
              match rx_clone.recv() {
                Ok(_val) => { /* Process val */ }
                Err(e) => {
                  println!(
                    "[{}] Recv error after {} items: {:?}",
                    thread::current().name().unwrap_or("C?"),
                    item_count,
                    e
                  );
                  completed_flag.store(true, std::sync::atomic::Ordering::SeqCst); // Still mark as "done" from its perspective
                  return;
                }
              }
            }
            // println!("[{}] Finished normally", thread::current().name().unwrap_or("C?"));
            completed_flag.store(true, std::sync::atomic::Ordering::SeqCst);
          })
          .unwrap(),
      );
    }
    drop(rx_orig);

    barrier.wait();
    // println!("[P-Iter{}][{:?}] Starting send", iteration_idx, thread::current().id());
    for i in 0..items {
      if let Err(e) = tx.send(ITEM_VALUE + i as u64) {
        println!(
          "[P-Iter{}][{:?}] Send error after {} items: {:?}",
          iteration_idx,
          thread::current().id(),
          i,
          e
        );
        break;
      }
    }
    // println!("[P-Iter{}][{:?}] Finished send", iteration_idx, thread::current().id());
    drop(tx); // Signal consumers
              // println!("[P-Iter{}][{:?}] Producer dropped", iteration_idx, thread::current().id());

    // --- Joining with overall iteration timeout ---
    let main_thread = thread::current();
    let timeout_signal = Arc::new((StdMutex::new(false), Condvar::new()));
    let timeout_signal_clone = Arc::clone(&timeout_signal);

    let _guard = thread::spawn(move || {
      thread::sleep(Duration::from_secs(iteration_timeout_secs));
      let (lock, cvar) = &*timeout_signal_clone;
      *lock.lock().unwrap() = true;
      cvar.notify_one();
      main_thread.unpark(); // Attempt to unpark the main thread if it's stuck in joins
    });

    let mut all_threads_joined_cleanly = true;
    for (idx, handle) in consumer_handles.into_iter().enumerate() {
      // We can't easily timeout individual std::thread::JoinHandle.join()
      // So we rely on the overall iteration timeout.
      // If join() blocks forever, the outer loop will eventually break.
      // This join is blocking.
      if handle.join().is_err() {
        println!("[Main-Iter{}]: Consumer {} panicked.", iteration_idx, idx);
        all_threads_joined_cleanly = false;
      }
      // Check our own flag
      if !consumers_completed_work[idx].load(std::sync::atomic::Ordering::SeqCst) {
        println!(
          "[Main-Iter{}]: Consumer {} joined but did NOT complete its work (flag false).",
          iteration_idx, idx
        );
        all_threads_joined_cleanly = false; // Or consider this a type of failure
      }
    }

    let (lock, cvar) = &*timeout_signal;
    let mut timed_out = *lock.lock().unwrap();
    if timed_out {
      // If the timeout thread already signaled
      println!(
        "[Main-Iter{}]: Iteration TIMED OUT ({}s). Consumer join status might be incomplete.",
        iteration_idx, iteration_timeout_secs
      );
      return false; // Indicate suspected hang
    }

    if all_threads_joined_cleanly {
      println!("--- Iteration {} finished successfully. ---", iteration_idx);
      true
    } else {
      println!(
        "--- Iteration {} finished with errors/panics/incomplete work. ---",
        iteration_idx
      );
      false // Indicate failure but not necessarily a hang caught by timeout
    }
  }

  #[test]
  fn looped_repro_spmc_sync_hang_4c_1cap() {
    let num_loops = 100;
    let items_per_iter = 100_000;
    let timeout_per_iter_secs = 30 + (items_per_iter / 10000) as u64;
    let mut hangs_detected_by_timeout = 0;
    for i in 0..num_loops {
      if !run_spmc_iteration(4, 1, items_per_iter, i, timeout_per_iter_secs) {
        hangs_detected_by_timeout += 1;
        println!(
          "Suspected hang/timeout in iteration {}. Aborting further loops for this test.",
          i
        );
        // break; // Optional: stop on first hang
      }
    }
    if hangs_detected_by_timeout > 0 {
      panic!(
                "Test looped_repro_spmc_sync_hang_4c_1cap failed with {} suspected hangs/timeouts out of {} iterations.",
                hangs_detected_by_timeout, num_loops
            );
    }
  }

  #[test]
  fn looped_repro_spmc_sync_hang_14c_1cap() {
    let num_loops = 20;
    let items_per_iter = 100_000;
    let timeout_per_iter_secs = 30 + (items_per_iter / 10000) as u64;
    let mut hangs_detected_by_timeout = 0;
    for i in 0..num_loops {
      if !run_spmc_iteration(14, 1, items_per_iter, i, timeout_per_iter_secs) {
        hangs_detected_by_timeout += 1;
        println!(
          "Suspected hang/timeout in iteration {}. Aborting further loops for this test.",
          i
        );
        // break;
      }
    }
    if hangs_detected_by_timeout > 0 {
      panic!(
                "Test looped_repro_spmc_sync_hang_14c_1cap failed with {} suspected hangs/timeouts out of {} iterations.",
                hangs_detected_by_timeout, num_loops
            );
    }
  }
}
