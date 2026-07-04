//! Test suite for the bounded MPSC, relocated verbatim from the former
//! flat `bounded_queue.rs`. The inner modules reach the public API + 
//! `BoundedQueue::new` through `use super::*`.

#[allow(unused_imports)]
use super::*;

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize as TestAtomicUsize;
  use std::task::{RawWaker, RawWakerVTable};
  // Shadow the facade's `thread` (from the glob above) with std's: these tests
  // use real threads/sleeps/is_finished, which loom's mock doesn't provide.
  // They compile under `--cfg loom` but only run in normal `cargo test`.
  use std::thread;

  struct SimpleRng {
    state: u32,
  }

  impl SimpleRng {
    fn new(seed: u32) -> Self {
      Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
      self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
      self.state
    }

    fn gen_range(&mut self, min: u32, max: u32) -> u32 {
      assert!(min < max);
      min + (self.next_u32() % (max - min))
    }
  }

  struct DropTracker {
    counter: Arc<TestAtomicUsize>,
  }

  impl Drop for DropTracker {
    fn drop(&mut self) {
      self.counter.fetch_add(1, Ordering::SeqCst);
    }
  }

  fn mock_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
      RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop_raw(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
  }

  // --- Helper Functions for Benchmarking ---

  fn print_benchmark_row(label: &str, elapsed_ms: f64, throughput_melem: f64) {
    println!(
      "Configuration: {:<15} | Time: {:<8.3} ms | Throughput: {:<8.3} Melem/s",
      label, elapsed_ms, throughput_melem
    );
  }

  fn run_sync_benchmark(prod_count: usize, total_items: usize, capacity: usize) -> (f64, f64) {
    let (tx, rx) = bounded::<usize>(capacity);
    let items_per_producer = total_items / prod_count;
    let actual_total_items = items_per_producer * prod_count;

    let start = Instant::now();

    // 1. Spawn Consumer Thread
    let consumer_handle = thread::spawn(move || {
      let mut count = 0;
      while count < actual_total_items {
        if rx.recv().is_ok() {
          count += 1;
        }
      }
      count
    });

    // 2. Spawn Producer Threads
    let mut producers = Vec::new();
    for _ in 0..prod_count {
      let tx_clone = tx.clone();
      producers.push(thread::spawn(move || {
        for seq in 0..items_per_producer {
          let _ = tx_clone.send(seq);
        }
      }));
    }
    drop(tx); // Close original handle to ensure clean termination on completion

    // 3. Await all completions
    for p in producers {
      p.join().unwrap();
    }
    let received = consumer_handle.join().unwrap();
    let elapsed = start.elapsed();

    assert_eq!(received, actual_total_items);

    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = (actual_total_items as f64 / elapsed_secs) / 1_000_000.0;
    (elapsed_secs * 1000.0, throughput)
  }

  async fn run_async_benchmark(
    prod_count: usize,
    total_items: usize,
    capacity: usize,
  ) -> (f64, f64) {
    let (tx, rx) = bounded_async::<usize>(capacity);
    let items_per_producer = total_items / prod_count;
    let actual_total_items = items_per_producer * prod_count;

    let start = Instant::now();

    // 1. Spawn Consumer Task
    let consumer_handle = tokio::spawn(async move {
      let mut count = 0;
      while count < actual_total_items {
        if rx.recv().await.is_ok() {
          count += 1;
        }
      }
      count
    });

    // 2. Spawn Producer Tasks
    let mut producers = Vec::new();
    for _ in 0..prod_count {
      let tx_clone = tx.clone();
      producers.push(tokio::spawn(async move {
        for seq in 0..items_per_producer {
          let _ = tx_clone.send(seq).await;
        }
      }));
    }
    drop(tx);

    // 3. Await all completions
    for p in producers {
      p.await.unwrap();
    }
    let received = consumer_handle.await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(received, actual_total_items);

    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = (actual_total_items as f64 / elapsed_secs) / 1_000_000.0;
    (elapsed_secs * 1000.0, throughput)
  }

  // --- Basic Functionality Tests ---

  #[test]
  fn test_queue_initial_state() {
    let q = BoundedQueue::<i32>::new(4);
    assert_eq!(q.capacity(), 4); // Dynamically scaled chunk size means capacity is exactly 4!
    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
    assert!(!q.is_full());
  }

  #[test]
  fn test_try_push_pop_sequence() {
    let (tx, rx) = bounded::<i32>(2);
    tx.try_send(10).unwrap();
    assert_eq!(tx.len(), 1);
    assert!(!tx.is_empty());

    tx.try_send(20).unwrap();
    assert_eq!(tx.len(), 2);
    assert!(tx.is_full());

    // 3rd push must return Full since capacity is exactly 2
    assert!(matches!(tx.try_send(30), Err(TrySendError::Full(30))));

    assert_eq!(rx.try_recv().unwrap(), 10);
    assert_eq!(rx.try_recv().unwrap(), 20);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
  }

  #[test]
  fn test_push_pop_blocking_coordination() {
    let (tx, rx) = bounded(1);
    tx.send(100).unwrap();

    let tx_clone = tx.clone();
    let producer_thread = thread::spawn(move || {
      tx_clone.send(200).unwrap();
    });

    thread::sleep(Duration::from_millis(50));
    assert!(!producer_thread.is_finished());

    assert_eq!(rx.recv().unwrap(), 100);
    producer_thread.join().unwrap();
    assert_eq!(rx.recv().unwrap(), 200);
  }

  #[test]
  fn test_pop_timeout_behavior() {
    let (_tx, rx) = bounded::<i32>(1);
    let start = Instant::now();
    let res = rx.recv_timeout(Duration::from_millis(50));
    assert!(matches!(res, Err(RecvErrorTimeout::Timeout)));
    assert!(start.elapsed() >= Duration::from_millis(45));
  }

  #[test]
  fn test_receiver_drop_unblocks_pushers() {
    let (tx, rx) = bounded::<i32>(1);
    tx.send(1).unwrap();

    let tx_clone = tx.clone();
    let handle = thread::spawn(move || tx_clone.send(2));

    thread::sleep(Duration::from_millis(50));
    assert!(!handle.is_finished());

    drop(rx);

    let res = handle.join().unwrap();
    assert!(matches!(res, Err(SendError::Closed)));
  }

  // --- Async Manual Polling Validation ---

  #[test]
  fn test_manual_async_polling_state_machine() {
    // Capacity 16 fills exactly 1 chunk
    let (tx, rx) = bounded_async::<i32>(16);
    let waker = mock_waker();
    let mut cx = Context::from_waker(&waker);

    // Fill all 16 slots (one chunk)
    for i in 0..16 {
      let mut fut = Box::pin(tx.send(i));
      assert!(fut.as_mut().poll(&mut cx).is_ready());
    }

    // Now the 17th push must return Pending!
    let mut blocked_fut = Box::pin(tx.send(999));
    assert!(blocked_fut.as_mut().poll(&mut cx).is_pending());

    // Drain all 16 items so the receiver's local recycle pool fills and
    // releases a complete chunk back to the global stack.
    for i in 0..16 {
      let mut recv_fut = Box::pin(rx.recv());
      let recv_res = recv_fut.as_mut().poll(&mut cx);
      assert!(matches!(recv_res, Poll::Ready(Ok(v)) if v == i));
    }

    // Re-poll original pending push; it should now complete
    assert!(blocked_fut.as_mut().poll(&mut cx).is_ready());
  }

  // --- Concurrent Stress Fuzzing ---

  #[test]
  fn test_concurrent_differential_fuzz() {
    const TOTAL_ITEMS: usize = 50_000;
    const CAPACITY: usize = 128;
    const PRODUCER_COUNT: usize = 4;

    let (tx, rx) = bounded::<usize>(CAPACITY);
    let mut producer_handles = Vec::new();

    // Spawn Producers
    for thread_idx in 0..PRODUCER_COUNT {
      let tx_clone = tx.clone();
      producer_handles.push(thread::spawn(move || {
        let mut rng = SimpleRng::new(1337 + thread_idx as u32);
        let items_to_send = TOTAL_ITEMS / PRODUCER_COUNT;

        for seq in 0..items_to_send {
          let msg = (thread_idx << 32) | seq;

          if rng.gen_range(0, 100) < 30 {
            while let Err(e) = tx_clone.try_send(msg) {
              match e {
                TrySendError::Full(_) => thread::yield_now(),
                TrySendError::Closed(_) => panic!("Queue unexpectedly closed"),
                TrySendError::Sent(_) => {
                  unreachable!("TrySendError::Sent is not possible on a bounded queue")
                }
              }
            }
          } else {
            tx_clone.send(msg).unwrap();
          }

          if rng.gen_range(0, 100) < 10 {
            thread::yield_now();
          }
        }
      }));
    }
    drop(tx);

    // Spawn Consumer
    let consumer_handle = thread::spawn(move || {
      let mut expected_seqs = vec![0usize; PRODUCER_COUNT];
      let mut received_count = 0;
      let mut rng = SimpleRng::new(9999);

      while received_count < (TOTAL_ITEMS / PRODUCER_COUNT) * PRODUCER_COUNT {
        let pop_res = if rng.gen_range(0, 100) < 20 {
          match rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) => {
              thread::yield_now();
              None
            }
            Err(TryRecvError::Disconnected) => panic!("Queue disconnected prematurely"),
          }
        } else if rng.gen_range(0, 100) < 40 {
          match rx.recv_timeout(Duration::from_micros(10)) {
            Ok(msg) => Some(msg),
            Err(RecvErrorTimeout::Timeout) => None,
            Err(RecvErrorTimeout::Disconnected) => panic!("Queue disconnected prematurely"),
          }
        } else {
          Some(rx.recv().unwrap())
        };

        if let Some(msg) = pop_res {
          let origin_thread = msg >> 32;
          let seq = msg & 0xFFFF_FFFF;

          assert_eq!(
            seq, expected_seqs[origin_thread],
            "Out of order item received from thread {}: expected {}, got {}",
            origin_thread, expected_seqs[origin_thread], seq
          );

          expected_seqs[origin_thread] += 1;
          received_count += 1;
        }
      }
      received_count
    });

    for h in producer_handles {
      h.join().unwrap();
    }

    let total_received = consumer_handle.join().unwrap();
    assert_eq!(
      total_received,
      (TOTAL_ITEMS / PRODUCER_COUNT) * PRODUCER_COUNT
    );
  }

  // --- Discrete Synchronous & Asynchronous Performance Benchmarks ---

  #[test]
  fn test_mpsc_sync_performance_profile() {
    let profiles = [
      (1, "1 to 1 (SPSC)"),
      (4, "4 to 1 (MPSC)"),
      (14, "14 to 1 (MPSC)"),
    ];

    const TOTAL_ITEMS: usize = 2_000_000;
    const CAPACITY: usize = 128;

    println!("\n=== MPSC_queue Concurrency Benchmark (Sync) ===");
    println!(
      "Workload Size: {} elements | Capacity: {}",
      TOTAL_ITEMS, CAPACITY
    );

    for &(prod_count, label) in &profiles {
      let (elapsed_ms, throughput) = run_sync_benchmark(prod_count, TOTAL_ITEMS, CAPACITY);
      print_benchmark_row(label, elapsed_ms, throughput);
    }
    println!("========================================================\n");
  }

  #[tokio::test]
  async fn test_mpsc_async_performance_profile() {
    let profiles = [
      (1, "1 to 1 (SPSC)"),
      (4, "4 to 1 (MPSC)"),
      (14, "14 to 1 (MPSC)"),
    ];

    const TOTAL_ITEMS: usize = 2_000_000;
    const CAPACITY: usize = 128;

    println!("\n=== Ported MPSC_queue Concurrency Benchmark (Async) ===");
    println!(
      "Workload Size: {} elements | Capacity: {}",
      TOTAL_ITEMS, CAPACITY
    );

    for &(prod_count, label) in &profiles {
      let (elapsed_ms, throughput) = run_async_benchmark(prod_count, TOTAL_ITEMS, CAPACITY).await;
      print_benchmark_row(label, elapsed_ms, throughput);
    }
    println!("========================================================\n");
  }
}

#[cfg(test)]
mod sync_batch_tests {
  use super::*;

  #[test]
  fn test_sync_send_batch_recv_batch() {
    let (tx, rx) = bounded::<i32>(32);

    let batch = vec![1, 2, 3, 4, 5];
    tx.send_batch(batch).unwrap();

    let values = rx.recv_batch(5).unwrap();
    assert_eq!(values, vec![1, 2, 3, 4, 5]);
  }

  #[test]
  fn test_sync_send_batch_mut_recv_batch_mut() {
    let (tx, rx) = bounded::<i32>(32);

    let mut batch = vec![10, 20, 30];
    tx.send_batch_mut(&mut batch).unwrap();
    assert!(batch.is_empty());

    let mut out = Vec::new();
    let count = rx.recv_batch_mut(&mut out, 3).unwrap();
    assert_eq!(count, 3);
    assert_eq!(out, vec![10, 20, 30]);
  }

  #[test]
  fn test_sync_batch_partial_completions() {
    let (tx, rx) = bounded::<i32>(16);

    let mut fill = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    tx.send_batch_mut(&mut fill).unwrap();

    let tx_clone = tx.clone();
    let blocked_send = std::thread::spawn(move || tx_clone.send_batch(vec![17, 18, 19]));

    std::thread::sleep(Duration::from_millis(50));
    assert!(!blocked_send.is_finished());

    let mut out = Vec::new();
    let popped = rx.recv_batch_mut(&mut out, 3).unwrap();
    assert_eq!(popped, 3);
    assert_eq!(out, vec![1, 2, 3]);

    let send_result = blocked_send.join().unwrap().unwrap();
    assert_eq!(send_result, 3);
  }
}

#[cfg(test)]
mod batch_tests {
  use super::*;

  #[tokio::test]
  async fn test_async_send_batch_recv_batch() {
    let (tx, rx) = bounded_async::<i32>(32);

    let batch = vec![10, 20, 30, 40, 50];
    tx.send_batch(batch).await.unwrap();

    let values = rx.recv_batch(5).await.unwrap();
    assert_eq!(values, vec![10, 20, 30, 40, 50]);
  }

  #[tokio::test]
  async fn test_async_send_batch_mut_recv_batch_mut() {
    let (tx, rx) = bounded_async::<i32>(32);

    let mut batch = vec![100, 200, 300];
    tx.send_batch_mut(&mut batch).await.unwrap();
    assert!(batch.is_empty());

    let mut out = Vec::new();
    let count = rx.recv_batch_mut(&mut out, 3).await.unwrap();
    assert_eq!(count, 3);
    assert_eq!(out, vec![100, 200, 300]);
  }

  #[tokio::test]
  async fn test_async_batch_partial_completions() {
    let (tx, rx) = bounded_async::<i32>(16);

    let mut fill = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    tx.send_batch_mut(&mut fill).await.unwrap();

    let tx_clone = tx.clone();
    let blocked_send = tokio::spawn(async move { tx_clone.send_batch(vec![17, 18, 19]).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!blocked_send.is_finished());

    let mut out = Vec::new();
    let popped = rx.recv_batch_mut(&mut out, 3).await.unwrap();
    assert_eq!(popped, 3);
    assert_eq!(out, vec![1, 2, 3]);

    let send_result = blocked_send.await.unwrap().unwrap();
    assert_eq!(send_result, 3);
  }
}
