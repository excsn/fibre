#[test]
#[cfg(not(debug_assertions))]
fn mpsc_tsan_stress_test() {
  // Run with RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test -Z build-std --release mpsc_tsan_stress_test --target aarch64-apple-darwin
  let (tx, rx) = fibre::mpsc::unbounded();
  let num_threads = 16;
  let items_per_thread = 1000000;
  let mut handles = vec![];

  for i in 0..num_threads {
    let mut tx_clone = tx.clone();
    handles.push(std::thread::spawn(move || {
      for j in 0..items_per_thread {
        // The send itself is the path we want to exercise.
        tx_clone.send((i, j)).unwrap();
        // A yield can help expose more interleavings.
        if j % 10 == 0 {
          std::thread::yield_now();
        }
      }
    }));
  }
  drop(tx);

  let mut count = 0;
  while rx.recv().is_ok() {
    count += 1;
    if count % 10 == 0 {
      std::thread::yield_now();
    }
  }

  for handle in handles {
    handle.join().unwrap();
  }

  assert_eq!(count, num_threads * items_per_thread);
}

#[test]
#[cfg(not(debug_assertions))]
fn mpmc_tsan_stress_test() {
  // Run with RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test --release mpmc_tsan_stress_test --target x86_64-unknown-linux-gnu
  // Designed to ruthlessly test the AtomicU8 state machines (STATE_WAITING, STATE_SUCCESS_HANDOFF)
  // in the MPMC queue under extreme contention and mixed batch sizes.
  let (tx, rx) = fibre::mpmc::bounded::<usize>(32);
  let num_threads = 8;
  let items_per_thread = 500_000; // Large enough to guarantee parking/waking
  let mut handles = vec![];

  // Spawn Producers
  for i in 0..num_threads {
    let tx_clone = tx.clone();
    handles.push(std::thread::spawn(move || {
      let mut next = i * items_per_thread;
      let end = next + items_per_thread;
      while next < end {
        // Mix single sends and batch sends to agitate the capacity boundaries
        if next % 3 == 0 {
          let batch: Vec<_> = (next..(next + 5).min(end)).collect();
          let sent = tx_clone.send_batch(batch).unwrap();
          next += sent;
        } else {
          tx_clone.send(next).unwrap();
          next += 1;
        }
      }
    }));
  }
  drop(tx);

  // Spawn Consumers
  let received_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
  for _ in 0..num_threads {
    let rx_clone = rx.clone();
    let r_count = received_count.clone();
    handles.push(std::thread::spawn(move || {
      let mut got = Vec::new();
      loop {
        match rx_clone.recv_batch_mut(&mut got, 16) {
          Ok(k) => {
            r_count.fetch_add(k, std::sync::atomic::Ordering::Relaxed);
            got.clear();
          }
          Err(_) => break, // Disconnected
        }
      }
    }));
  }
  drop(rx);

  for handle in handles {
    handle.join().unwrap();
  }

  assert_eq!(
    received_count.load(std::sync::atomic::Ordering::Relaxed),
    num_threads * items_per_thread
  );
}
