#[test]
#[cfg(not(debug_assertions))]
fn mpsc_tsan_stress_test() {
  // Run witn RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test --release mpsc_tsan_stress_test --target aarch64-apple-darwin
  let (tx, mut rx) = fibre::mpsc::unbounded_v1();
  let num_threads = 16;
  let items_per_thread = 1000000;
  let mut handles = vec![];

  for i in 0..num_threads {
    let tx_clone = tx.clone();
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
